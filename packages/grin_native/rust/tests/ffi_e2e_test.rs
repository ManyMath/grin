//! End-to-end tests that drive the C ABI the way a C or Dart caller would:
//! CString arguments in, JSON out, every result freed.

#![cfg(feature = "test-utils")]

use std::ffi::{c_void, CStr, CString};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::{Arc, Mutex};
use std::thread;

use grin_ffi::test_support::{retain_session, set_scan_test_hooks};
use grin_ffi::*;
use grin_wallet_libwallet::ScannedBlockInfo;
use serde_json::{json, Value};

const OK: i32 = 0;
const INVALID_ARGUMENT: i32 = GrinErrorCode::InvalidArgument as i32;
const INVALID_UTF8: i32 = GrinErrorCode::InvalidUtf8 as i32;
const JSON: i32 = GrinErrorCode::Json as i32;
const WALLET_NOT_FOUND: i32 = GrinErrorCode::WalletNotFound as i32;
const INVALID_PASSWORD: i32 = GrinErrorCode::InvalidPassword as i32;
const WALLET_EXISTS: i32 = GrinErrorCode::WalletExists as i32;
const INVALID_HANDLE: i32 = GrinErrorCode::InvalidHandle as i32;
const LIFECYCLE: i32 = GrinErrorCode::Lifecycle as i32;
const NODE: i32 = GrinErrorCode::Node as i32;
const CANCELLED: i32 = GrinErrorCode::Cancelled as i32;
const INTERNAL_PANIC: i32 = GrinErrorCode::InternalPanic as i32;
const WALLET_BUSY: i32 = GrinErrorCode::WalletBusy as i32;
const INVALID_ADDRESS: i32 = GrinErrorCode::InvalidAddress as i32;
const INVALID_MNEMONIC: i32 = GrinErrorCode::InvalidMnemonic as i32;

const MAINNET: u8 = 0;
const TESTNET: u8 = 1;
const AUTOMATED: u8 = 3;

const PASSWORD: &str = "correct horse";
/// BIP39 test vector (valid checksum).
const MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const UNREACHABLE_NODE: &str = "http://127.0.0.1:1";

/// Reads and frees a result. Ok carries the data JSON (Null when the
/// function returns nothing); Err carries the code and the error JSON.
fn read_result(res: GrinFfiResult) -> Result<Value, (i32, Value)> {
    let read = |p: *mut c_char| -> Option<String> {
        if p.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_owned())
        }
    };
    let code = res.code;
    let data = read(res.data);
    let error = read(res.error);
    unsafe { grin_ffi_result_free(res) };
    if code == OK {
        assert!(error.is_none(), "success must not carry an error");
        Ok(data
            .map(|d| serde_json::from_str(&d).unwrap())
            .unwrap_or(Value::Null))
    } else {
        assert!(data.is_none(), "failure must not carry data");
        let e: Value = serde_json::from_str(&error.expect("error JSON")).unwrap();
        assert_eq!(
            e["code"], code,
            "error JSON code must match the result code"
        );
        assert!(e["message"].is_string());
        assert!(e.get("details").is_some());
        Err((code, e))
    }
}

/// Calls an entry point and reads its result with `read_result`. The
/// pointers these tests pass are NULL or come from a `CString` that lives
/// until the end of the statement, which is what the entry points require.
/// Entry points that take only integers are safe, hence the allow.
macro_rules! call {
    ($e:expr) => {{
        #[allow(unused_unsafe)]
        let res = unsafe { $e };
        read_result(res)
    }};
}

fn code_of<T>(r: Result<T, (i32, Value)>) -> i32 {
    match r {
        Ok(_) => OK,
        Err((c, _)) => c,
    }
}

fn cs(s: &str) -> CString {
    CString::new(s).unwrap()
}

fn path_str(p: &Path) -> String {
    p.to_str().unwrap().to_owned()
}

fn config(dir: &str, network: u8) -> CString {
    cs(&json!({
        "wallet_dir": dir,
        "network": network,
        "node_url": UNREACHABLE_NODE,
        "node_api_secret": null,
        "socks_proxy": null,
        "request_timeout_secs": 2,
    })
    .to_string())
}

fn create(dir: &str, network: u8, mnemonic: Option<&str>) -> Result<u64, (i32, Value)> {
    let m = mnemonic.map(cs);
    let v = call!(grin_wallet_create(
        config(dir, network).as_ptr(),
        m.as_ref().map_or(ptr::null(), |m| m.as_ptr()),
        cs(PASSWORD).as_ptr(),
    ))?;
    Ok(v["handle"].as_u64().unwrap())
}

fn open_with(dir: &str, network: u8, password: &str) -> Result<u64, (i32, Value)> {
    let v = call!(grin_wallet_open(
        config(dir, network).as_ptr(),
        cs(password).as_ptr(),
    ))?;
    Ok(v["handle"].as_u64().unwrap())
}

fn open(dir: &str, network: u8) -> Result<u64, (i32, Value)> {
    open_with(dir, network, PASSWORD)
}

fn close(handle: u64) {
    call!(grin_wallet_close(handle)).unwrap();
}

fn address(handle: u64, index: u32) -> String {
    let v = call!(grin_wallet_get_address(handle, index)).unwrap();
    assert_eq!(v["index"], index);
    let ov3 = v["onion_v3_url"].as_str().unwrap();
    assert!(ov3.starts_with("http://") && ov3.ends_with(".onion"));
    v["slatepack_address"].as_str().unwrap().to_owned()
}

fn delete(dir: &str, handle: u64) -> Result<Value, (i32, Value)> {
    call!(grin_wallet_delete(cs(dir).as_ptr(), handle))
}

fn progress(handle: u64) -> Value {
    call!(grin_wallet_scan_progress(handle)).unwrap()
}

/// The last scanned block the wallet saved, read through the library.
fn last_scanned_block(handle: u64) -> ScannedBlockInfo {
    let session = retain_session(handle).expect("session is registered");
    let mut w = session.wallet.lock();
    w.lc_provider()
        .unwrap()
        .wallet_inst()
        .unwrap()
        .last_scanned_block()
        .unwrap()
}

