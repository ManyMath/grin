//! Create, open, close, delete, password change and mnemonic functions.

use std::ffi::c_void;
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::Arc;

use grin_core::global;
use grin_keychain::mnemonic;
use grin_keychain::ExtKeychain;
use grin_util::secp::key::SecretKey;
use grin_util::ZeroingString;
use grin_wallet_config::GRIN_WALLET_DIR;
use grin_wallet_impls::DefaultWalletImpl;
use grin_wallet_libwallet::{Error as LibwalletError, WalletInst};
use rand::RngCore;
use serde::Serialize;
use zeroize::{Zeroize, Zeroizing};

use crate::config::{FfiNodeClient, NodeSettings, WalletConfig};
use crate::error::{
    log_caught_panic, read_opt_nonempty, read_opt_secret, read_secret, read_str, to_json,
    to_secret_json, GrinError, GrinErrorCode, GrinFfiResult,
};
use crate::handle::{
    canonical_data_dir_create, canonical_data_dir_existing, get_session, lookup_session,
    register_session, remove_session, DirKey, DirLease, WalletInstance, WalletLc, WalletSession,
};

/// Name of the encrypted seed file inside `wallet_data` (impls keeps its own
/// constant in a private module).
pub const SEED_FILE: &str = "wallet.seed";

/// Directory of the wallet database inside `wallet_data` (libwallet's
/// `DB_DIR`; the LMDB files live below it).
const DB_DIR: &str = "db";

/// `DefaultLCProvider::open_wallet`'s message when `wallet.seed` cannot be
/// read or decrypted.
const OPEN_SEED_FAILED: &str = "Error opening wallet (is password correct?)";

/// `DefaultLCProvider::get_mnemonic`'s and `change_password`'s message when
/// `wallet.seed` cannot be read or decrypted.
const READ_SEED_FAILED: &str = "Error opening wallet seed file";

/// Fields of the encrypted `wallet.seed` JSON (impls' private
/// `EncryptedWalletSeed`). Only used to tell a damaged file from a wrong
/// password.
#[derive(serde::Deserialize)]
struct SeedFileShape {
    encrypted_seed: String,
    salt: String,
    nonce: String,
}

/// What `open_backend` does before opening.
pub(crate) enum OpenMode {
    Create(Option<ZeroingString>),
    Open,
}

#[derive(Serialize)]
struct HandleOut {
    handle: u64,
}

#[derive(Serialize)]
struct MnemonicOut<'a> {
    mnemonic: &'a str,
}

/// The lifecycle provider derives everything from `<top>/wallet_data`, so
/// the canonical data directory must still be named `wallet_data` (it is not
/// if `wallet_data` is a symlink to a directory with another name).
pub(crate) fn check_data_dir_name(data_dir: &Path) -> Result<(), GrinError> {
    if data_dir.file_name().and_then(|n| n.to_str()) == Some(GRIN_WALLET_DIR) {
        Ok(())
    } else {
        Err(GrinError::invalid_argument(format!(
            "{GRIN_WALLET_DIR} must be a directory, not a link to a differently named one"
        )))
    }
}

/// Builds the lifecycle provider on the canonical data directory, optionally
/// creates the seed and database, and opens the wallet with a keychain mask.
/// The caller holds the directory lease and has set the thread's chain type.
pub(crate) fn open_backend(
    client: FfiNodeClient,
    data_dir: &Path,
    password: ZeroingString,
    mode: OpenMode,
) -> Result<(WalletInstance, Option<SecretKey>), GrinError> {
    let top = data_dir
        .parent()
        .and_then(Path::to_str)
        .ok_or_else(|| GrinError::invalid_argument("wallet_dir is not a valid UTF-8 path"))?;
    let mut wallet: Box<dyn WalletInst<'static, WalletLc, FfiNodeClient, ExtKeychain>> =
        Box::new(DefaultWalletImpl::<FfiNodeClient>::new(client)?);
    let mask = {
        let lc = wallet.lc_provider()?;
        lc.set_top_level_directory(top)?;
        match mode {
            OpenMode::Create(mnemonic) => {
                lc.create_wallet(None, mnemonic, 32, password.clone(), false)?
            }
            OpenMode::Open => check_seed_file(data_dir)?,
        }
        lc.open_wallet(None, password, true, false)
            .map_err(|e| seed_error(e, OPEN_SEED_FAILED, data_dir))?
    };
    Ok((Arc::new(grin_util::Mutex::new(wallet)), mask))
}

