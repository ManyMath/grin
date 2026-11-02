//! Session registry, `Drop`-based close and the per-directory LMDB guard
//! (docs/architecture.md section 4.5).
//!
//! Lock rules: `SESSION_REGISTRY` and `OPEN_DIRS` are never held at the same
//! time, no LMDB work runs while either is held, and both are locked with
//! `unwrap_or_else(PoisonError::into_inner)` because `Drop` may run while a
//! thread unwinds.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

use grin_core::global::{self, ChainTypes};
use grin_keychain::ExtKeychain;
use grin_util::secp::key::SecretKey;
use grin_wallet_config::GRIN_WALLET_DIR;
use grin_wallet_impls::DefaultLCProvider;
use grin_wallet_libwallet::WalletInst;
use zeroize::Zeroize;

use crate::config::{FfiNodeClient, NodeSettings};
use crate::error::{GrinError, GrinErrorCode};

/// Lifecycle provider of every wallet in this crate.
pub type WalletLc = DefaultLCProvider<FfiNodeClient, ExtKeychain>;

/// The one concrete wallet type. libwallet's owner functions take exactly
/// this shape (`Arc<grin_util::Mutex<Box<dyn WalletInst>>>`).
pub type WalletInstance =
    Arc<grin_util::Mutex<Box<dyn WalletInst<'static, WalletLc, FfiNodeClient, ExtKeychain>>>>;

/// Progress of the most recent scan, readable without the wallet mutex.
#[derive(Debug, Default)]
pub struct ScanProgressState {
    pub is_scanning: AtomicBool,
    pub cancel_requested: AtomicBool,
    pub scanned_height: AtomicU64,
    pub tip_height: AtomicU64,
    pub start_height: AtomicU64,
}

impl ScanProgressState {
    /// Percentage of the current or last scan range covered, 0 to 100.
    pub fn progress_pct(&self) -> u8 {
        let start = self.start_height.load(Ordering::Acquire);
        let tip = self.tip_height.load(Ordering::Acquire);
        let scanned = self.scanned_height.load(Ordering::Acquire);
        if tip == 0 {
            return 0;
        }
        if tip <= start {
            return if scanned >= tip { 100 } else { 0 };
        }
        let done = scanned.saturating_sub(start) as u128 * 100 / (tip - start) as u128;
        done.min(100) as u8
    }
}

/// An open wallet. Dropping the last `Arc` closes the backend, zeroizes the
/// keychain mask and then releases the directory lease.
pub struct WalletSession {
    pub handle: u64,
    pub chain_type: ChainTypes,
    pub wallet: WalletInstance,
    /// XOR mask for the in-memory keychain. Zeroized in `Drop`.
    pub keychain_mask: Mutex<Option<SecretKey>>,
    pub scan_state: ScanProgressState,
    /// Canonical `<wallet_dir>/wallet_data`.
    pub data_dir: PathBuf,
    pub node: Mutex<NodeSettings>,
    /// In-process chain of a wallet made by `grin_test_create_wallet`.
    #[cfg(feature = "test-utils")]
    pub(crate) test_chain: Option<Arc<crate::test_support::TestChain>>,
    /// Declared last so it is dropped after everything above.
    _lease: DirLease,
}

impl WalletSession {
    pub(crate) fn new(
        handle: u64,
        chain_type: ChainTypes,
        wallet: WalletInstance,
        keychain_mask: Option<SecretKey>,
        node: NodeSettings,
        lease: DirLease,
    ) -> Self {
        WalletSession {
            handle,
            chain_type,
            wallet,
            keychain_mask: Mutex::new(keychain_mask),
            scan_state: ScanProgressState::default(),
            data_dir: lease.path().to_path_buf(),
            node: Mutex::new(node),
            #[cfg(feature = "test-utils")]
            test_chain: None,
            _lease: lease,
        }
    }

    /// Copy of the keychain mask for one libwallet call.
    pub fn mask(&self) -> Option<SecretKey> {
        self.keychain_mask
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn node_settings(&self) -> MutexGuard<'_, NodeSettings> {
        self.node.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Clones the wallet's node client so network calls run without the
    /// wallet mutex held.
    pub fn node_client(&self) -> Result<FfiNodeClient, GrinError> {
        let mut w = self.wallet.lock();
        let lc = w.lc_provider()?;
        let backend = lc.wallet_inst()?;
        Ok(backend.w2n_client().clone())
    }
}

impl std::fmt::Debug for WalletSession {
    // Leaves out the wallet and the keychain mask on purpose.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalletSession")
            .field("handle", &self.handle)
            .field("chain_type", &self.chain_type)
            .field("data_dir", &self.data_dir)
            .finish_non_exhaustive()
    }
}

impl Drop for WalletSession {
    fn drop(&mut self) {
        self.scan_state
            .cancel_requested
            .store(true, Ordering::Release);
        {
            let mut w = self.wallet.lock();
            match w.lc_provider() {
                Ok(lc) => {
                    if let Err(e) = lc.close_wallet(None) {
                        log::warn!("wallet {}: close_wallet failed: {e}", self.handle);
                    }
                }
                Err(e) => log::warn!("wallet {}: no lifecycle provider: {e}", self.handle),
            }
        }
        if let Some(mut mask) = self
            .keychain_mask
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            mask.zeroize();
        }
        log::debug!("wallet {} closed", self.handle);
        // `_lease` is the last field, so it is released after the backend.
    }
}