fn test_wallet(chain: &Path, dir: &Path, client_id: &str) -> u64 {
    let v = call!(grin_test_create_wallet(
        cs(&path_str(chain)).as_ptr(),
        cs(&path_str(dir)).as_ptr(),
        cs(client_id).as_ptr(),
        AUTOMATED,
        ptr::null(),
        cs(PASSWORD).as_ptr(),
    ))
    .unwrap();
    v["handle"].as_u64().unwrap()
}

#[test]
fn mnemonic_generate_and_validate() {
    for words in [12u32, 15, 18, 21, 24] {
        let v = call!(grin_mnemonic_generate(words)).unwrap();
        let m = v["mnemonic"].as_str().unwrap().to_owned();
        assert_eq!(m.split_whitespace().count() as u32, words);
        let v = call!(grin_mnemonic_validate(cs(&m).as_ptr())).unwrap();
        assert_eq!(v, json!({"valid": true, "word_count": words}));
    }
    assert_eq!(code_of(call!(grin_mnemonic_generate(13))), INVALID_ARGUMENT);
    assert_eq!(code_of(call!(grin_mnemonic_generate(0))), INVALID_ARGUMENT);

    // Right length, wrong checksum.
    let bad = "abandon ".repeat(12);
    let v = call!(grin_mnemonic_validate(cs(&bad).as_ptr())).unwrap();
    assert_eq!(v, json!({"valid": false, "word_count": 12}));
    let v = call!(grin_mnemonic_validate(cs("not a mnemonic").as_ptr())).unwrap();
    assert_eq!(v, json!({"valid": false, "word_count": 3}));
    let v = call!(grin_mnemonic_validate(cs(MNEMONIC).as_ptr())).unwrap();
    assert_eq!(v["valid"], true);
    assert_eq!(
        code_of(call!(grin_mnemonic_validate(ptr::null()))),
        INVALID_ARGUMENT
    );
}

#[test]
fn create_open_address_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = path_str(&tmp.path().join("auto"));

    let h = create(&dir, AUTOMATED, None).unwrap();
    let addr = address(h, 0);
    assert!(addr.starts_with("tgrin1"), "{addr}");
    assert_ne!(address(h, 1), addr);

    let v = call!(grin_wallet_get_mnemonic(h, cs(PASSWORD).as_ptr())).unwrap();
    assert_eq!(
        v["mnemonic"].as_str().unwrap().split_whitespace().count(),
        24
    );
    assert_eq!(
        code_of(call!(grin_wallet_get_mnemonic(h, cs("nope").as_ptr()))),
        INVALID_PASSWORD
    );
    close(h);
    assert_eq!(code_of(call!(grin_wallet_close(h))), INVALID_HANDLE);

    let h = open(&dir, AUTOMATED).unwrap();
    assert_eq!(address(h, 0), addr);

    // The address validates on its own network and is rejected on mainnet.
    let v = call!(grin_address_validate(cs(&addr).as_ptr(), AUTOMATED)).unwrap();
    assert_eq!(v["kind"], "slatepack");
    assert_eq!(v["slatepack_address"], addr.as_str());
    assert_eq!(v["url"], v["onion_v3_url"]);
    assert_eq!(
        code_of(call!(grin_address_validate(cs(&addr).as_ptr(), MAINNET))),
        INVALID_ADDRESS
    );
    assert_eq!(
        code_of(call!(grin_address_validate(cs(&addr).as_ptr(), 7))),
        INVALID_ARGUMENT
    );

    // Password change takes effect for the next open.
    call!(grin_wallet_change_password(
        h,
        cs(PASSWORD).as_ptr(),
        cs("new password").as_ptr(),
    ))
    .unwrap();
    assert_eq!(
        code_of(call!(grin_wallet_change_password(
            h,
            cs("wrong").as_ptr(),
            cs("x").as_ptr()
        ))),
        INVALID_PASSWORD
    );
    close(h);
    assert_eq!(code_of(open(&dir, AUTOMATED)), INVALID_PASSWORD);
    let h = open_with(&dir, AUTOMATED, "new password").unwrap();
    assert_eq!(address(h, 0), addr);
    close(h);

    // A fixed mnemonic gives the same key on mainnet and automated testing
    // (both derive mainnet-style keychains); only the address prefix differs.
    let main_dir = path_str(&tmp.path().join("main"));
    let h = create(&main_dir, MAINNET, Some(MNEMONIC)).unwrap();
    let main_addr = address(h, 0);
    assert!(main_addr.starts_with("grin1"), "{main_addr}");
    let main_onion = call!(grin_wallet_get_address(h, 0)).unwrap()["onion_v3_url"].clone();
    let v = call!(grin_wallet_get_mnemonic(h, cs(PASSWORD).as_ptr())).unwrap();
    assert_eq!(v["mnemonic"], MNEMONIC);
    close(h);
    let auto_dir = path_str(&tmp.path().join("auto2"));
    let h = create(&auto_dir, AUTOMATED, Some(MNEMONIC)).unwrap();
    assert!(address(h, 0).starts_with("tgrin1"));
    let auto_onion = call!(grin_wallet_get_address(h, 0)).unwrap()["onion_v3_url"].clone();
    close(h);
    assert_eq!(auto_onion, main_onion);

    // A bad mnemonic is rejected before anything is written.
    let bad_dir = tmp.path().join("bad");
    let e = create(&path_str(&bad_dir), AUTOMATED, Some(&"abandon ".repeat(12))).unwrap_err();
    assert_eq!(e.0, INVALID_MNEMONIC);
    assert!(!bad_dir.join("wallet_data").exists());

    // Missing wallets.
    let none = path_str(&tmp.path().join("none"));
    assert_eq!(code_of(open(&none, AUTOMATED)), WALLET_NOT_FOUND);
    std::fs::create_dir_all(tmp.path().join("none").join("wallet_data")).unwrap();
    assert_eq!(code_of(open(&none, AUTOMATED)), WALLET_NOT_FOUND);
}