/// Checks a mnemonic without echoing any of its words in the error.
pub(crate) fn validate_mnemonic_words(words: &str) -> Result<(), GrinError> {
    match mnemonic::to_entropy(words) {
        Ok(mut entropy) => {
            entropy.zeroize();
            Ok(())
        }
        Err(e) => {
            let message = match e {
                mnemonic::Error::BadWord(mut w) => {
                    w.zeroize();
                    "mnemonic contains a word that is not in the BIP39 English list".to_owned()
                }
                mnemonic::Error::BadChecksum(..) => "mnemonic checksum does not match".to_owned(),
                mnemonic::Error::InvalidLength(n) => {
                    format!("mnemonic must have 12, 15, 18, 21 or 24 words, got {n}")
                }
            };
            Err(GrinError::new(GrinErrorCode::InvalidMnemonic, message))
        }
    }
}

/// The lifecycle provider reports every failure to read `wallet.seed` (file
/// missing, unreadable, not valid JSON, wrong password) with one fixed
/// Lifecycle message. Only that exact message is looked at here, and the
/// file is then checked directly: if it reads and has the shape of a seed
/// file, the decryption is what failed and the password is wrong.
fn seed_error(e: LibwalletError, seed_message: &str, data_dir: &Path) -> GrinError {
    match &e {
        LibwalletError::Lifecycle(msg) if msg == seed_message => seed_open_error(data_dir),
        _ => e.into(),
    }
}

fn seed_open_error(data_dir: &Path) -> GrinError {
    match std::fs::read(data_dir.join(SEED_FILE)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => GrinError::new(
            GrinErrorCode::WalletNotFound,
            format!("{SEED_FILE} is missing from the wallet directory"),
        ),
        Err(e) => GrinError::new(
            GrinErrorCode::Io,
            format!("could not read {SEED_FILE}: {e}"),
        ),
        Ok(bytes) if seed_shape_ok(&bytes) => {
            GrinError::new(GrinErrorCode::InvalidPassword, "wrong password")
        }
        Ok(_) => damaged_seed(),
    }
}

fn damaged_seed() -> GrinError {
    GrinError::new(
        GrinErrorCode::Lifecycle,
        format!("{SEED_FILE} is damaged: it is not a valid seed file"),
    )
}

/// Whether `bytes` are a seed file libwallet can decrypt: JSON with three hex
/// strings, a 12-byte nonce, a salt of at least 8 bytes (8 in files written
/// before the seed format recorded its KDF iterations, 16 after) and a
/// ciphertext longer than its 16-byte tag (`EncryptedWalletSeed` in the
/// provider). Older libwallet versions index the nonce without checking its
/// length, so a file that parses but holds the wrong sizes is caught before
/// it is handed to the provider.
fn seed_shape_ok(bytes: &[u8]) -> bool {
    let Ok(shape) = serde_json::from_slice::<SeedFileShape>(bytes) else {
        return false;
    };
    let hex = |s: &str| grin_util::from_hex(s).ok();
    matches!(
        (hex(&shape.encrypted_seed), hex(&shape.salt), hex(&shape.nonce)),
        (Some(e), Some(s), Some(n)) if e.len() > 16 && s.len() >= 8 && n.len() == 12
    )
}

/// Rejects a present but malformed `wallet.seed` with a Lifecycle error
/// before the provider reads it. A missing or unreadable file is left to the
/// provider, whose error `seed_open_error` maps.
fn check_seed_file(data_dir: &Path) -> Result<(), GrinError> {
    match std::fs::read(data_dir.join(SEED_FILE)) {
        Ok(bytes) if !seed_shape_ok(&bytes) => Err(damaged_seed()),
        _ => Ok(()),
    }
}

