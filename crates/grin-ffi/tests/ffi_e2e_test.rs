//! End-to-end tests that drive the C ABI the way a C or Dart caller would:
//! CString arguments in, JSON out, every result freed.

#![cfg(feature = "test-utils")]

use std::ffi::{c_void, CStr, CString};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Arc;
use std::thread;

use grin_ffi::test_support::retain_session;
use grin_ffi::*;
use serde_json::{json, Value};

const OK: i32 = 0;
const INVALID_ARGUMENT: i32 = GrinErrorCode::InvalidArgument as i32;
const INVALID_UTF8: i32 = GrinErrorCode::InvalidUtf8 as i32;
const JSON: i32 = GrinErrorCode::Json as i32;
const WALLET_NOT_FOUND: i32 = GrinErrorCode::WalletNotFound as i32;
const INVALID_PASSWORD: i32 = GrinErrorCode::InvalidPassword as i32;
const WALLET_EXISTS: i32 = GrinErrorCode::WalletExists as i32;
const INVALID_HANDLE: i32 = GrinErrorCode::InvalidHandle as i32;
const NODE: i32 = GrinErrorCode::Node as i32;
const CANCELLED: i32 = GrinErrorCode::Cancelled as i32;
const INTERNAL_PANIC: i32 = GrinErrorCode::InternalPanic as i32;
const WALLET_BUSY: i32 = GrinErrorCode::WalletBusy as i32;
const NOT_SUPPORTED: i32 = GrinErrorCode::NotSupported as i32;
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
fn call(res: GrinFfiResult) -> Result<Value, (i32, Value)> {
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
    grin_ffi_result_free(res);
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
    let v = call(grin_wallet_create(
        config(dir, network).as_ptr(),
        m.as_ref().map_or(ptr::null(), |m| m.as_ptr()),
        cs(PASSWORD).as_ptr(),
    ))?;
    Ok(v["handle"].as_u64().unwrap())
}

fn open_with(dir: &str, network: u8, password: &str) -> Result<u64, (i32, Value)> {
    let v = call(grin_wallet_open(
        config(dir, network).as_ptr(),
        cs(password).as_ptr(),
    ))?;
    Ok(v["handle"].as_u64().unwrap())
}

fn open(dir: &str, network: u8) -> Result<u64, (i32, Value)> {
    open_with(dir, network, PASSWORD)
}

fn close(handle: u64) {
    call(grin_wallet_close(handle)).unwrap();
}

fn address(handle: u64, index: u32) -> String {
    let v = call(grin_wallet_get_address(handle, index)).unwrap();
    assert_eq!(v["index"], index);
    let ov3 = v["onion_v3_url"].as_str().unwrap();
    assert!(ov3.starts_with("http://") && ov3.ends_with(".onion"));
    v["slatepack_address"].as_str().unwrap().to_owned()
}

fn delete(dir: &str, handle: u64) -> Result<Value, (i32, Value)> {
    call(grin_wallet_delete(cs(dir).as_ptr(), handle))
}