static SESSION_REGISTRY: LazyLock<Mutex<HashMap<u64, Arc<WalletSession>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

static OPEN_DIRS: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

fn registry() -> MutexGuard<'static, HashMap<u64, Arc<WalletSession>>> {
    SESSION_REGISTRY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

fn open_dirs() -> MutexGuard<'static, HashSet<PathBuf>> {
    OPEN_DIRS.lock().unwrap_or_else(PoisonError::into_inner)
}

fn invalid_handle(handle: u64) -> GrinError {
    GrinError::new(
        GrinErrorCode::InvalidHandle,
        format!("no open wallet with handle {handle}"),
    )
}

/// Allocates a handle, builds the session outside the registry lock and
/// registers it.
pub(crate) fn register_session<F>(build: F) -> u64
where
    F: FnOnce(u64) -> WalletSession,
{
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    let session = Arc::new(build(handle));
    registry().insert(handle, session);
    handle
}

/// Clones the session out of the registry, releases the lock and sets this
/// thread's chain type to the wallet's.
pub fn get_session(handle: u64) -> Result<Arc<WalletSession>, GrinError> {
    let session = lookup_session(handle).ok_or_else(|| invalid_handle(handle))?;
    global::set_local_chain_type(session.chain_type);
    Ok(session)
}

/// Registry lookup without touching the chain type.
pub(crate) fn lookup_session(handle: u64) -> Option<Arc<WalletSession>> {
    if handle == 0 {
        return None;
    }
    registry().get(&handle).cloned()
}

/// Marks the session cancelled and removes it from the registry. The caller
/// drops the returned `Arc` after the lock is gone; if it was the last one,
/// the backend closes right there.
pub(crate) fn remove_session(handle: u64) -> Option<Arc<WalletSession>> {
    if handle == 0 {
        return None;
    }
    let session = registry().remove(&handle);
    if let Some(s) = &session {
        s.scan_state.cancel_requested.store(true, Ordering::Release);
    }
    session
}

/// RAII membership of a canonical wallet data directory in `OPEN_DIRS`.
#[derive(Debug)]
pub struct DirLease(PathBuf);

impl DirLease {
    /// Fails with `WalletBusy` if another lease holds `dir`. `dir` must be
    /// canonical.
    pub fn acquire(dir: &Path) -> Result<DirLease, GrinError> {
        if open_dirs().insert(dir.to_path_buf()) {
            Ok(DirLease(dir.to_path_buf()))
        } else {
            Err(GrinError::new(
                GrinErrorCode::WalletBusy,
                "the wallet directory is in use by an open or still closing wallet",
            )
            .with_details(serde_json::json!({ "wallet_dir": dir.to_string_lossy() })))
        }
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for DirLease {
    fn drop(&mut self) {
        open_dirs().remove(&self.0);
    }
}

/// `<wallet_dir>/wallet_data` as given (not canonical).
pub fn data_dir_path(wallet_dir: &str) -> PathBuf {
    Path::new(wallet_dir).join(GRIN_WALLET_DIR)
}

/// For create: makes `<wallet_dir>/wallet_data` if needed and returns its
/// canonical path plus whether it existed before.
pub fn canonical_data_dir_create(wallet_dir: &str) -> Result<(PathBuf, bool), GrinError> {
    let dir = data_dir_path(wallet_dir);
    let existed = dir.exists();
    std::fs::create_dir_all(&dir)?;
    Ok((std::fs::canonicalize(&dir)?, existed))
}

/// For open and delete: the canonical data directory, or `None` if it does
/// not exist.
pub fn canonical_data_dir_existing(wallet_dir: &str) -> Result<Option<PathBuf>, GrinError> {
    match std::fs::canonicalize(data_dir_path(wallet_dir)) {
        Ok(p) => Ok(Some(p)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_is_exclusive_and_released() {
        let dir = tempfile::tempdir().unwrap();
        let p = std::fs::canonicalize(dir.path()).unwrap();
        let a = DirLease::acquire(&p).unwrap();
        let e = DirLease::acquire(&p).unwrap_err();
        assert_eq!(e.code, GrinErrorCode::WalletBusy);
        assert_eq!(
            e.details.unwrap()["wallet_dir"],
            p.to_string_lossy().as_ref()
        );
        drop(a);
        let _b = DirLease::acquire(&p).unwrap();
    }

    #[test]
    fn progress_pct() {
        let s = ScanProgressState::default();
        assert_eq!(s.progress_pct(), 0);
        s.start_height.store(1, Ordering::Release);
        s.tip_height.store(101, Ordering::Release);
        s.scanned_height.store(51, Ordering::Release);
        assert_eq!(s.progress_pct(), 50);
        s.scanned_height.store(500, Ordering::Release);
        assert_eq!(s.progress_pct(), 100);
    }

    #[test]
    fn unknown_handles() {
        assert_eq!(
            get_session(0).unwrap_err().code,
            GrinErrorCode::InvalidHandle
        );
        assert_eq!(
            get_session(u64::MAX).unwrap_err().code,
            GrinErrorCode::InvalidHandle
        );
        assert!(remove_session(0).is_none());
    }
}