/// Names of the `wallet.seed.bak*` files in `data_dir`.
fn seed_backups(data_dir: &Path) -> Vec<String> {
    let prefix = format!("{SEED_FILE}.bak");
    let mut names: Vec<String> = std::fs::read_dir(data_dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.starts_with(&prefix))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// A `change_password` failure after the old password was verified. The
/// provider moves `wallet.seed` to a `wallet.seed.bak*` file before writing
/// the new one, so the message says where the old seed is.
fn change_password_error(
    e: LibwalletError,
    data_dir: &Path,
    backups_before: &[String],
) -> GrinError {
    let seed_present = data_dir.join(SEED_FILE).is_file();
    let backup = seed_backups(data_dir)
        .into_iter()
        .rfind(|n| !backups_before.contains(n));
    let mut message = format!("the password change failed: {e}");
    match (&backup, seed_present) {
        (Some(b), false) => message.push_str(&format!(
            ". {SEED_FILE} is missing; the seed encrypted with the old password is kept \
             as {b} in the wallet directory (rename it to {SEED_FILE} to restore it)"
        )),
        (Some(b), true) => message.push_str(&format!(
            ". The seed encrypted with the old password is also kept as {b}"
        )),
        (None, _) => {}
    }
    GrinError::new(GrinErrorCode::Lifecycle, message).with_details(serde_json::json!({
        "seed_present": seed_present,
        "backup_file": backup,
    }))
}

/// Anything in a `wallet_data` without `wallet.seed` that shows an earlier
/// wallet lived there: the database directory or an LMDB file. Returns the
/// first such path, relative to `data_dir`.
fn leftover_wallet_data(data_dir: &Path) -> Option<String> {
    if std::fs::symlink_metadata(data_dir.join(DB_DIR)).is_ok() {
        return Some(DB_DIR.to_owned());
    }
    fn find_mdb(dir: &Path, depth: u32) -> Option<std::path::PathBuf> {
        for entry in std::fs::read_dir(dir).ok()?.filter_map(Result::ok) {
            let path = entry.path();
            let file_type = entry.file_type().ok()?;
            if file_type.is_file() && path.extension().is_some_and(|x| x == "mdb") {
                return Some(path);
            }
            if file_type.is_dir() && depth > 0 {
                if let Some(p) = find_mdb(&path, depth - 1) {
                    return Some(p);
                }
            }
        }
        None
    }
    find_mdb(data_dir, 3).map(|p| {
        p.strip_prefix(data_dir)
            .unwrap_or(&p)
            .to_string_lossy()
            .into_owned()
    })
}

/// `wallet.seed` is missing but a backup of it is not: a password change or
/// a recovery was interrupted after moving the seed aside. Reported in place
/// of "no wallet" or "leftover data" so that a caller does not treat the
/// directory as empty or delete it.
fn seed_backup_only(data_dir: &Path, wallet_dir: &str) -> Option<GrinError> {
    if data_dir.join(SEED_FILE).exists() {
        return None;
    }
    let backups = seed_backups(data_dir);
    let latest = backups.last()?.clone();
    Some(
        GrinError::new(
            GrinErrorCode::Lifecycle,
            format!(
                "{SEED_FILE} is missing but {latest} is in the wallet directory; an interrupted \
                 password change or recovery may have left it. Rename it to {SEED_FILE} to \
                 restore the wallet, and do not delete the directory"
            ),
        )
        .with_details(serde_json::json!({
            "wallet_dir": wallet_dir,
            "reason": "seed_backup_only",
            "backup_files": backups,
        })),
    )
}

fn wallet_not_found(wallet_dir: &str) -> GrinError {
    GrinError::new(
        GrinErrorCode::WalletNotFound,
        "no wallet exists in this directory",
    )
    .with_details(serde_json::json!({ "wallet_dir": wallet_dir }))
}

/// Creates a wallet in `<wallet_dir>/wallet_data` and opens it. `mnemonic`
/// may be NULL to generate a new 24-word seed. Returns `{"handle": u64}`.
// # Safety
// Every pointer argument must be NULL or point to a NUL-terminated string
// that stays valid and unchanged until the call returns.
#[no_mangle]
pub unsafe extern "C" fn grin_wallet_create(
    config_json: *const c_char,
    mnemonic: *const c_char,
    password: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let config = WalletConfig::parse(read_str(&config_json, "config_json")?)?;
        global::set_local_chain_type(config.chain_type);
        let mnemonic = read_opt_secret(mnemonic, "mnemonic")?;
        let password = read_secret(password, "password")?;
        if let Some(m) = &mnemonic {
            validate_mnemonic_words(m)?;
        }

        let (data_dir, existed) = canonical_data_dir_create(&config.wallet_dir)?;
        check_data_dir_name(&data_dir)?;
        let lease = DirLease::acquire(&data_dir)?;

        let leftover = if existed { leftover_wallet_data(&data_dir) } else { None };
        let opened = if let Some(e) = seed_backup_only(&data_dir, &config.wallet_dir) {
            Err(e)
        } else if data_dir.join(SEED_FILE).exists() {
            Err(GrinError::new(
                GrinErrorCode::WalletExists,
                "a wallet already exists in this directory",
            )
            .with_details(serde_json::json!({ "wallet_dir": config.wallet_dir })))
        } else if let Some(found) = leftover {
            // A new seed must not start on another seed's database (its
            // outputs, history and transaction contexts).
            Err(GrinError::new(
                GrinErrorCode::WalletExists,
                format!(
                    "the wallet directory holds leftover wallet data ({found}) but no {SEED_FILE}; \
                     delete the wallet directory before creating a wallet in it"
                ),
            )
            .with_details(serde_json::json!({
                "wallet_dir": config.wallet_dir,
                "reason": "leftover_wallet_data",
                "path": found,
            })))
        } else {
            config.node.http_client().and_then(|c| {
                open_backend(
                    FfiNodeClient::Http(c),
                    &data_dir,
                    password,
                    OpenMode::Create(mnemonic),
                )
            })
        };
        let (wallet, mask) = match opened {
            Ok(v) => v,
            Err(e) => {
                // Only remove what this call created; the lease is still held.
                if !existed && e.code != GrinErrorCode::WalletExists {
                    if let Err(rm) = std::fs::remove_dir_all(&data_dir) {
                        log::warn!("could not clean up a failed wallet create: {rm}");
                    }
                }
                return Err(e);
            }
        };

        let chain_type = config.chain_type;
        let handle = register_session(|h| {
            WalletSession::new(h, chain_type, wallet, mask, config.node, lease)
        });
        log::info!("wallet {handle} created");
        to_json(&HandleOut { handle })
    }
}