fn test_wallet(chain: &Path, dir: &Path, client_id: &str) -> u64 {
    let v = call(grin_test_create_wallet(
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
        let v = call(grin_mnemonic_generate(words)).unwrap();
        let m = v["mnemonic"].as_str().unwrap().to_owned();
        assert_eq!(m.split_whitespace().count() as u32, words);
        let v = call(grin_mnemonic_validate(cs(&m).as_ptr())).unwrap();
        assert_eq!(v, json!({"valid": true, "word_count": words}));
    }
    assert_eq!(code_of(call(grin_mnemonic_generate(13))), INVALID_ARGUMENT);
    assert_eq!(code_of(call(grin_mnemonic_generate(0))), INVALID_ARGUMENT);

    // Right length, wrong checksum.
    let bad = "abandon ".repeat(12);
    let v = call(grin_mnemonic_validate(cs(&bad).as_ptr())).unwrap();
    assert_eq!(v, json!({"valid": false, "word_count": 12}));
    let v = call(grin_mnemonic_validate(cs("not a mnemonic").as_ptr())).unwrap();
    assert_eq!(v, json!({"valid": false, "word_count": 3}));
    let v = call(grin_mnemonic_validate(cs(MNEMONIC).as_ptr())).unwrap();
    assert_eq!(v["valid"], true);
    assert_eq!(
        code_of(call(grin_mnemonic_validate(ptr::null()))),
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

    let v = call(grin_wallet_get_mnemonic(h, cs(PASSWORD).as_ptr())).unwrap();
    assert_eq!(
        v["mnemonic"].as_str().unwrap().split_whitespace().count(),
        24
    );
    assert_eq!(
        code_of(call(grin_wallet_get_mnemonic(h, cs("nope").as_ptr()))),
        INVALID_PASSWORD
    );
    close(h);
    assert_eq!(code_of(call(grin_wallet_close(h))), INVALID_HANDLE);

    let h = open(&dir, AUTOMATED).unwrap();
    assert_eq!(address(h, 0), addr);

    // The address validates on its own network and is rejected on mainnet.
    let v = call(grin_address_validate(cs(&addr).as_ptr(), AUTOMATED)).unwrap();
    assert_eq!(v["kind"], "slatepack");
    assert_eq!(v["slatepack_address"], addr.as_str());
    assert_eq!(v["url"], v["onion_v3_url"]);
    assert_eq!(
        code_of(call(grin_address_validate(cs(&addr).as_ptr(), MAINNET))),
        INVALID_ADDRESS
    );
    assert_eq!(
        code_of(call(grin_address_validate(cs(&addr).as_ptr(), 7))),
        INVALID_ARGUMENT
    );

    // Password change takes effect for the next open.
    call(grin_wallet_change_password(
        h,
        cs(PASSWORD).as_ptr(),
        cs("new password").as_ptr(),
    ))
    .unwrap();
    assert_eq!(
        code_of(call(grin_wallet_change_password(
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
    let main_onion = call(grin_wallet_get_address(h, 0)).unwrap()["onion_v3_url"].clone();
    let v = call(grin_wallet_get_mnemonic(h, cs(PASSWORD).as_ptr())).unwrap();
    assert_eq!(v["mnemonic"], MNEMONIC);
    close(h);
    let auto_dir = path_str(&tmp.path().join("auto2"));
    let h = create(&auto_dir, AUTOMATED, Some(MNEMONIC)).unwrap();
    assert!(address(h, 0).starts_with("tgrin1"));
    let auto_onion = call(grin_wallet_get_address(h, 0)).unwrap()["onion_v3_url"].clone();
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

    let v = call(grin_test_mine_blocks(alice, 5)).unwrap();
    assert_eq!(v["height"], 5);
    let v = call(grin_wallet_get_chain_height(alice)).unwrap();
    assert_eq!(v["height"], 5);
    assert!(v["hash"].as_str().unwrap().len() == 64);

    let b = call(grin_wallet_get_balance(alice, true, 1)).unwrap();
    assert_eq!(b["refreshed_from_node"], true);
    assert_eq!(b["last_confirmed_height"], 5);
    let spendable = b["amount_currently_spendable"].as_u64().unwrap();
    let immature = b["amount_immature"].as_u64().unwrap();
    assert!(spendable > 0, "{b}");
    assert!(immature > 0, "{b}");
    assert_eq!(b["total"].as_u64().unwrap(), spendable + immature);

    let b = call(grin_wallet_get_balance(bob, true, 1)).unwrap();
    assert_eq!(b["total"], 0);

    let v = call(grin_wallet_scan(alice, 0, false)).unwrap();
    assert_eq!(v, json!({"scanned_height": 5, "tip_height": 5}));
    let p = call(grin_wallet_scan_progress(alice)).unwrap();
    assert_eq!(p["is_scanning"], false);
    assert_eq!(p["cancel_requested"], false);
    assert_eq!(p["start_height"], 1);
    assert_eq!(p["tip_height"], 5);
    assert_eq!(p["progress_pct"], 100);

    let after = call(grin_wallet_get_balance(alice, false, 1)).unwrap();
    assert_eq!(after["amount_currently_spendable"], spendable);

    // Cancel before a scan stops that scan from starting, once.
    call(grin_wallet_scan_cancel(alice)).unwrap();
    let p = call(grin_wallet_scan_progress(alice)).unwrap();
    assert_eq!(p["cancel_requested"], true);
    assert_eq!(code_of(call(grin_wallet_scan(alice, 0, false))), CANCELLED);
    call(grin_wallet_scan(alice, 3, false)).unwrap();

    // Phase 1b entry points exist and say so.
    assert_eq!(code_of(call(grin_txs_get(alice, false))), NOT_SUPPORTED);
    assert_eq!(
        code_of(call(grin_tx_estimate_fee(alice, 1, 1, false, false))),
        NOT_SUPPORTED
    );

    close(alice);
    close(bob);

    // Reopening a test wallet on the same chain keeps its funds.
    let alice = test_wallet(&chain, &tmp.path().join("alice"), "alice");
    let b = call(grin_wallet_get_balance(alice, true, 1)).unwrap();
    assert_eq!(b["amount_currently_spendable"], spendable);
    close(alice);
}

fn wallet_busy_case(use_finalizer: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("w");
    let dir = path_str(&real);
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let link = path_str(&link);
    let dotted = format!("{dir}/./");
    assert!(!real.exists(), "directory must not exist yet");

    let h = create(&dir, AUTOMATED, None).unwrap();
    let addr = address(h, 0);
    let canonical = std::fs::canonicalize(real.join("wallet_data")).unwrap();

    // Stand-in for an in-flight call that still holds the session.
    let retained = retain_session(h).expect("session is registered");
    if use_finalizer {
        grin_wallet_close_finalizer(h as usize as *mut c_void);
    } else {
        close(h);
    }
    assert_eq!(code_of(call(grin_wallet_get_address(h, 0))), INVALID_HANDLE);

    for d in [&dir, &dotted, &link] {
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
    assert_eq!(code_of(open(&dir, AUTOMATED)), WALLET_BUSY);
    assert_eq!(code_of(open(&link, AUTOMATED)), WALLET_BUSY);
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
    delete(&link, h).unwrap();
    assert_eq!(code_of(call(grin_wallet_get_address(h, 0))), INVALID_HANDLE);
    assert!(!canonical.exists());
    assert!(real.exists(), "only wallet_data is removed");
    delete(&dir, 0).unwrap();
    assert_eq!(code_of(open(&dir, AUTOMATED)), WALLET_NOT_FOUND);

    // An unknown handle is ignored by the finalizer.
    grin_wallet_close_finalizer(987_654_321usize as *mut c_void);
    grin_wallet_close_finalizer(ptr::null_mut());
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
                    call(grin_address_validate(cs(probe).as_ptr(), net)).unwrap();
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

    assert_eq!(code_of(call(grin_wallet_get_chain_height(h))), NODE);
    call(grin_wallet_update_network(
        h,
        cs("http://127.0.0.1:2").as_ptr(),
        cs("secret").as_ptr(),
        cs("127.0.0.1:9050").as_ptr(),
    ))
    .unwrap();
    assert_eq!(
        code_of(call(grin_wallet_update_network(
            h,
            cs(UNREACHABLE_NODE).as_ptr(),
            ptr::null(),
            cs("localhost:9050").as_ptr(),
        ))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        code_of(call(grin_wallet_update_network(
            h,
            cs("ftp://x").as_ptr(),
            ptr::null(),
            ptr::null(),
        ))),
        INVALID_ARGUMENT
    );
    call(grin_wallet_update_network(
        h,
        cs(UNREACHABLE_NODE).as_ptr(),
        ptr::null(),
        cs("").as_ptr(),
    ))
    .unwrap();
    assert_eq!(code_of(call(grin_wallet_get_chain_height(h))), NODE);
    // An unreachable node is not an error for balances, only not a refresh.
    let b = call(grin_wallet_get_balance(h, true, 10)).unwrap();
    assert_eq!(b["refreshed_from_node"], false);
    assert_eq!(b["minimum_confirmations"], 10);
    assert_eq!(b["total"], 0);
    close(h);

    assert_eq!(
        code_of(call(grin_node_test_connection(
            cs(UNREACHABLE_NODE).as_ptr(),
            ptr::null(),
            ptr::null(),
        ))),
        NODE
    );
    assert_eq!(
        code_of(call(grin_node_test_connection(
            ptr::null(),
            ptr::null(),
            ptr::null()
        ))),
        INVALID_ARGUMENT
    );
}

#[test]
fn error_paths() {
    call(grin_init_logs(cs("error").as_ptr())).unwrap();
    call(grin_init_logs(ptr::null())).unwrap();
    call(grin_init_logs(cs("error").as_ptr())).unwrap();
    assert_eq!(
        code_of(call(grin_init_logs(cs("chatty").as_ptr()))),
        INVALID_ARGUMENT
    );

    let e = call(grin_test_panic()).unwrap_err();
    assert_eq!(e.0, INTERNAL_PANIC);
    assert_eq!(e.1["message"], "grin_test_panic");

    for h in [0u64, 12345] {
        assert_eq!(code_of(call(grin_wallet_get_address(h, 0))), INVALID_HANDLE);
        assert_eq!(code_of(call(grin_wallet_scan_progress(h))), INVALID_HANDLE);
        assert_eq!(code_of(call(grin_wallet_scan_cancel(h))), INVALID_HANDLE);
        assert_eq!(
            code_of(call(grin_wallet_get_balance(h, false, 1))),
            INVALID_HANDLE
        );
        assert_eq!(code_of(call(grin_wallet_close(h))), INVALID_HANDLE);
        assert_eq!(code_of(call(grin_test_mine_blocks(h, 1))), INVALID_HANDLE);
    }

    let tmp = tempfile::tempdir().unwrap();
    let dir = path_str(&tmp.path().join("w"));
    let pw = cs(PASSWORD);

    // NULL pointers.
    assert_eq!(
        code_of(call(grin_wallet_create(
            ptr::null(),
            ptr::null(),
            pw.as_ptr()
        ))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        code_of(call(grin_wallet_create(
            config(&dir, AUTOMATED).as_ptr(),
            ptr::null(),
            ptr::null()
        ))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        code_of(call(grin_wallet_open(
            config(&dir, AUTOMATED).as_ptr(),
            ptr::null()
        ))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        code_of(call(grin_wallet_delete(ptr::null(), 0))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        code_of(call(grin_address_validate(ptr::null(), MAINNET))),
        INVALID_ARGUMENT
    );

    // Invalid UTF-8.
    let bad = CString::new(vec![b'{', 0xff, 0xfe, b'}']).unwrap();
    assert_eq!(
        code_of(call(grin_wallet_open(bad.as_ptr(), pw.as_ptr()))),
        INVALID_UTF8
    );
    assert_eq!(
        code_of(call(grin_mnemonic_validate(bad.as_ptr()))),
        INVALID_UTF8
    );
    assert_eq!(
        code_of(call(grin_wallet_create(
            config(&dir, AUTOMATED).as_ptr(),
            ptr::null(),
            bad.as_ptr()
        ))),
        INVALID_UTF8
    );

    // Malformed or invalid config.
    assert_eq!(
        code_of(call(grin_wallet_open(
            cs("{not json").as_ptr(),
            pw.as_ptr()
        ))),
        JSON
    );
    assert_eq!(
        code_of(call(grin_wallet_open(
            cs("{\"network\": 0}").as_ptr(),
            pw.as_ptr()
        ))),
        JSON
    );
    let bad_net = json!({"wallet_dir": dir, "network": 9, "node_url": UNREACHABLE_NODE});
    assert_eq!(
        code_of(call(grin_wallet_create(
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
        code_of(call(grin_wallet_create(
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
    let v = call(grin_address_validate(
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
        code_of(call(grin_address_validate(
            cs("grin1nope").as_ptr(),
            MAINNET
        ))),
        INVALID_ADDRESS
    );
}