#[test]
fn funded_wallet_on_test_chain() {
    let tmp = tempfile::tempdir().unwrap();
    let chain = tmp.path().join("chain");
    let alice = test_wallet(&chain, &tmp.path().join("alice"), "alice");
    let bob = test_wallet(&chain, &tmp.path().join("bob"), "bob");
    assert!(address(alice, 0).starts_with("tgrin1"));

    let v = call!(grin_test_mine_blocks(alice, 5)).unwrap();
    assert_eq!(v["height"], 5);
    let v = call!(grin_wallet_get_chain_height(alice)).unwrap();
    assert_eq!(v["height"], 5);
    assert!(v["hash"].as_str().unwrap().len() == 64);

    let b = call!(grin_wallet_get_balance(alice, true, 1)).unwrap();
    assert_eq!(b["refreshed_from_node"], true);
    assert_eq!(b["last_confirmed_height"], 5);
    let spendable = b["amount_currently_spendable"].as_u64().unwrap();
    let immature = b["amount_immature"].as_u64().unwrap();
    assert!(spendable > 0, "{b}");
    assert!(immature > 0, "{b}");
    assert_eq!(b["total"].as_u64().unwrap(), spendable + immature);

    let b = call!(grin_wallet_get_balance(bob, true, 1)).unwrap();
    assert_eq!(b["total"], 0);

    let v = call!(grin_wallet_scan(alice, 0, false)).unwrap();
    assert_eq!(v, json!({"scanned_height": 5, "tip_height": 5}));
    let p = call!(grin_wallet_scan_progress(alice)).unwrap();
    assert_eq!(p["is_scanning"], false);
    assert_eq!(p["cancel_requested"], false);
    assert_eq!(p["start_height"], 1);
    assert_eq!(p["tip_height"], 5);
    assert_eq!(p["progress_pct"], 100);

    let after = call!(grin_wallet_get_balance(alice, false, 1)).unwrap();
    assert_eq!(after["amount_currently_spendable"], spendable);

    // A cancel with no scan running does nothing and cannot stop the next
    // scan (scan_cancels_between_batches covers a running one).
    call!(grin_wallet_scan_cancel(alice)).unwrap();
    let p = progress(alice);
    assert_eq!(p["cancel_requested"], false);
    assert_eq!(p["is_scanning"], false);
    let v = call!(grin_wallet_scan(alice, 3, false)).unwrap();
    assert_eq!(v, json!({"scanned_height": 5, "tip_height": 5}));
    assert_eq!(progress(alice)["start_height"], 3);

    // History holds only the coinbase outputs so far.
    let txs = call!(grin_txs_get(alice, false)).unwrap();
    let txs = txs["txs"].as_array().unwrap();
    assert!(!txs.is_empty());
    assert!(txs.iter().all(|t| t["type"] == "ConfirmedCoinbase"));
    let est = call!(grin_tx_estimate_fee(alice, 1_000_000_000, 1, false, false)).unwrap();
    assert!(est["fee"].as_u64().unwrap() > 0, "{est}");

    close(alice);
    close(bob);

    // Reopening a test wallet on the same chain keeps its funds.
    let alice = test_wallet(&chain, &tmp.path().join("alice"), "alice");
    let b = call!(grin_wallet_get_balance(alice, true, 1)).unwrap();
    assert_eq!(b["amount_currently_spendable"], spendable);
    close(alice);
}

/// A symlink to `target` (made before `target` exists), or None where
/// symlinks need privileges.
#[cfg(unix)]
fn dir_link(tmp: &Path, target: &Path) -> Option<String> {
    let link = tmp.join("link");
    std::os::unix::fs::symlink(target, &link).unwrap();
    Some(path_str(&link))
}

#[cfg(not(unix))]
fn dir_link(_tmp: &Path, _target: &Path) -> Option<String> {
    None
}

fn wallet_busy_case(use_finalizer: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("w");
    let dir = path_str(&real);
    let link = dir_link(tmp.path(), &real);
    let dotted = format!("{dir}/./");
    assert!(!real.exists(), "directory must not exist yet");
    let mut aliases = vec![dir.clone(), dotted.clone()];
    aliases.extend(link.clone());

    let h = create(&dir, AUTOMATED, None).unwrap();
    let addr = address(h, 0);
    let canonical = std::fs::canonicalize(real.join("wallet_data")).unwrap();

    // Stand-in for an in-flight call that still holds the session.
    let retained = retain_session(h).expect("session is registered");
    if use_finalizer {
        unsafe { grin_wallet_close_finalizer(h as usize as *mut c_void) };
    } else {
        close(h);
    }
    assert_eq!(
        code_of(call!(grin_wallet_get_address(h, 0))),
        INVALID_HANDLE
    );

    for d in &aliases {
        let e = open(d, AUTOMATED).unwrap_err();
        assert_eq!(e.0, WALLET_BUSY, "open {d}");
        assert_eq!(
            e.1["details"]["wallet_dir"],
            canonical.to_string_lossy().as_ref()
        );
        assert_eq!(
            code_of(create(d, AUTOMATED, None)),
            WALLET_BUSY,
            "create {d}"
        );
        assert_eq!(code_of(delete(d, 0)), WALLET_BUSY, "delete {d}");
    }
    assert!(canonical.join("wallet.seed").is_file());

    // The last Arc closes the backend and releases the directory.
    drop(retained);
    let h = open(&dir, AUTOMATED).unwrap();
    assert_eq!(address(h, 0), addr);
    for d in &aliases {
        assert_eq!(code_of(open(d, AUTOMATED)), WALLET_BUSY, "open {d}");
    }
    close(h);

    // Failures after the lease is taken release it again.
    assert_eq!(
        code_of(open_with(&dir, AUTOMATED, "wrong")),
        INVALID_PASSWORD
    );
    assert_eq!(code_of(create(&dir, AUTOMATED, None)), WALLET_EXISTS);
    let h = open(&dotted, AUTOMATED).unwrap();
    assert_eq!(address(h, 0), addr);

    // A handle for another directory is rejected; this one is closed and
    // the data removed.
    let other = path_str(&tmp.path().join("other"));
    let other_h = create(&other, AUTOMATED, None).unwrap();
    assert_eq!(code_of(delete(&dir, other_h)), INVALID_ARGUMENT);
    close(other_h);
    delete(aliases.last().unwrap(), h).unwrap();
    assert_eq!(
        code_of(call!(grin_wallet_get_address(h, 0))),
        INVALID_HANDLE
    );
    assert!(!canonical.exists());
    assert!(real.exists(), "only wallet_data is removed");
    delete(&dir, 0).unwrap();
    assert_eq!(code_of(open(&dir, AUTOMATED)), WALLET_NOT_FOUND);

    // An unknown handle is ignored by the finalizer.
    unsafe { grin_wallet_close_finalizer(987_654_321usize as *mut c_void) };
    unsafe { grin_wallet_close_finalizer(ptr::null_mut()) };
}