/// Opens an existing wallet. Returns `{"handle": u64}`.
// # Safety
// Every pointer argument must be NULL or point to a NUL-terminated string
// that stays valid and unchanged until the call returns.
#[no_mangle]
pub unsafe extern "C" fn grin_wallet_open(
    config_json: *const c_char,
    password: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let config = WalletConfig::parse(read_str(&config_json, "config_json")?)?;
        global::set_local_chain_type(config.chain_type);
        let password = read_secret(password, "password")?;

        let data_dir = canonical_data_dir_existing(&config.wallet_dir)?
            .ok_or_else(|| wallet_not_found(&config.wallet_dir))?;
        check_data_dir_name(&data_dir)?;
        let lease = DirLease::acquire(&data_dir)?;
        if let Some(e) = seed_backup_only(&data_dir, &config.wallet_dir) {
            return Err(e);
        }
        if !data_dir.join(SEED_FILE).is_file() {
            return Err(wallet_not_found(&config.wallet_dir));
        }
        let client = config.node.http_client()?;
        let (wallet, mask) =
            open_backend(FfiNodeClient::Http(client), &data_dir, password, OpenMode::Open)?;

        let chain_type = config.chain_type;
        let handle = register_session(|h| {
            WalletSession::new(h, chain_type, wallet, mask, config.node, lease)
        });
        log::info!("wallet {handle} opened");
        to_json(&HandleOut { handle })
    }
}

/// Removes the handle from the registry. The backend closes when the last
/// in-flight call on it returns. Call it once per handle: a second call gets
/// InvalidHandle.
#[no_mangle]
pub extern "C" fn grin_wallet_close(handle: u64) -> GrinFfiResult {
    ffi_guard! {
        let session = remove_session(handle).ok_or_else(|| {
            GrinError::new(
                GrinErrorCode::InvalidHandle,
                format!("no open wallet with handle {handle}"),
            )
        })?;
        drop(session);
        Ok(None)
    }
}

