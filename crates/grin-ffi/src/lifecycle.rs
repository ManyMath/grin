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
    read_opt_nonempty, read_opt_secret, read_secret, read_str, to_json, to_secret_json, GrinError,
    GrinErrorCode, GrinFfiResult,
};
use crate::handle::{
    canonical_data_dir_create, canonical_data_dir_existing, get_session, lookup_session,
    register_session, remove_session, DirLease, WalletInstance, WalletLc, WalletSession,
};

/// Name of the encrypted seed file inside `wallet_data` (impls keeps its own
/// constant in a private module).
pub const SEED_FILE: &str = "wallet.seed";

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
        if let OpenMode::Create(mnemonic) = mode {
            lc.create_wallet(None, mnemonic, 32, password.clone(), false)?;
        }
        lc.open_wallet(None, password, true, false)?
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

/// `get_mnemonic` and `change_password` report a seed that fails to decrypt
/// as this Lifecycle error. The session is open, so the file exists and a
/// decryption failure means a wrong password.
fn seed_error(e: LibwalletError) -> GrinError {
    match &e {
        LibwalletError::Lifecycle(msg) if msg == "Error opening wallet seed file" => {
            GrinError::new(GrinErrorCode::InvalidPassword, "wrong password")
        }
        _ => e.into(),
    }
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
#[no_mangle]
pub extern "C" fn grin_wallet_create(
    config_json: *const c_char,
    mnemonic: *const c_char,
    password: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let config = WalletConfig::parse(read_str(config_json, "config_json")?)?;
        global::set_local_chain_type(config.chain_type);
        let mnemonic = read_opt_secret(mnemonic, "mnemonic")?;
        let password = read_secret(password, "password")?;
        if let Some(m) = &mnemonic {
            validate_mnemonic_words(m)?;
        }

        let (data_dir, existed) = canonical_data_dir_create(&config.wallet_dir)?;
        check_data_dir_name(&data_dir)?;
        let lease = DirLease::acquire(&data_dir)?;

        let opened = if data_dir.join(SEED_FILE).exists() {
            Err(GrinError::new(
                GrinErrorCode::WalletExists,
                "a wallet already exists in this directory",
            )
            .with_details(serde_json::json!({ "wallet_dir": config.wallet_dir })))
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
#[no_mangle]
pub extern "C" fn grin_wallet_open(
    config_json: *const c_char,
    password: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let config = WalletConfig::parse(read_str(config_json, "config_json")?)?;
        global::set_local_chain_type(config.chain_type);
        let password = read_secret(password, "password")?;

        let data_dir = canonical_data_dir_existing(&config.wallet_dir)?
            .ok_or_else(|| wallet_not_found(&config.wallet_dir))?;
        check_data_dir_name(&data_dir)?;
        let lease = DirLease::acquire(&data_dir)?;
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
#[no_mangle]
pub extern "C" fn grin_wallet_close_finalizer(token: *mut c_void) {
    let handle = token as usize as u64;
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(session) = remove_session(handle) {
            drop(session);
        }
    }));
}

/// Deletes `<wallet_dir>/wallet_data`. A non-zero `handle` for this
/// directory is closed first; a handle for another directory is rejected,
/// and a handle that is not registered is ignored. A missing directory
/// counts as already deleted.
#[no_mangle]
pub extern "C" fn grin_wallet_delete(wallet_dir: *const c_char, handle: u64) -> GrinFfiResult {
    ffi_guard! {
        let wallet_dir = read_str(wallet_dir, "wallet_dir")?;
        if wallet_dir.trim().is_empty() {
            return Err(GrinError::invalid_argument("wallet_dir must not be empty"));
        }
        let target = canonical_data_dir_existing(wallet_dir)?;
        if let Some(session) = lookup_session(handle) {
            if target.as_deref() != Some(session.data_dir.as_path()) {
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
#[no_mangle]
pub extern "C" fn grin_wallet_change_password(
    handle: u64,
    old_pw: *const c_char,
    new_pw: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        let old = read_secret(old_pw, "old_pw")?;
        let new = read_secret(new_pw, "new_pw")?;
        let mut w = session.wallet.lock();
        w.lc_provider()?
            .change_password(None, old, new)
            .map_err(seed_error)?;
        Ok(None)
    }
}

/// Returns `{"mnemonic": "<words>"}` after checking the password.
#[no_mangle]
pub extern "C" fn grin_wallet_get_mnemonic(handle: u64, password: *const c_char) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        let password = read_secret(password, "password")?;
        let words = {
            let mut w = session.wallet.lock();
            w.lc_provider()?
                .get_mnemonic(None, password)
                .map_err(seed_error)?
        };
        to_secret_json(&MnemonicOut { mnemonic: &words }, words.len())
    }
}

/// Points the wallet at another node (and optional SOCKS proxy) without
/// reopening the database. `api_secret` and `socks_proxy` may be NULL.
#[no_mangle]
pub extern "C" fn grin_wallet_update_network(
    handle: u64,
    node_url: *const c_char,
    api_secret: *const c_char,
    socks_proxy: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        let url = read_str(node_url, "node_url")?;
        let secret = read_opt_nonempty(api_secret, "api_secret")?;
        let proxy = read_opt_nonempty(socks_proxy, "socks_proxy")?;
        let timeout = session.node_settings().timeout;
        let settings = NodeSettings::new(url, secret, proxy, timeout)?;
        let client = settings.http_client()?;
        {
            let mut w = session.wallet.lock();
            let backend = w.lc_provider()?.wallet_inst()?;
            *backend.w2n_client() = FfiNodeClient::Http(client);
        }
        *session.node_settings() = settings;
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
        to_secret_json(&MnemonicOut { mnemonic: &words }, words.len())
    }
}

/// Returns `{"valid": bool, "word_count": u32}`. A bad mnemonic is not an
/// error, only `valid: false`.
#[no_mangle]
pub extern "C" fn grin_mnemonic_validate(mnemonic: *const c_char) -> GrinFfiResult {
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
}