#[test]
fn wallet_busy_after_close() {
    wallet_busy_case(false);
}

#[test]
fn wallet_busy_after_close_finalizer() {
    wallet_busy_case(true);
}

#[test]
fn mainnet_and_testnet_concurrently() {
    let tmp = tempfile::tempdir().unwrap();
    let main = create(&path_str(&tmp.path().join("main")), MAINNET, None).unwrap();
    let test = create(&path_str(&tmp.path().join("test")), TESTNET, None).unwrap();
    let main_addr = Arc::new(address(main, 0));
    let test_addr = Arc::new(address(test, 0));
    assert!(main_addr.starts_with("grin1"));
    assert!(test_addr.starts_with("tgrin1"));

    let workers: Vec<_> = (0..4)
        .map(|worker| {
            let main_addr = main_addr.clone();
            let test_addr = test_addr.clone();
            thread::spawn(move || {
                for i in 0..200 {
                    // Alternate the order so threads interleave both wallets.
                    let (first, second) = if (i + worker) % 2 == 0 {
                        (main, test)
                    } else {
                        (test, main)
                    };
                    for h in [first, second] {
                        let a = address(h, 0);
                        if h == main {
                            assert!(a.starts_with("grin1"), "mainnet gave {a}");
                            assert_eq!(&a, main_addr.as_str());
                        } else {
                            assert!(a.starts_with("tgrin1"), "testnet gave {a}");
                            assert_eq!(&a, test_addr.as_str());
                        }
                    }
                    // Stateless calls on the same threads must not leak a
                    // chain type into the wallet calls above.
                    let net = if i % 2 == 0 { MAINNET } else { TESTNET };
                    let probe = if net == MAINNET {
                        &main_addr
                    } else {
                        &test_addr
                    };
                    call!(grin_address_validate(cs(probe).as_ptr(), net)).unwrap();
                }
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
    close(main);
    close(test);
}

#[test]
fn network_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = path_str(&tmp.path().join("w"));
    let h = create(&dir, AUTOMATED, None).unwrap();

    assert_eq!(code_of(call!(grin_wallet_get_chain_height(h))), NODE);
    call!(grin_wallet_update_network(
        h,
        cs("http://127.0.0.1:2").as_ptr(),
        cs("secret").as_ptr(),
        cs("127.0.0.1:9050").as_ptr(),
    ))
    .unwrap();
    assert_eq!(
        code_of(call!(grin_wallet_update_network(
            h,
            cs(UNREACHABLE_NODE).as_ptr(),
            ptr::null(),
            cs("localhost:9050").as_ptr(),
        ))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        code_of(call!(grin_wallet_update_network(
            h,
            cs("ftp://x").as_ptr(),
            ptr::null(),
            ptr::null(),
        ))),
        INVALID_ARGUMENT
    );
    // Credentials in the URL would reach error text and logs; the secret
    // goes in its own argument.
    let with_password = "http://grin:hunter2@127.0.0.1:2";
    let e = call!(grin_wallet_update_network(
        h,
        cs(with_password).as_ptr(),
        ptr::null(),
        ptr::null(),
    ))
    .unwrap_err();
    assert_eq!(e.0, INVALID_ARGUMENT, "{}", e.1);
    assert!(!e.1.to_string().contains("hunter2"), "{}", e.1);
    call!(grin_wallet_update_network(
        h,
        cs(UNREACHABLE_NODE).as_ptr(),
        ptr::null(),
        cs("").as_ptr(),
    ))
    .unwrap();
    assert_eq!(code_of(call!(grin_wallet_get_chain_height(h))), NODE);
    // An unreachable node is not an error for balances, only not a refresh.
    let b = call!(grin_wallet_get_balance(h, true, 10)).unwrap();
    assert_eq!(b["refreshed_from_node"], false);
    assert_eq!(b["minimum_confirmations"], 10);
    assert_eq!(b["total"], 0);
    close(h);

    assert_eq!(
        code_of(call!(grin_node_test_connection(
            cs(UNREACHABLE_NODE).as_ptr(),
            ptr::null(),
            ptr::null(),
        ))),
        NODE
    );
    assert_eq!(
        code_of(call!(grin_node_test_connection(
            ptr::null(),
            ptr::null(),
            ptr::null()
        ))),
        INVALID_ARGUMENT
    );
    let e = call!(grin_node_test_connection(
        cs(with_password).as_ptr(),
        ptr::null(),
        ptr::null(),
    ))
    .unwrap_err();
    assert_eq!(e.0, INVALID_ARGUMENT, "{}", e.1);
    assert!(!e.1.to_string().contains("hunter2"), "{}", e.1);
    let config = json!({"wallet_dir": dir, "network": AUTOMATED, "node_url": with_password});
    let e = call!(grin_wallet_open(
        cs(&config.to_string()).as_ptr(),
        cs(PASSWORD).as_ptr(),
    ))
    .unwrap_err();
    assert_eq!(e.0, INVALID_ARGUMENT, "{}", e.1);
    assert!(!e.1.to_string().contains("hunter2"), "{}", e.1);
}

#[test]
fn scan_cancels_between_batches() {
    let tmp = tempfile::tempdir().unwrap();
    let chain = tmp.path().join("chain");
    let dir = tmp.path().join("w");
    let w = test_wallet(&chain, &dir, "w");
    call!(grin_test_mine_blocks(w, 10)).unwrap();
    let spendable =
        call!(grin_wallet_get_balance(w, true, 1)).unwrap()["amount_currently_spendable"].clone();
    assert!(spendable.as_u64().unwrap() > 0);

    let v = call!(grin_wallet_scan(w, 0, false)).unwrap();
    assert_eq!(v, json!({"scanned_height": 10, "tip_height": 10}));
    assert_eq!(progress(w)["progress_pct"], 100);
    assert_eq!(last_scanned_block(w).height, 10);

    // Two more blocks the wallet has not seen; the refresh at the start of
    // a scan records the new tip as the last confirmed height.
    call!(grin_test_mine_blocks(w, 2)).unwrap();
    let tip_hash = call!(grin_wallet_get_chain_height(w)).unwrap()["hash"].clone();

    // Two heights per batch: 1-2, 3-4, 5-6, ... The hook runs after each
    // batch is saved and cancels after the one ending at 4.
    let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
    let log = seen.clone();
    let hooked = set_scan_test_hooks(
        w,
        2,
        Some(Box::new(move |height| {
            let mut p = progress(w);
            p["batch_end"] = json!(height);
            if height == 4 {
                call!(grin_wallet_scan_cancel(w)).unwrap();
                p["after_cancel"] = progress(w)["cancel_requested"].clone();
            }
            log.lock().unwrap().push(p);
        })),
    );
    assert!(hooked);
    let e = call!(grin_wallet_scan(w, 0, false)).unwrap_err();
    assert_eq!(e.0, CANCELLED, "{}", e.1);
    assert_eq!(
        e.1["details"],
        json!({"scanned_height": 4, "tip_height": 12})
    );
    let seen = std::mem::take(&mut *seen.lock().unwrap());
    assert_eq!(seen.len(), 2, "{seen:?}");
    // The heights were reset for this scan: the previous one ended at 100%.
    let first = &seen[0];
    assert_eq!(first["batch_end"], 2);
    assert_eq!(first["is_scanning"], true);
    assert_eq!(first["cancel_requested"], false);
    assert_eq!(first["start_height"], 1);
    assert_eq!(first["tip_height"], 12);
    assert!(first["scanned_height"].as_u64().unwrap() < 12, "{first}");
    assert!(first["progress_pct"].as_u64().unwrap() < 100, "{first}");
    assert_eq!(seen[1]["after_cancel"], true);
    // The scan stopped before the batch starting at 5, and what it saved
    // matches what it reported.
    let p = progress(w);
    assert_eq!(p["is_scanning"], false);
    assert_eq!(p["cancel_requested"], false);
    assert_eq!(p["scanned_height"], 4);
    assert_eq!(p["tip_height"], 12);
    assert_eq!(last_scanned_block(w).height, 4);
    let b = call!(grin_wallet_get_balance(w, false, 1)).unwrap();
    assert_eq!(b["last_confirmed_height"], 12);

    // A cancel that arrives after the last batch has nothing to stop, and
    // the flag it leaves does not fail the next scan.
    set_scan_test_hooks(
        w,
        5,
        Some(Box::new(move |height| {
            if height == 12 {
                call!(grin_wallet_scan_cancel(w)).unwrap();
            }
        })),
    );
    let v = call!(grin_wallet_scan(w, 0, false)).unwrap();
    assert_eq!(v, json!({"scanned_height": 12, "tip_height": 12}));
    set_scan_test_hooks(w, 0, None);
    let v = call!(grin_wallet_scan(w, 0, false)).unwrap();
    assert_eq!(v, json!({"scanned_height": 12, "tip_height": 12}));
    let saved = last_scanned_block(w);
    assert_eq!(saved.height, 12);
    assert_eq!(json!(saved.hash), tip_hash);
    let b = call!(grin_wallet_get_balance(w, true, 1)).unwrap();
    assert!(b["amount_currently_spendable"].as_u64().unwrap() >= spendable.as_u64().unwrap());

    // Closing the handle stops a running scan too, so close does not wait
    // for the whole scan, and the directory is free once the scan returns.
    set_scan_test_hooks(
        w,
        2,
        Some(Box::new(move |height| {
            if height == 2 {
                call!(grin_wallet_close(w)).unwrap();
            }
        })),
    );
    let e = call!(grin_wallet_scan(w, 0, false)).unwrap_err();
    assert_eq!(e.0, CANCELLED, "{}", e.1);
    assert_eq!(e.1["details"]["scanned_height"], 2);
    assert_eq!(code_of(call!(grin_wallet_scan_progress(w))), INVALID_HANDLE);
    let w = test_wallet(&chain, &dir, "w");
    let v = call!(grin_wallet_scan(w, 0, false)).unwrap();
    assert_eq!(v, json!({"scanned_height": 12, "tip_height": 12}));
    close(w);
}

#[test]
fn second_scan_is_busy_while_one_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("w");
    let w = test_wallet(&tmp.path().join("chain"), &dir, "w");
    call!(grin_test_mine_blocks(w, 4)).unwrap();
    let data_dir = std::fs::canonicalize(dir.join("wallet_data")).unwrap();

    // After each batch of the running scan, another thread starts a scan on
    // the same wallet. It is refused before it makes a node call.
    let refused: Arc<Mutex<Vec<(i32, Value)>>> = Arc::default();
    let log = refused.clone();
    let hooked = set_scan_test_hooks(
        w,
        2,
        Some(Box::new(move |_height| {
            let second = thread::spawn(move || call!(grin_wallet_scan(w, 0, false)))
                .join()
                .unwrap();
            log.lock().unwrap().push(second.unwrap_err());
        })),
    );
    assert!(hooked);
    let v = call!(grin_wallet_scan(w, 0, false)).unwrap();
    assert_eq!(v, json!({"scanned_height": 4, "tip_height": 4}));
    let refused = std::mem::take(&mut *refused.lock().unwrap());
    assert_eq!(refused.len(), 2, "{refused:?}");
    for (code, e) in &refused {
        assert_eq!(*code, WALLET_BUSY, "{e}");
        assert_eq!(e["details"]["reason"], "scan_in_progress", "{e}");
        assert_eq!(
            e["details"]["wallet_dir"],
            data_dir.to_string_lossy().as_ref()
        );
    }

    // The refused scans left the slot alone: the next scan runs.
    set_scan_test_hooks(w, 0, None);
    assert!(!progress(w)["is_scanning"].as_bool().unwrap());
    let v = call!(grin_wallet_scan(w, 0, false)).unwrap();
    assert_eq!(v, json!({"scanned_height": 4, "tip_height": 4}));
    close(w);
}

#[test]
fn password_errors_are_precise() {
    let tmp = tempfile::tempdir().unwrap();
    // A path that mentions a password must not turn other errors into
    // InvalidPassword.
    let wallet = tmp.path().join("password-store");
    let dir = path_str(&wallet);
    let data = wallet.join("wallet_data");
    let seed = data.join("wallet.seed");
    let h = create(&dir, AUTOMATED, None).unwrap();
    let original = std::fs::read(&seed).unwrap();
    let mnemonic = |h: u64, pw: &str| call!(grin_wallet_get_mnemonic(h, cs(pw).as_ptr()));
    let change = |h: u64, old: &str, new: &str| {
        call!(grin_wallet_change_password(
            h,
            cs(old).as_ptr(),
            cs(new).as_ptr(),
        ))
    };

    assert_eq!(code_of(mnemonic(h, "nope")), INVALID_PASSWORD);
    assert_eq!(code_of(change(h, "nope", "x")), INVALID_PASSWORD);
    assert_eq!(std::fs::read(&seed).unwrap(), original);

    // A damaged or missing seed file is not a wrong password.
    std::fs::write(&seed, b"garbage").unwrap();
    let e = mnemonic(h, PASSWORD).unwrap_err();
    assert_eq!(e.0, LIFECYCLE, "{}", e.1);
    assert!(
        e.1["message"].as_str().unwrap().contains("damaged"),
        "{}",
        e.1
    );
    assert_eq!(code_of(change(h, PASSWORD, "x")), LIFECYCLE);
    // Shape-valid but corrupt: three strings that are not hex, and valid hex
    // with a nonce too short for the provider, which would otherwise index
    // past it and panic.
    for bad in [
        r#"{"encrypted_seed":"zz","salt":"zz","nonce":"zz"}"#,
        r#"{"encrypted_seed":"00112233445566778899aabbccddeeff0011223344556677","salt":"0011223344556677","nonce":"00"}"#,
    ] {
        std::fs::write(&seed, bad).unwrap();
        let e = mnemonic(h, PASSWORD).unwrap_err();
        assert_eq!(e.0, LIFECYCLE, "{}", e.1);
        assert!(
            e.1["message"].as_str().unwrap().contains("damaged"),
            "{}",
            e.1
        );
        assert_eq!(code_of(change(h, PASSWORD, "x")), LIFECYCLE);
    }
    std::fs::remove_file(&seed).unwrap();
    assert_eq!(code_of(mnemonic(h, PASSWORD)), WALLET_NOT_FOUND);
    assert_eq!(code_of(change(h, PASSWORD, "x")), WALLET_NOT_FOUND);
    std::fs::write(&seed, &original).unwrap();

    // With the old password verified, a later failure is Lifecycle with the
    // provider's own message. A read-only wallet_data makes writing the new
    // seed file fail, and the seed stays as it was.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let set_mode =
            |m: u32| std::fs::set_permissions(&data, std::fs::Permissions::from_mode(m)).unwrap();
        set_mode(0o555);
        let probe = data.join("probe");
        if std::fs::write(&probe, b"").is_ok() {
            // Running as root: permissions do not stop the rename.
            std::fs::remove_file(&probe).unwrap();
        } else {
            let e = change(h, PASSWORD, "new password");
            set_mode(0o755);
            let e = e.unwrap_err();
            assert_eq!(e.0, LIFECYCLE, "{}", e.1);
            let message = e.1["message"].as_str().unwrap();
            assert!(message.contains("password change failed"), "{message}");
            assert!(message.contains("writing new wallet seed"), "{message}");
            assert_eq!(e.1["details"]["seed_present"], true);
            assert_eq!(std::fs::read(&seed).unwrap(), original);
        }
        set_mode(0o755);
    }
    assert_eq!(
        mnemonic(h, PASSWORD).unwrap()["mnemonic"]
            .as_str()
            .unwrap()
            .split_whitespace()
            .count(),
        24
    );
    close(h);

    assert_eq!(
        code_of(open_with(&dir, AUTOMATED, "wrong")),
        INVALID_PASSWORD
    );
    std::fs::write(&seed, b"garbage").unwrap();
    assert_eq!(code_of(open(&dir, AUTOMATED)), LIFECYCLE);
    std::fs::write(
        &seed,
        r#"{"encrypted_seed":"00112233445566778899aabbccddeeff0011223344556677","salt":"0011223344556677","nonce":"00"}"#,
    )
    .unwrap();
    assert_eq!(code_of(open(&dir, AUTOMATED)), LIFECYCLE);
    std::fs::write(&seed, &original).unwrap();
    close(open(&dir, AUTOMATED).unwrap());
}

#[test]
fn create_refuses_leftover_wallet_data() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = path_str(&tmp.path().join("w"));
    let data = tmp.path().join("w").join("wallet_data");
    close(create(&dir, AUTOMATED, None).unwrap());
    // A delete that stopped half way, or a seed removed by hand.
    std::fs::remove_file(data.join("wallet.seed")).unwrap();

    let e = create(&dir, AUTOMATED, Some(MNEMONIC)).unwrap_err();
    assert_eq!(e.0, WALLET_EXISTS, "{}", e.1);
    assert!(
        e.1["message"]
            .as_str()
            .unwrap()
            .contains("leftover wallet data"),
        "{}",
        e.1
    );
    assert_eq!(e.1["details"]["reason"], "leftover_wallet_data");
    assert_eq!(e.1["details"]["path"], "db");
    assert!(data.join("db").is_dir(), "nothing is removed");
    assert!(!data.join("wallet.seed").exists());
    assert_eq!(code_of(open(&dir, AUTOMATED)), WALLET_NOT_FOUND);

    delete(&dir, 0).unwrap();
    close(create(&dir, AUTOMATED, Some(MNEMONIC)).unwrap());
}

/// Removing files LMDB has open only works on unix.
#[cfg(unix)]
#[test]
fn delete_closes_a_handle_whose_data_is_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = path_str(&tmp.path().join("w"));
    let h = create(&dir, AUTOMATED, None).unwrap();
    std::fs::remove_dir_all(tmp.path().join("w").join("wallet_data")).unwrap();
    delete(&dir, h).unwrap();
    assert_eq!(
        code_of(call!(grin_wallet_get_address(h, 0))),
        INVALID_HANDLE
    );
    close(create(&dir, AUTOMATED, None).unwrap());

    // A handle whose data still exists is not closed by deleting some other,
    // missing directory.
    let other = create(&path_str(&tmp.path().join("other")), AUTOMATED, None).unwrap();
    let missing = path_str(&tmp.path().join("missing"));
    assert_eq!(code_of(delete(&missing, other)), INVALID_ARGUMENT);
    address(other, 0);
    close(other);
}