/// `NativeFinalizer` callback: `token` is the handle cast to a pointer.
/// Never fails and never unwinds.
// # Safety
// `token` must be a handle cast to a pointer, or NULL. It is only read as
// an integer and never dereferenced; the function is unsafe because it
// takes a raw pointer from C like the other entry points.
#[no_mangle]
pub unsafe extern "C" fn grin_wallet_close_finalizer(token: *mut c_void) {
    let handle = token as usize as u64;
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| {
        if let Some(session) = remove_session(handle) {
            drop(session);
        }
    })) {
        log_caught_panic("grin_wallet_close_finalizer", payload);
    }
}

/// Deletes `<wallet_dir>/wallet_data`. A non-zero `handle` for this
/// directory is closed first; a handle for another directory is rejected,
/// and a handle that is not registered is ignored. A missing directory
/// counts as already deleted.
// (Plain comment: doc comments here are copied into include/grin_ffi.h.)
// With the directory missing, a handle whose own data directory is gone too
// is taken to be this wallet's and is closed.
// # Safety
// Every pointer argument must be NULL or point to a NUL-terminated string
// that stays valid and unchanged until the call returns.
#[no_mangle]
pub unsafe extern "C" fn grin_wallet_delete(
    wallet_dir: *const c_char,
    handle: u64,
) -> GrinFfiResult {
    ffi_guard! {
        let wallet_dir = read_str(&wallet_dir, "wallet_dir")?;
        if wallet_dir.trim().is_empty() {
            return Err(GrinError::invalid_argument("wallet_dir must not be empty"));
        }
        let target = canonical_data_dir_existing(wallet_dir)?;
        if let Some(session) = lookup_session(handle) {
            let same = match &target {
                // The same path, or another path to the same directory
                // (a bind mount, a parent renamed since the open).
                Some(t) => *t == session.data_dir || DirKey::of(t)? == *session.dir_key(),
                // The directory was removed behind the wallet's back (its
                // canonical path no longer resolves); if the handle's own
                // data directory is gone as well, it is this wallet.
                None => matches!(
                    std::fs::symlink_metadata(&session.data_dir),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound
                ),
            };
            if !same {
                return Err(GrinError::invalid_argument(
                    "handle belongs to a different wallet directory",
                ));
            }
            drop(session);
            drop(remove_session(handle));
        }
        let Some(target) = target else {
            return Ok(None);
        };
        check_data_dir_name(&target)?;
        let lease = DirLease::acquire(&target)?;
        std::fs::remove_dir_all(&target)?;
        drop(lease);
        log::info!("wallet data deleted");
        Ok(None)
    }
}

/// Re-encrypts `wallet.seed` with a new password.
// # Safety
// Every pointer argument must be NULL or point to a NUL-terminated string
// that stays valid and unchanged until the call returns.
#[no_mangle]
pub unsafe extern "C" fn grin_wallet_change_password(
    handle: u64,
    old_pw: *const c_char,
    new_pw: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        let old = read_secret(old_pw, "old_pw")?;
        let new = read_secret(new_pw, "new_pw")?;
        let mut w = session.wallet.lock();
        let lc = w.lc_provider()?;
        // Decrypt the seed with the old password first, so InvalidPassword
        // means exactly that: the provider reports a failure to write the
        // new seed with the same text as a wrong password. The provider
        // replaces wallet.seed atomically, so a failure leaves the old file;
        // the backup handling below covers seeds left by older versions.
        check_seed_file(&session.data_dir)?;
        drop(
            lc.get_mnemonic(None, old.clone())
                .map_err(|e| seed_error(e, READ_SEED_FAILED, &session.data_dir))?,
        );
        let backups_before = seed_backups(&session.data_dir);
        lc.change_password(None, old, new)
            .map_err(|e| change_password_error(e, &session.data_dir, &backups_before))?;
        Ok(None)
    }
}

/// Returns `{"mnemonic": "<words>"}` after checking the password.
// # Safety
// Every pointer argument must be NULL or point to a NUL-terminated string
// that stays valid and unchanged until the call returns.
#[no_mangle]
pub unsafe extern "C" fn grin_wallet_get_mnemonic(
    handle: u64,
    password: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        let password = read_secret(password, "password")?;
        let words = {
            let mut w = session.wallet.lock();
            check_seed_file(&session.data_dir)?;
            w.lc_provider()?
                .get_mnemonic(None, password)
                .map_err(|e| seed_error(e, READ_SEED_FAILED, &session.data_dir))?
        };
        to_secret_json(&MnemonicOut { mnemonic: &words })
    }
}

