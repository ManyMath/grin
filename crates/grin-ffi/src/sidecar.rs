//! Per-transaction metadata that `TxLogEntry` does not store, kept in
//! `<wallet_dir>/wallet_data/tx_metadata.json` (docs/architecture.md 4.4).
//!
//! Callers treat every failure here as non-fatal: log it and carry on.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SIDECAR_FILE: &str = "tx_metadata.json";
const SIDECAR_TMP: &str = "tx_metadata.json.tmp";
const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq, Eq)]
pub struct TxSidecarEntry {
    /// Latest armored S1/S2 or I1/I2 Slatepack string so the user can view/re-share
    /// an unfinished interactive transaction from the transaction details screen.
    pub armored_slatepack: Option<String>,
    /// Counterparty address (`grin1...` or `http://...`) for transactions that do not
    /// carry a StoredProofInfo (`payment_proof`).
    pub counterparty_address: Option<String>,
}

impl TxSidecarEntry {
    /// Overwrites the fields that `update` sets and keeps the rest.
    fn merge(&mut self, update: TxSidecarEntry) {
        if update.armored_slatepack.is_some() {
            self.armored_slatepack = update.armored_slatepack;
        }
        if update.counterparty_address.is_some() {
            self.counterparty_address = update.counterparty_address;
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct SidecarFile {
    version: u32,
    /// Keyed by slate id.
    entries: BTreeMap<Uuid, TxSidecarEntry>,
}

/// Serializes read-modify-write cycles within this process.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Snapshot of the sidecar file.
#[derive(Default, Debug)]
pub struct Sidecar {
    entries: BTreeMap<Uuid, TxSidecarEntry>,
}

impl Sidecar {
    /// Reads the sidecar from `data_dir`. A missing file is empty; an
    /// unreadable or corrupt one is logged and treated as empty.
    pub fn load(data_dir: &Path) -> Sidecar {
        match read_file(data_dir) {
            Ok(entries) => Sidecar { entries },
            Err(e) => {
                log::warn!("ignoring unreadable transaction sidecar: {e}");
                Sidecar::default()
            }
        }
    }

    pub fn get(&self, slate_id: &Uuid) -> Option<&TxSidecarEntry> {
        self.entries.get(slate_id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Merges `update` into the entry for `slate_id` and rewrites the file
    /// atomically (temp file, fsync, rename). A corrupt existing file is
    /// replaced.
    pub fn upsert(data_dir: &Path, slate_id: Uuid, update: TxSidecarEntry) -> io::Result<()> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let mut entries = read_file(data_dir).unwrap_or_else(|e| {
            log::warn!("replacing unreadable transaction sidecar: {e}");
            BTreeMap::new()
        });
        entries.entry(slate_id).or_default().merge(update);
        write_file(data_dir, entries)
    }
}

fn path_of(data_dir: &Path) -> PathBuf {
    data_dir.join(SIDECAR_FILE)
}

fn read_file(data_dir: &Path) -> io::Result<BTreeMap<Uuid, TxSidecarEntry>> {
    let bytes = match fs::read(path_of(data_dir)) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e),
    };
    let file: SidecarFile = serde_json::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if file.version > FORMAT_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "sidecar format {} is newer than {FORMAT_VERSION}",
                file.version
            ),
        ));
    }
    Ok(file.entries)
}

fn write_file(data_dir: &Path, entries: BTreeMap<Uuid, TxSidecarEntry>) -> io::Result<()> {
    let file = SidecarFile {
        version: FORMAT_VERSION,
        entries,
    };
    let json = serde_json::to_vec_pretty(&file)?;
    let tmp = data_dir.join(SIDECAR_TMP);
    {
        let mut f = File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path_of(data_dir))?;
    // Make the rename itself durable where the platform allows it.
    #[cfg(unix)]
    if let Ok(dir) = File::open(data_dir) {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_merges_and_survives_reload() {
        let dir = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        let other = Uuid::new_v4();
        assert!(Sidecar::load(dir.path()).is_empty());

        Sidecar::upsert(
            dir.path(),
            id,
            TxSidecarEntry {
                armored_slatepack: Some("BEGINSLATEPACK. one. ENDSLATEPACK.".into()),
                counterparty_address: None,
            },
        )
        .unwrap();
        Sidecar::upsert(
            dir.path(),
            id,
            TxSidecarEntry {
                armored_slatepack: None,
                counterparty_address: Some("http://example.com".into()),
            },
        )
        .unwrap();
        Sidecar::upsert(dir.path(), other, TxSidecarEntry::default()).unwrap();

        let s = Sidecar::load(dir.path());
        assert_eq!(s.len(), 2);
        let e = s.get(&id).unwrap();
        assert_eq!(
            e.armored_slatepack.as_deref(),
            Some("BEGINSLATEPACK. one. ENDSLATEPACK.")
        );
        assert_eq!(
            e.counterparty_address.as_deref(),
            Some("http://example.com")
        );
        assert!(!dir.path().join(SIDECAR_TMP).exists());

        // A later slatepack replaces the earlier one.
        Sidecar::upsert(
            dir.path(),
            id,
            TxSidecarEntry {
                armored_slatepack: Some("two".into()),
                counterparty_address: None,
            },
        )
        .unwrap();
        let s = Sidecar::load(dir.path());
        assert_eq!(
            s.get(&id).unwrap().armored_slatepack.as_deref(),
            Some("two")
        );
        assert_eq!(
            s.get(&id).unwrap().counterparty_address.as_deref(),
            Some("http://example.com")
        );
    }

    #[test]
    fn corrupt_file_is_empty_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(SIDECAR_FILE), b"{not json").unwrap();
        fs::write(dir.path().join(SIDECAR_TMP), b"stale").unwrap();
        assert!(Sidecar::load(dir.path()).is_empty());
        let id = Uuid::new_v4();
        Sidecar::upsert(
            dir.path(),
            id,
            TxSidecarEntry {
                armored_slatepack: None,
                counterparty_address: Some("grin1x".into()),
            },
        )
        .unwrap();
        assert_eq!(Sidecar::load(dir.path()).len(), 1);
    }

    #[test]
    fn missing_directory_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("nope");
        assert!(Sidecar::upsert(&gone, Uuid::new_v4(), TxSidecarEntry::default()).is_err());
        assert!(Sidecar::load(&gone).is_empty());
    }
}