/// A `wallet_data` that links to a directory with another name is refused
/// by create and open before they read or write through it.
#[cfg(unix)]
#[test]
fn wallet_data_link_to_another_name_is_refused() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let link_to = |top: &str, target: &Path| {
        let top = tmp.path().join(top);
        std::fs::create_dir(&top).unwrap();
        symlink(target, top.join("wallet_data")).unwrap();
        path_str(&top)
    };

    // A real wallet's data, moved to a directory named "moved". The link is
    // refused as a link before its target's name is looked at.
    let real = tmp.path().join("real");
    close(create(&path_str(&real), AUTOMATED, None).unwrap());
    let moved = tmp.path().join("moved");
    std::fs::rename(real.join("wallet_data"), &moved).unwrap();
    let dir = link_to("w", &moved);
    for e in [
        open(&dir, AUTOMATED).unwrap_err(),
        create(&dir, AUTOMATED, Some(MNEMONIC)).unwrap_err(),
    ] {
        assert_eq!(e.0, INVALID_ARGUMENT, "{}", e.1);
        assert!(
            e.1["message"].as_str().unwrap().contains("symbolic link"),
            "{}",
            e.1
        );
    }
    assert!(moved.join("wallet.seed").is_file());
    // The name rule itself still holds for a real directory reached through
    // a link higher up: canonical names other than wallet_data are refused.
    assert_eq!(
        code_of(call!(grin_wallet_open(
            config(&path_str(tmp.path()), AUTOMATED).as_ptr(),
            cs(PASSWORD).as_ptr(),
        ))),
        WALLET_NOT_FOUND
    );

    // An empty target: create would otherwise make a wallet in it.
    let empty = tmp.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let dir = link_to("w2", &empty);
    assert_eq!(code_of(create(&dir, AUTOMATED, None)), INVALID_ARGUMENT);
    assert_eq!(code_of(open(&dir, AUTOMATED)), INVALID_ARGUMENT);
    assert_eq!(std::fs::read_dir(&empty).unwrap().count(), 0);

    // A link to a directory that is itself named wallet_data is refused as
    // well: through such a link a delete would remove another wallet.
    let named = tmp.path().join("elsewhere").join("wallet_data");
    std::fs::create_dir(tmp.path().join("elsewhere")).unwrap();
    std::fs::rename(&moved, &named).unwrap();
    let dir = link_to("w3", &named);
    for e in [
        open(&dir, AUTOMATED).unwrap_err(),
        create(&dir, AUTOMATED, None).unwrap_err(),
        delete(&dir, 0).unwrap_err(),
    ] {
        assert_eq!(e.0, INVALID_ARGUMENT, "{}", e.1);
        assert!(
            e.1["message"].as_str().unwrap().contains("symbolic link"),
            "{}",
            e.1
        );
    }
    assert!(named.join("wallet.seed").is_file());
    // The directory it points at is still a wallet, through its own path.
    close(open(&path_str(&tmp.path().join("elsewhere")), AUTOMATED).unwrap());
    // A link higher up the path is not the data directory itself.
    let via_parent = tmp.path().join("via_parent");
    symlink(tmp.path().join("elsewhere"), &via_parent).unwrap();
    close(open(&path_str(&via_parent), AUTOMATED).unwrap());
}