/// Points the wallet at another node (and optional SOCKS proxy) without
/// reopening the database. `api_secret` and `socks_proxy` may be NULL.
// # Safety
// Every pointer argument must be NULL or point to a NUL-terminated string
// that stays valid and unchanged until the call returns.
#[no_mangle]
pub unsafe extern "C" fn grin_wallet_update_network(
    handle: u64,
    node_url: *const c_char,
    api_secret: *const c_char,
    socks_proxy: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        let url = read_str(&node_url, "node_url")?;
        let secret = read_opt_nonempty(&api_secret, "api_secret")?;
        let proxy = read_opt_nonempty(&socks_proxy, "socks_proxy")?;
        let timeout = session.node_settings().timeout;
        let settings = NodeSettings::new(url, secret, proxy, timeout)?;
        let client = settings.http_client()?;
        // Both change while the wallet mutex is held, with the settings
        // lock taken inside it (the only place the two are held together),
        // so a reader holding the wallet mutex never sees the new client
        // with the old settings or the other way round.
        let mut w = session.wallet.lock();
        let backend = w.lc_provider()?.wallet_inst()?;
        let mut node = session.node_settings();
        *backend.w2n_client() = FfiNodeClient::Http(client);
        *node = settings;
        Ok(None)
    }
}

/// Returns `{"mnemonic": "<words>"}` with 12, 15, 18, 21 or 24 new words.
#[no_mangle]
pub extern "C" fn grin_mnemonic_generate(word_count: u32) -> GrinFfiResult {
    ffi_guard! {
        let len = match word_count {
            12 => 16,
            15 => 20,
            18 => 24,
            21 => 28,
            24 => 32,
            n => {
                return Err(GrinError::invalid_argument(format!(
                    "word_count must be 12, 15, 18, 21 or 24, got {n}"
                )))
            }
        };
        let mut entropy = Zeroizing::new(vec![0u8; len]);
        let mut rng = rand::rngs::OsRng::new()
            .map_err(|e| GrinError::internal(format!("no OS random source: {e}")))?;
        rng.try_fill_bytes(&mut entropy)
            .map_err(|e| GrinError::internal(format!("OS random source failed: {e}")))?;
        let words = ZeroingString::from(
            mnemonic::from_entropy(&entropy)
                .map_err(|e| GrinError::internal(format!("mnemonic encoding failed: {e}")))?,
        );
        to_secret_json(&MnemonicOut { mnemonic: &words })
    }
}

/// Returns `{"valid": bool, "word_count": u32}`. A bad mnemonic is not an
/// error, only `valid: false`.
// # Safety
// Every pointer argument must be NULL or point to a NUL-terminated string
// that stays valid and unchanged until the call returns.
#[no_mangle]
pub unsafe extern "C" fn grin_mnemonic_validate(mnemonic: *const c_char) -> GrinFfiResult {
    ffi_guard! {
        let words = read_secret(mnemonic, "mnemonic")?;
        let word_count = u32::try_from(words.split_whitespace().count()).unwrap_or(u32::MAX);
        let valid = validate_mnemonic_words(&words).is_ok();
        to_json(&serde_json::json!({ "valid": valid, "word_count": word_count }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mnemonic_errors_do_not_echo_words() {
        let e = validate_mnemonic_words(&"zzzsecretword ".repeat(12)).unwrap_err();
        assert_eq!(e.code, GrinErrorCode::InvalidMnemonic);
        assert!(!e.message.contains("zzzsecretword"));
        let e = validate_mnemonic_words("abandon").unwrap_err();
        assert!(e.message.contains("got 1"));
    }

    #[test]
    fn data_dir_name_check() {
        assert!(check_data_dir_name(Path::new("/a/wallet_data")).is_ok());
        assert!(check_data_dir_name(Path::new("/a/other")).is_err());
    }

    #[test]
    fn seed_errors_are_told_apart() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let read_failed = || LibwalletError::Lifecycle(READ_SEED_FAILED.into());
        assert_eq!(
            seed_error(read_failed(), READ_SEED_FAILED, d).code,
            GrinErrorCode::WalletNotFound
        );
        std::fs::write(d.join(SEED_FILE), b"not json").unwrap();
        assert_eq!(
            seed_error(read_failed(), READ_SEED_FAILED, d).code,
            GrinErrorCode::Lifecycle
        );
        // The right JSON shape with the wrong sizes is a damaged file, not
        // a wrong password: the provider would index past the nonce.
        std::fs::write(
            d.join(SEED_FILE),
            br#"{"encrypted_seed": "00", "salt": "00", "nonce": "00"}"#,
        )
        .unwrap();
        assert_eq!(
            seed_error(read_failed(), READ_SEED_FAILED, d).code,
            GrinErrorCode::Lifecycle
        );
        assert_eq!(
            check_seed_file(d).unwrap_err().code,
            GrinErrorCode::Lifecycle
        );
        // A seed file the provider can decrypt (right sizes) that fails to
        // decrypt is a wrong password.
        std::fs::write(
            d.join(SEED_FILE),
            format!(
                r#"{{"encrypted_seed": "{}", "salt": "{}", "nonce": "{}"}}"#,
                "ab".repeat(48),
                "cd".repeat(8),
                "ef".repeat(12)
            ),
        )
        .unwrap();
        assert!(check_seed_file(d).is_ok());
        assert_eq!(
            seed_error(read_failed(), READ_SEED_FAILED, d).code,
            GrinErrorCode::InvalidPassword
        );
        // The current format: a 16-byte salt and a recorded iteration count.
        std::fs::write(
            d.join(SEED_FILE),
            format!(
                r#"{{"encrypted_seed": "{}", "salt": "{}", "nonce": "{}", "kdf_iterations": 210000}}"#,
                "ab".repeat(48),
                "cd".repeat(16),
                "ef".repeat(12)
            ),
        )
        .unwrap();
        assert!(check_seed_file(d).is_ok());
        // Other messages are never read as a wrong password, even when they
        // mention one.
        let other = LibwalletError::Lifecycle(
            "New and Old wallet seeds are not equal on password change, not removing backups."
                .into(),
        );
        assert_eq!(
            seed_error(other, READ_SEED_FAILED, d).code,
            GrinErrorCode::Lifecycle
        );
        let open = LibwalletError::Lifecycle(OPEN_SEED_FAILED.into());
        assert_eq!(
            seed_error(open, READ_SEED_FAILED, d).code,
            GrinErrorCode::Lifecycle
        );
    }

    #[test]
    fn change_password_failure_names_the_backup() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join(format!("{SEED_FILE}.bak")), b"older").unwrap();
        let before = seed_backups(d);
        // What the provider leaves behind when writing the new seed fails:
        // the old one moved aside, no wallet.seed.
        std::fs::write(d.join(format!("{SEED_FILE}.bak.1")), b"old").unwrap();
        let e = change_password_error(
            LibwalletError::Lifecycle(READ_SEED_FAILED.into()),
            d,
            &before,
        );
        assert_eq!(e.code, GrinErrorCode::Lifecycle);
        assert!(e.message.contains("wallet.seed.bak.1"), "{}", e.message);
        assert!(e.message.contains("is missing"), "{}", e.message);
        let details = e.details.unwrap();
        assert_eq!(details["seed_present"], false);
        assert_eq!(details["backup_file"], "wallet.seed.bak.1");
    }

    #[test]
    fn leftover_data_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert_eq!(leftover_wallet_data(d), None);
        std::fs::write(d.join("tx_metadata.json"), b"{}").unwrap();
        assert_eq!(leftover_wallet_data(d), None);
        std::fs::create_dir_all(d.join("saved_txs").join("lmdb")).unwrap();
        std::fs::write(d.join("saved_txs").join("lmdb").join("data.mdb"), b"").unwrap();
        assert!(leftover_wallet_data(d).unwrap().ends_with("data.mdb"));
        std::fs::create_dir(d.join(DB_DIR)).unwrap();
        assert_eq!(leftover_wallet_data(d).as_deref(), Some(DB_DIR));
    }
}