#[test]
fn delete_with_a_held_handle_keeps_the_handle() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = path_str(&tmp.path().join("w"));
    let h = create(&dir, AUTOMATED, None).unwrap();
    let addr = address(h, 0);

    // Another call still holds the session: nothing is deleted and the
    // handle stays usable.
    let retained = retain_session(h).expect("session is registered");
    let e = delete(&dir, h).unwrap_err();
    assert_eq!(e.0, WALLET_BUSY, "{}", e.1);
    assert_eq!(address(h, 0), addr);
    assert!(tmp
        .path()
        .join("w")
        .join("wallet_data")
        .join("wallet.seed")
        .is_file());

    // Once the call has returned, the same delete closes the handle and
    // removes the data.
    drop(retained);
    delete(&dir, h).unwrap();
    assert_eq!(
        code_of(call!(grin_wallet_get_address(h, 0))),
        INVALID_HANDLE
    );
    assert!(!tmp.path().join("w").join("wallet_data").exists());
}

#[test]
fn error_paths() {
    call!(grin_init_logs(cs("error").as_ptr())).unwrap();
    call!(grin_init_logs(ptr::null())).unwrap();
    call!(grin_init_logs(cs("error").as_ptr())).unwrap();
    assert_eq!(
        code_of(call!(grin_init_logs(cs("chatty").as_ptr()))),
        INVALID_ARGUMENT
    );

    let e = call!(grin_test_panic()).unwrap_err();
    assert_eq!(e.0, INTERNAL_PANIC);
    assert_eq!(e.1["message"], "grin_test_panic");

    for h in [0u64, 12345] {
        assert_eq!(
            code_of(call!(grin_wallet_get_address(h, 0))),
            INVALID_HANDLE
        );
        assert_eq!(code_of(call!(grin_wallet_scan_progress(h))), INVALID_HANDLE);
        assert_eq!(code_of(call!(grin_wallet_scan_cancel(h))), INVALID_HANDLE);
        assert_eq!(
            code_of(call!(grin_wallet_get_balance(h, false, 1))),
            INVALID_HANDLE
        );
        assert_eq!(code_of(call!(grin_wallet_close(h))), INVALID_HANDLE);
        assert_eq!(code_of(call!(grin_test_mine_blocks(h, 1))), INVALID_HANDLE);
    }

    let tmp = tempfile::tempdir().unwrap();
    let dir = path_str(&tmp.path().join("w"));
    let pw = cs(PASSWORD);

    // NULL pointers.
    assert_eq!(
        code_of(call!(grin_wallet_create(
            ptr::null(),
            ptr::null(),
            pw.as_ptr()
        ))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        code_of(call!(grin_wallet_create(
            config(&dir, AUTOMATED).as_ptr(),
            ptr::null(),
            ptr::null()
        ))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        code_of(call!(grin_wallet_open(
            config(&dir, AUTOMATED).as_ptr(),
            ptr::null()
        ))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        code_of(call!(grin_wallet_delete(ptr::null(), 0))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        code_of(call!(grin_address_validate(ptr::null(), MAINNET))),
        INVALID_ARGUMENT
    );

    // Invalid UTF-8.
    let bad = CString::new(vec![b'{', 0xff, 0xfe, b'}']).unwrap();
    assert_eq!(
        code_of(call!(grin_wallet_open(bad.as_ptr(), pw.as_ptr()))),
        INVALID_UTF8
    );
    assert_eq!(
        code_of(call!(grin_mnemonic_validate(bad.as_ptr()))),
        INVALID_UTF8
    );
    assert_eq!(
        code_of(call!(grin_wallet_create(
            config(&dir, AUTOMATED).as_ptr(),
            ptr::null(),
            bad.as_ptr()
        ))),
        INVALID_UTF8
    );

    // Malformed or invalid config.
    assert_eq!(
        code_of(call!(grin_wallet_open(
            cs("{not json").as_ptr(),
            pw.as_ptr()
        ))),
        JSON
    );
    assert_eq!(
        code_of(call!(grin_wallet_open(
            cs("{\"network\": 0}").as_ptr(),
            pw.as_ptr()
        ))),
        JSON
    );
    let bad_net = json!({"wallet_dir": dir, "network": 9, "node_url": UNREACHABLE_NODE});
    assert_eq!(
        code_of(call!(grin_wallet_create(
            cs(&bad_net.to_string()).as_ptr(),
            ptr::null(),
            pw.as_ptr()
        ))),
        INVALID_ARGUMENT
    );
    let bad_proxy = json!({
        "wallet_dir": dir, "network": 0, "node_url": UNREACHABLE_NODE,
        "socks_proxy": "tor.example:9050",
    });
    assert_eq!(
        code_of(call!(grin_wallet_create(
            cs(&bad_proxy.to_string()).as_ptr(),
            ptr::null(),
            pw.as_ptr()
        ))),
        INVALID_ARGUMENT
    );
    assert!(
        !PathBuf::from(&dir).exists(),
        "config errors create nothing"
    );

    // Address forms.
    let v = call!(grin_address_validate(
        cs("https://wallet.example.com/v2/foreign").as_ptr(),
        MAINNET,
    ))
    .unwrap();
    assert_eq!(
        v,
        json!({
            "kind": "http",
            "slatepack_address": null,
            "onion_v3_url": null,
            "url": "https://wallet.example.com",
        })
    );
    assert_eq!(
        code_of(call!(grin_address_validate(
            cs("grin1nope").as_ptr(),
            MAINNET
        ))),
        INVALID_ADDRESS
    );
}

#[test]
fn lone_seed_backup_is_reported_not_treated_as_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = path_str(&tmp.path().join("w"));
    let h = create(&dir, AUTOMATED, None).unwrap();
    let addr = address(h, 0);
    close(h);

    // The state an interrupted password change or recovery can leave.
    let data_dir = tmp.path().join("w").join("wallet_data");
    std::fs::rename(
        data_dir.join("wallet.seed"),
        data_dir.join("wallet.seed.bak"),
    )
    .unwrap();

    for (what, r) in [
        ("open", open(&dir, AUTOMATED)),
        ("create", create(&dir, AUTOMATED, None)),
    ] {
        let (code, e) = r.unwrap_err();
        assert_eq!(code, LIFECYCLE, "{what}: {e}");
        assert_eq!(e["details"]["reason"], "seed_backup_only", "{what}: {e}");
        assert_eq!(
            e["details"]["backup_files"],
            json!(["wallet.seed.bak"]),
            "{what}: {e}"
        );
    }
    // Neither call removed or replaced anything.
    assert!(data_dir.join("wallet.seed.bak").is_file());
    assert!(!data_dir.join("wallet.seed").exists());

    std::fs::rename(
        data_dir.join("wallet.seed.bak"),
        data_dir.join("wallet.seed"),
    )
    .unwrap();
    let h = open(&dir, AUTOMATED).unwrap();
    assert_eq!(address(h, 0), addr);
    close(h);
}
