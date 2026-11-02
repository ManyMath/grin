//! Funded transaction flows through the C ABI on the in-process test chain:
//! S1 -> S2 -> S3, I1 -> I2 -> I3, cancel, repost, payment proofs, the
//! synchronous HTTP send and argument errors. Every test has its own chain
//! and wallets in a fresh tempdir.
//!
//! Harness rules (src/test_support.rs): one node call at a time per test
//! wallet, and no mining while a broadcast is in flight on the same chain.
//! A broadcast on the test chain mines a block that pays the poster.

#![cfg(feature = "test-utils")]

use std::ffi::{CStr, CString};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::raw::c_char;
use std::path::Path;
use std::ptr;
use std::thread;

use grin_core::global::{self, ChainTypes};
use grin_ffi::*;
use grin_wallet_libwallet::{Slate, Slatepacker, SlatepackerArgs};
use serde_json::{json, Value};

const OK: i32 = 0;
const INVALID_ARGUMENT: i32 = GrinErrorCode::InvalidArgument as i32;
const JSON: i32 = GrinErrorCode::Json as i32;
const INVALID_HANDLE: i32 = GrinErrorCode::InvalidHandle as i32;
const INSUFFICIENT_FUNDS: i32 = GrinErrorCode::InsufficientFunds as i32;
const SLATE: i32 = GrinErrorCode::Slate as i32;
const TRANSACTION: i32 = GrinErrorCode::Transaction as i32;
const NOT_FOUND: i32 = GrinErrorCode::NotFound as i32;
const HTTP: i32 = GrinErrorCode::Http as i32;
const INVALID_ADDRESS: i32 = GrinErrorCode::InvalidAddress as i32;

const AUTOMATED: u8 = 3;
const PASSWORD: &str = "pw";
/// Block reward in nanogrin (60 grin).
const REWARD: u64 = 60_000_000_000;
const GRIN: u64 = 1_000_000_000;

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
        assert!(error.is_none());
        Ok(data
            .map(|d| serde_json::from_str(&d).unwrap())
            .unwrap_or(Value::Null))
    } else {
        assert!(data.is_none());
        let e: Value = serde_json::from_str(&error.expect("error JSON")).unwrap();
        assert_eq!(e["code"], code);
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

fn test_wallet(chain: &Path, dir: &Path, client_id: &str) -> u64 {
    let v = call!(grin_test_create_wallet(
        cs(chain.to_str().unwrap()).as_ptr(),
        cs(dir.to_str().unwrap()).as_ptr(),
        cs(client_id).as_ptr(),
        AUTOMATED,
        ptr::null(),
        cs(PASSWORD).as_ptr(),
    ))
    .unwrap();
    v["handle"].as_u64().unwrap()
}

/// Alice and Bob on a fresh chain, with `blocks` mined to Alice.
fn alice_and_bob(tmp: &Path, blocks: u64) -> (u64, u64) {
    let chain = tmp.join("chain");
    let alice = test_wallet(&chain, &tmp.join("alice"), "alice");
    let bob = test_wallet(&chain, &tmp.join("bob"), "bob");
    if blocks > 0 {
        mine(alice, blocks);
    }
    (alice, bob)
}

fn mine(handle: u64, count: u64) -> u64 {
    call!(grin_test_mine_blocks(handle, count)).unwrap()["height"]
        .as_u64()
        .unwrap()
}

fn address(handle: u64) -> String {
    call!(grin_wallet_get_address(handle, 0)).unwrap()["slatepack_address"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn balance(handle: u64, refresh: bool) -> Value {
    call!(grin_wallet_get_balance(handle, refresh, 1)).unwrap()
}

fn u(v: &Value) -> u64 {
    v.as_u64().unwrap_or_else(|| panic!("not a u64: {v}"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}"))
}

fn init_send(handle: u64, args: Value) -> Result<Value, (i32, Value)> {
    call!(grin_tx_init_send(handle, cs(&args.to_string()).as_ptr()))
}

fn receive(handle: u64, armored: &str) -> Result<Value, (i32, Value)> {
    call!(grin_tx_receive_slatepack(
        handle,
        cs(armored).as_ptr(),
        ptr::null(),
    ))
}

fn finalize(handle: u64, armored: &str, broadcast: bool) -> Result<Value, (i32, Value)> {
    call!(grin_tx_finalize_slatepack(
        handle,
        cs(armored).as_ptr(),
        broadcast,
        false,
    ))
}

fn decode(handle: u64, armored: &str) -> Result<Value, (i32, Value)> {
    call!(grin_slatepack_decode(
        handle,
        AUTOMATED,
        cs(armored).as_ptr(),
        0,
    ))
}

fn txs(handle: u64, refresh: bool) -> Value {
    call!(grin_txs_get(handle, refresh)).unwrap()
}

/// The single log entry for `slate_id`.
fn entry(txs: &Value, slate_id: &str) -> Value {
    let found: Vec<&Value> = txs["txs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["slate_id"] == slate_id)
        .collect();
    assert_eq!(found.len(), 1, "entries for {slate_id}: {txs}");
    found[0].clone()
}

fn armored(v: &Value) -> String {
    s(&v["armored_slatepack"]).to_owned()
}

#[test]
fn slatepack_s1_s2_s3_with_payment_proof() {
    let tmp = tempfile::tempdir().unwrap();
    let (alice, bob) = alice_and_bob(tmp.path(), 5);
    let alice_addr = address(alice);
    let bob_addr = address(bob);
    let amount = 50 * GRIN;

    // Coinbase maturity is 3, so at height 5 the outputs of heights 1 and 2
    // are spendable.
    let b = balance(alice, true);
    assert_eq!(u(&b["amount_currently_spendable"]), 2 * REWARD, "{b}");

    let est = call!(grin_tx_estimate_fee(alice, amount, 1, false, false)).unwrap();
    let fee = u(&est["fee"]);
    assert!(fee > 0, "{est}");
    assert_eq!(u(&est["amount"]), amount);
    assert_eq!(est["net_amount"], est["amount"]);
    assert_eq!(u(&est["total"]), amount + fee);
    assert_eq!(est["inputs_count"], 1, "{est}");
    assert_eq!(est["change_outputs"], 1, "{est}");
    let inc = call!(grin_tx_estimate_fee(alice, amount, 1, false, true)).unwrap();
    assert_eq!(u(&inc["total"]), amount);
    assert_eq!(u(&inc["amount"]) + u(&inc["fee"]), amount);

    // S1, encrypted to Bob and asking for a payment proof.
    let s1 = init_send(
        alice,
        json!({
            "amount": amount,
            "min_confirms": 1,
            "selection_strategy_is_all": false,
            "amount_includes_fee": false,
            "recipient_address": bob_addr,
            "ttl_blocks": null,
            "message": "unused",
            "late_lock": false,
        }),
    )
    .unwrap();
    assert_eq!(s1["state"], "S1");
    assert_eq!(s1["is_encrypted"], true);
    assert_eq!(s1["sender"], alice_addr.as_str());
    assert_eq!(s1["recipients"], json!([bob_addr]));
    assert_eq!(u(&s1["amount"]), amount);
    assert_eq!(u(&s1["fee"]), fee);
    let slate_id = s(&s1["slate_id"]).to_owned();

    // Only Bob can read it.
    let e = decode(0, &armored(&s1)).unwrap_err();
    assert_eq!(e.0, SLATE);
    assert_eq!(e.1["message"], "encrypted slatepack needs an open wallet");
    assert_eq!(code_of(decode(alice, &armored(&s1))), SLATE);
    let d = decode(bob, &armored(&s1)).unwrap();
    assert_eq!(d["slate_id"], slate_id.as_str());
    assert_eq!(d["state"], "S1");
    assert_eq!(d["is_encrypted"], true);
    assert_eq!(d["sender"], alice_addr.as_str());
    assert_eq!(u(&d["amount"]), amount);
    assert_eq!(u(&d["fee"]), fee);
    assert_eq!(d["num_participants"], 2);
    assert!(d["kernel_excess"].is_null());
    let v4: Value = serde_json::from_str(s(&d["slate_json"])).unwrap();
    assert_eq!(v4["id"], slate_id.as_str());

    // The S1 is locked and logged on Alice's side.
    let a = entry(&txs(alice, false), &slate_id);
    assert_eq!(a["type"], "TxSent");
    assert_eq!(a["slate_state"], "S1");
    assert_eq!(a["num_inputs"], est["inputs_count"]);
    assert_eq!(a["counterparty_address"], bob_addr.as_str());
    assert_eq!(a["armored_slatepack"], s1["armored_slatepack"]);
    assert_eq!(a["payment_proof"]["receiver_address"], bob_addr.as_str());
    assert_eq!(a["payment_proof"]["sender_address"], alice_addr.as_str());
    assert_eq!(a["has_stored_tx"], false);
    assert!(u(&balance(alice, false)["amount_locked"]) > 0);

    let s2 = receive(bob, &armored(&s1)).unwrap();
    assert_eq!(s2["state"], "S2");
    assert_eq!(s2["slate_id"], slate_id.as_str());
    assert_eq!(s2["is_encrypted"], true);
    assert_eq!(s2["sender"], bob_addr.as_str());
    assert_eq!(s2["recipients"], json!([alice_addr]));
    assert_eq!(u(&s2["amount"]), amount);
    assert_eq!(u(&s2["fee"]), fee);
    // Receiving twice is refused by libwallet.
    assert_eq!(code_of(receive(bob, &armored(&s1))), TRANSACTION);

    let s3 = finalize(alice, &armored(&s2), true).unwrap();
    assert_eq!(s3["state"], "S3");
    assert_eq!(s3["posted"], true);
    assert_eq!(u(&s3["amount"]), amount);
    assert_eq!(u(&s3["fee"]), fee);
    let excess = s(&s3["kernel_excess"]).to_owned();
    assert_eq!(excess.len(), 66);
    // The transaction context is gone once finalized. The error still names
    // the slate, as every failure after the wallet recorded it does.
    let e = finalize(alice, &armored(&s2), false).unwrap_err();
    assert_eq!(e.0, NOT_FOUND);
    assert_eq!(e.1["details"]["slate_id"], slate_id.as_str());
    // The broadcast mined height 6; one more block confirms it further.
    assert_eq!(mine(alice, 1), 7);

    let d = decode(0, &armored(&s3)).unwrap();
    assert_eq!(d["state"], "S3");
    assert_eq!(d["is_encrypted"], false);
    assert!(d["sender"].is_null());
    assert_eq!(d["kernel_excess"], excess.as_str());
    assert_eq!(u(&d["fee"]), fee);

    let b = balance(bob, true);
    assert_eq!(u(&b["amount_currently_spendable"]), amount, "{b}");
    assert_eq!(u(&b["total"]), amount);
    // Alice mined 7 blocks; the fee came back to her in block 6's coinbase.
    let a = balance(alice, true);
    assert_eq!(u(&a["total"]), 7 * REWARD - amount, "{a}");
    assert_eq!(u(&a["amount_locked"]), 0);

    let at = txs(alice, true);
    assert_eq!(at["refreshed_from_node"], true);
    let a = entry(&at, &slate_id);
    assert_eq!(a["type"], "TxSent");
    assert_eq!(a["confirmed"], true);
    assert_eq!(a["slate_state"], "S3");
    assert_eq!(a["kernel_excess"], excess.as_str());
    assert_eq!(u(&a["fee"]), fee);
    assert_eq!(a["has_stored_tx"], true);
    assert_eq!(a["counterparty_address"], bob_addr.as_str());
    assert_eq!(a["armored_slatepack"], s3["armored_slatepack"]);
    assert_eq!(a["payment_proof"]["receiver_signature_present"], true);
    assert_eq!(a["payment_proof"]["sender_signature_present"], true);
    assert!(s(&a["creation_ts"]).contains('T'));
    assert!(a["confirmation_ts"].is_string());
    assert!(a["ttl_cutoff_height"].is_null());
    assert!(a["reverted_after_secs"].is_null());
    let coinbase = at["txs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["type"] == "ConfirmedCoinbase")
        .count();
    assert!(coinbase >= 5, "{at}");

    let b = entry(&txs(bob, true), &slate_id);
    assert_eq!(b["type"], "TxReceived");
    assert_eq!(b["confirmed"], true);
    assert_eq!(b["slate_state"], "S2");
    assert_eq!(u(&b["amount_credited"]), amount);
    assert_eq!(b["kernel_excess"], excess.as_str());
    assert!(b["payment_proof"].is_null());
    assert_eq!(b["counterparty_address"], alice_addr.as_str());
    assert_eq!(b["armored_slatepack"], s2["armored_slatepack"]);

    // Payment proof: export on Alice, verify on both sides.
    let exported = call!(grin_payment_proof_export(alice, -1, cs(&slate_id).as_ptr())).unwrap();
    let proof = exported["proof"].clone();
    assert_eq!(proof["recipient_address"], bob_addr.as_str());
    assert_eq!(proof["sender_address"], alice_addr.as_str());
    assert_eq!(proof["excess"], excess.as_str());
    let by_id = call!(grin_payment_proof_export(
        alice,
        u(&a["id"]) as i64,
        ptr::null(),
    ))
    .unwrap();
    assert_eq!(by_id, exported);

    let wrapped = cs(&exported.to_string());
    let v = call!(grin_payment_proof_verify(alice, wrapped.as_ptr())).unwrap();
    assert_eq!(v, json!({"is_sender": true, "is_recipient": false}));
    let bare = cs(&proof.to_string());
    let v = call!(grin_payment_proof_verify(bob, bare.as_ptr())).unwrap();
    assert_eq!(v, json!({"is_sender": false, "is_recipient": true}));

    let mut tampered = proof.clone();
    tampered["amount"] = json!("20");
    let e = call!(grin_payment_proof_verify(
        bob,
        cs(&tampered.to_string()).as_ptr(),
    ));
    assert_eq!(code_of(e), TRANSACTION);
    assert_eq!(
        code_of(call!(grin_payment_proof_verify(
            alice,
            cs("{\"proof\": 1}").as_ptr()
        ))),
        JSON
    );
    // Bob's side of a send keeps no proof.
    assert_eq!(
        code_of(call!(grin_payment_proof_export(
            bob,
            -1,
            cs(&slate_id).as_ptr()
        ))),
        NOT_FOUND
    );

    close(alice);
    close(bob);
}

fn close(handle: u64) {
    call!(grin_wallet_close(handle)).unwrap();
}

#[test]
fn late_lock_locks_inputs_at_finalization() {
    let tmp = tempfile::tempdir().unwrap();
    let (alice, bob) = alice_and_bob(tmp.path(), 5);
    let bob_addr = address(bob);
    let amount = 50 * GRIN;
    let spendable = 2 * REWARD;
    assert_eq!(
        u(&balance(alice, true)["amount_currently_spendable"]),
        spendable
    );

    let s1 = init_send(
        alice,
        json!({
            "amount": amount,
            "min_confirms": 1,
            "recipient_address": bob_addr,
            "late_lock": true,
        }),
    )
    .unwrap();
    assert_eq!(s1["state"], "S1");
    let fee = u(&s1["fee"]);
    assert!(fee > 0, "{s1}");
    let slate_id = s(&s1["slate_id"]).to_owned();

    // Nothing is locked, and libwallet logs the transaction only when it
    // locks the inputs.
    let b = balance(alice, false);
    assert_eq!(u(&b["amount_locked"]), 0, "{b}");
    assert_eq!(u(&b["amount_currently_spendable"]), spendable, "{b}");
    let logged = txs(alice, false)["txs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["slate_id"] == slate_id.as_str())
        .count();
    assert_eq!(logged, 0);

    // So the whole balance can still go into another S1. Cancelling that
    // one frees its inputs again.
    let all = init_send(
        alice,
        json!({
            "amount": spendable,
            "min_confirms": 1,
            "amount_includes_fee": true,
            "selection_strategy_is_all": true,
        }),
    )
    .unwrap();
    assert_eq!(u(&balance(alice, false)["amount_locked"]), spendable);
    call!(grin_tx_cancel(alice, -1, cs(s(&all["slate_id"])).as_ptr())).unwrap();
    assert_eq!(u(&balance(alice, false)["amount_locked"]), 0);

    // Finalizing selects and locks the inputs, then broadcasts.
    let s2 = receive(bob, &armored(&s1)).unwrap();
    let s3 = finalize(alice, &armored(&s2), true).unwrap();
    assert_eq!(s3["state"], "S3");
    assert_eq!(s3["posted"], true);
    assert_eq!(u(&s3["amount"]), amount);
    assert_eq!(u(&s3["fee"]), fee);
    // The wallet has not looked at the chain since, so the spent inputs
    // still show as locked.
    assert!(u(&balance(alice, false)["amount_locked"]) >= amount + fee);
    let a = entry(&txs(alice, false), &slate_id);
    assert_eq!(a["type"], "TxSent");
    assert!(u(&a["num_inputs"]) >= 1, "{a}");
    assert_eq!(a["has_stored_tx"], true);

    // The broadcast mined height 6.
    assert_eq!(mine(alice, 1), 7);
    let b = balance(bob, true);
    assert_eq!(u(&b["amount_currently_spendable"]), amount, "{b}");
    let a = balance(alice, true);
    assert_eq!(u(&a["total"]), 7 * REWARD - amount, "{a}");
    assert_eq!(u(&a["amount_locked"]), 0, "{a}");
    let a = entry(&txs(alice, false), &slate_id);
    assert_eq!(a["confirmed"], true);
    assert_eq!(a["kernel_excess"], s3["kernel_excess"]);
    assert_eq!(a["payment_proof"]["receiver_signature_present"], true);
    close(alice);
    close(bob);
}

#[test]
fn invoice_i1_i2_i3_locks_payer_inputs() {
    let tmp = tempfile::tempdir().unwrap();
    // Height 6: heights 1 to 3 are mature.
    let (alice, bob) = alice_and_bob(tmp.path(), 6);
    let alice_addr = address(alice);
    let bob_addr = address(bob);
    let amount = 30 * GRIN;

    let i1 = call!(grin_tx_issue_invoice(
        bob,
        cs(
            &json!({"amount": amount, "dest_acct_name": null, "recipient_address": alice_addr})
                .to_string(),
        )
        .as_ptr(),
    ))
    .unwrap();
    assert_eq!(i1["state"], "I1");
    assert_eq!(i1["is_encrypted"], true);
    assert_eq!(i1["sender"], bob_addr.as_str());
    assert_eq!(i1["recipients"], json!([alice_addr]));
    assert_eq!(u(&i1["amount"]), amount);
    assert_eq!(u(&i1["fee"]), 0);
    let slate_id = s(&i1["slate_id"]).to_owned();
    let b = entry(&txs(bob, false), &slate_id);
    assert_eq!(b["type"], "TxReceived");
    assert_eq!(b["slate_state"], "I1");
    assert_eq!(b["counterparty_address"], alice_addr.as_str());

    let spendable = u(&balance(alice, true)["amount_currently_spendable"]);
    assert_eq!(spendable, 3 * REWARD);

    let i2 = call!(grin_tx_process_invoice(
        alice,
        cs(&armored(&i1)).as_ptr(),
        cs(r#"{"min_confirms": 1, "selection_strategy_is_all": true, "ttl_blocks": null}"#)
            .as_ptr(),
    ))
    .unwrap();
    assert_eq!(i2["state"], "I2");
    assert_eq!(i2["slate_id"], slate_id.as_str());
    assert_eq!(i2["is_encrypted"], true);
    assert_eq!(i2["recipients"], json!([bob_addr]));
    assert_eq!(u(&i2["amount"]), amount);
    let fee = u(&i2["fee"]);
    assert!(fee > 0);

    // process_invoice_tx alone would leave these outputs free; the FFI locks
    // them right away, so spending what is left fails before I3 exists.
    let remaining = spendable - amount - fee;
    let e = init_send(alice, json!({"amount": remaining, "min_confirms": 1})).unwrap_err();
    assert_eq!(e.0, INSUFFICIENT_FUNDS, "{}", e.1);
    let details = &e.1["details"];
    assert!(
        u(&details["available"]) < u(&details["required"]),
        "{details}"
    );
    let b = balance(alice, false);
    assert_eq!(u(&b["amount_currently_spendable"]), 0, "{b}");
    assert!(u(&b["amount_locked"]) >= amount + fee);
    let a = entry(&txs(alice, false), &slate_id);
    assert_eq!(a["type"], "TxSent");
    assert_eq!(a["slate_state"], "I2");
    assert_eq!(a["counterparty_address"], bob_addr.as_str());

    let i3 = call!(grin_tx_finalize_invoice(
        bob,
        cs(&armored(&i2)).as_ptr(),
        true,
        false,
    ))
    .unwrap();
    assert_eq!(i3["state"], "I3");
    assert_eq!(i3["posted"], true);
    let e = call!(grin_tx_finalize_invoice(
        bob,
        cs(&armored(&i2)).as_ptr(),
        false,
        false,
    ))
    .unwrap_err();
    assert_eq!(e.0, NOT_FOUND, "{}", e.1);
    assert_eq!(e.1["details"]["slate_id"], slate_id.as_str());
    assert_eq!(u(&i3["amount"]), amount);
    assert_eq!(u(&i3["fee"]), fee);
    let excess = s(&i3["kernel_excess"]).to_owned();
    // The broadcast mined height 7 and paid its coinbase (plus the fee) to Bob.
    assert_eq!(mine(alice, 1), 8);

    let b = balance(bob, true);
    assert_eq!(u(&b["amount_currently_spendable"]), amount, "{b}");
    assert_eq!(u(&b["amount_immature"]), REWARD + fee, "{b}");
    let a = balance(alice, true);
    assert_eq!(u(&a["total"]), 7 * REWARD - amount - fee, "{a}");
    assert_eq!(u(&a["amount_locked"]), 0, "{a}");

    let b = entry(&txs(bob, true), &slate_id);
    assert_eq!(b["slate_state"], "I3");
    assert_eq!(b["confirmed"], true);
    assert_eq!(b["armored_slatepack"], i3["armored_slatepack"]);
    let a = entry(&txs(alice, true), &slate_id);
    assert_eq!(a["confirmed"], true);
    assert_eq!(a["kernel_excess"], excess.as_str());

    let d = decode(0, &armored(&i3)).unwrap();
    assert_eq!(d["state"], "I3");
    assert_eq!(d["kernel_excess"], excess.as_str());

    close(alice);
    close(bob);
}

#[test]
fn cancel_by_slate_id_and_by_tx_id() {
    let tmp = tempfile::tempdir().unwrap();
    let (alice, bob) = alice_and_bob(tmp.path(), 5);
    let alice_addr = address(alice);
    let before = u(&balance(alice, true)["amount_currently_spendable"]);

    // Unencrypted S1: no recipient, the sender travels in the clear.
    let s1 = init_send(alice, json!({"amount": 10 * GRIN, "min_confirms": 1})).unwrap();
    assert_eq!(s1["is_encrypted"], false);
    assert_eq!(s1["recipients"], json!([]));
    let d = decode(0, &armored(&s1)).unwrap();
    assert_eq!(d["is_encrypted"], false);
    assert_eq!(d["sender"], alice_addr.as_str());
    let slate_id = s(&s1["slate_id"]).to_owned();
    let locked = balance(alice, false);
    assert!(u(&locked["amount_currently_spendable"]) < before);
    assert!(u(&locked["amount_locked"]) > 0);

    let v = call!(grin_tx_cancel(alice, -1, cs(&slate_id).as_ptr())).unwrap();
    assert_eq!(v, json!({"cancelled": true}));
    let e = entry(&txs(alice, false), &slate_id);
    assert_eq!(e["type"], "TxSentCancelled");
    let b = balance(alice, false);
    assert_eq!(u(&b["amount_currently_spendable"]), before, "{b}");
    assert_eq!(u(&b["amount_locked"]), 0);
    // Already cancelled.
    assert_eq!(
        code_of(call!(grin_tx_cancel(alice, -1, cs(&slate_id).as_ptr()))),
        TRANSACTION
    );

    let s1 = init_send(alice, json!({"amount": 15 * GRIN, "min_confirms": 1})).unwrap();
    let id = u(&entry(&txs(alice, false), s(&s1["slate_id"]))["id"]);
    let v = call!(grin_tx_cancel(alice, id as i64, ptr::null())).unwrap();
    assert_eq!(v["cancelled"], true);
    let e = entry(&txs(alice, false), s(&s1["slate_id"]));
    assert_eq!(e["type"], "TxSentCancelled");
    assert_eq!(
        u(&balance(alice, false)["amount_currently_spendable"]),
        before
    );

    let unknown = cs("6f1c3e1a-0d4b-4c39-9b6e-6c1f6f0d2a11");
    assert_eq!(
        code_of(call!(grin_tx_cancel(alice, -1, unknown.as_ptr()))),
        NOT_FOUND
    );
    assert_eq!(
        code_of(call!(grin_tx_cancel(alice, -1, ptr::null()))),
        INVALID_ARGUMENT
    );
    close(alice);
    close(bob);
}

#[test]
fn repost_a_finalized_transaction() {
    let tmp = tempfile::tempdir().unwrap();
    let (alice, bob) = alice_and_bob(tmp.path(), 5);
    let amount = 20 * GRIN;

    let s1 = init_send(
        alice,
        json!({"amount": amount, "min_confirms": 1, "recipient_address": address(bob)}),
    )
    .unwrap();
    let slate_id = s(&s1["slate_id"]).to_owned();
    // Not finalized yet, so nothing is stored to repost.
    assert_eq!(
        code_of(call!(grin_tx_repost(
            alice,
            -1,
            cs(&slate_id).as_ptr(),
            false
        ))),
        NOT_FOUND
    );
    let s2 = receive(bob, &armored(&s1)).unwrap();
    let s3 = finalize(alice, &armored(&s2), false).unwrap();
    assert_eq!(s3["posted"], false);
    let e = entry(&txs(alice, false), &slate_id);
    assert_eq!(e["has_stored_tx"], true);
    assert_eq!(e["confirmed"], false);

    let v = call!(grin_tx_repost(alice, -1, cs(&slate_id).as_ptr(), false)).unwrap();
    assert_eq!(v, json!({"posted": true}));
    assert_eq!(mine(alice, 1), 7);
    let e = entry(&txs(alice, true), &slate_id);
    assert_eq!(e["confirmed"], true);
    assert_eq!(e["kernel_excess"], s3["kernel_excess"]);
    assert_eq!(u(&balance(bob, true)["amount_currently_spendable"]), amount);

    // A coinbase entry has no slate and nothing stored.
    let coinbase_id = txs(alice, false)["txs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["type"] == "ConfirmedCoinbase")
        .map(|t| u(&t["id"]))
        .unwrap();
    assert_eq!(
        code_of(call!(grin_tx_repost(
            alice,
            coinbase_id as i64,
            ptr::null(),
            false
        ))),
        NOT_FOUND
    );
    assert_eq!(
        code_of(call!(grin_tx_repost(alice, -1, ptr::null(), false))),
        INVALID_ARGUMENT
    );

    // A finalized transaction that was cancelled before it was posted has a
    // stored copy, but reposting it is refused, by slate id and by tx id.
    let s1 = init_send(
        alice,
        json!({"amount": amount, "min_confirms": 1, "recipient_address": address(bob)}),
    )
    .unwrap();
    let cancelled_id = s(&s1["slate_id"]).to_owned();
    let s2 = receive(bob, &armored(&s1)).unwrap();
    finalize(alice, &armored(&s2), false).unwrap();
    assert_eq!(
        entry(&txs(alice, false), &cancelled_id)["has_stored_tx"],
        true
    );
    call!(grin_tx_cancel(alice, -1, cs(&cancelled_id).as_ptr())).unwrap();
    let e = call!(grin_tx_repost(alice, -1, cs(&cancelled_id).as_ptr(), false)).unwrap_err();
    assert_eq!(e.0, TRANSACTION, "{}", e.1);
    assert!(s(&e.1["message"]).contains("cancelled"), "{}", e.1);
    let id = u(&entry(&txs(alice, false), &cancelled_id)["id"]);
    assert_eq!(
        code_of(call!(grin_tx_repost(alice, id as i64, ptr::null(), false))),
        TRANSACTION
    );
    close(alice);
    close(bob);
}

#[test]
fn estimate_when_change_comes_out_at_zero() {
    let tmp = tempfile::tempdir().unwrap();
    // Height 4: one mature coinbase output of REWARD.
    let (alice, bob) = alice_and_bob(tmp.path(), 4);
    let est = call!(grin_tx_estimate_fee(alice, 50 * GRIN, 1, false, false)).unwrap();
    assert_eq!(est["inputs_count"], 1);
    assert_eq!(est["change_outputs"], 1);
    // The fee of one input and two outputs.
    let fee = u(&est["fee"]);

    // REWARD - fee: libwallet's first try (fee for one output) leaves
    // change, so it prices a change output; with that fee the one input
    // pays exactly and no change output is made. The fee keeps the change
    // output's weight, which must not be read as 21 more inputs.
    let amount = REWARD - fee;
    let check = |est: &Value| {
        assert_eq!(u(&est["fee"]), fee, "{est}");
        assert_eq!(u(&est["total"]), REWARD, "{est}");
        assert_eq!(est["change_outputs"], 0, "{est}");
        assert_eq!(est["inputs_count"], 1, "{est}");
    };
    // With one spendable output, pricing a further change output runs out
    // of funds.
    check(&call!(grin_tx_estimate_fee(alice, amount, 1, false, false)).unwrap());
    // With two, it selects both and the fee moves.
    assert_eq!(mine(alice, 1), 5);
    let est = call!(grin_tx_estimate_fee(alice, amount, 1, false, false)).unwrap();
    check(&est);

    // The real send agrees.
    let s1 = init_send(alice, json!({"amount": amount, "min_confirms": 1})).unwrap();
    assert_eq!(u(&s1["fee"]), fee);
    let e = entry(&txs(alice, false), s(&s1["slate_id"]));
    assert_eq!(e["num_inputs"], est["inputs_count"], "{e}");
    call!(grin_tx_cancel(alice, -1, cs(s(&s1["slate_id"])).as_ptr())).unwrap();

    // The whole output with the fee taken out of it: paid exactly on the
    // first try, priced for one output.
    let est = call!(grin_tx_estimate_fee(alice, REWARD, 1, false, true)).unwrap();
    assert_eq!(est["change_outputs"], 0, "{est}");
    assert_eq!(est["inputs_count"], 1, "{est}");
    assert_eq!(u(&est["total"]), REWARD);
    close(alice);
    close(bob);
}

/// Reads one HTTP/1.1 request: (request line, body).
fn read_request(stream: &TcpStream) -> (String, String) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    reader.read_line(&mut request_line).unwrap();
    let mut len = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                len = value.trim().parse().unwrap();
            }
        }
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).unwrap();
    (
        request_line.trim_end().to_owned(),
        String::from_utf8(body).unwrap(),
    )
}

/// A scripted foreign API on 127.0.0.1: answers `check_version` itself and
/// passes `receive_tx` to Bob's wallet through the C ABI. Returns the base
/// URL and a handle that yields the JSON-RPC methods it served.
fn foreign_api(bob: u64) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        global::set_local_chain_type(ChainTypes::AutomatedTesting);
        let mut methods = vec![];
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let (request_line, body) = read_request(&stream);
            assert!(
                request_line.starts_with("POST /v2/foreign "),
                "{request_line}"
            );
            let req: Value = serde_json::from_str(&body).unwrap();
            let method = s(&req["method"]).to_owned();
            let result = match method.as_str() {
                "check_version" => {
                    json!({"foreign_api_version": 2, "supported_slate_versions": ["V4"]})
                }
                "receive_tx" => {
                    let s1 = Slate::deserialize_upgrade(&req["params"][0].to_string()).unwrap();
                    let packer = Slatepacker::new(SlatepackerArgs {
                        sender: None,
                        recipients: vec![],
                        dec_key: None,
                    });
                    let pack = packer
                        .armor_slatepack(&packer.create_slatepack(&s1).unwrap())
                        .unwrap();
                    let s2 = receive(bob, &pack).unwrap();
                    let d = decode(bob, &armored(&s2)).unwrap();
                    serde_json::from_str::<Value>(s(&d["slate_json"])).unwrap()
                }
                other => panic!("unexpected method {other}"),
            };
            let reply = json!({"jsonrpc": "2.0", "id": 1, "result": {"Ok": result}}).to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.len(),
                reply
            )
            .unwrap();
            methods.push(method);
        }
        methods
    });
    (url, server)
}

fn send_http(handle: u64, args: Value) -> Result<Value, (i32, Value)> {
    call!(grin_tx_send_http(handle, cs(&args.to_string()).as_ptr()))
}

#[test]
fn synchronous_http_send() {
    let tmp = tempfile::tempdir().unwrap();
    let (alice, bob) = alice_and_bob(tmp.path(), 5);
    let bob_addr = address(bob);
    let amount = 25 * GRIN;
    let before = u(&balance(alice, true)["amount_currently_spendable"]);

    // Test wallets have no SOCKS proxy, so nothing that resolves to .onion
    // is allowed.
    let onion = s(&call!(grin_wallet_get_address(bob, 0)).unwrap()["onion_v3_url"]).to_owned();
    for target in [bob_addr.as_str(), onion.as_str()] {
        let e = send_http(
            alice,
            json!({"amount": amount, "min_confirms": 1, "recipient_url_or_address": target}),
        )
        .unwrap_err();
        assert_eq!(e.0, INVALID_ARGUMENT, "{target}: {}", e.1);
    }
    assert_eq!(
        code_of(send_http(
            alice,
            json!({"amount": amount, "min_confirms": 1, "recipient_url_or_address": "ftp://x"}),
        )),
        INVALID_ADDRESS
    );
    // A slatepack or onion target is Bob's key; asking for the proof from
    // another key is refused up front.
    let alice_addr = address(alice);
    for target in [bob_addr.as_str(), onion.as_str()] {
        let e = send_http(
            alice,
            json!({
                "amount": amount,
                "min_confirms": 1,
                "recipient_url_or_address": target,
                "recipient_address": alice_addr,
            }),
        )
        .unwrap_err();
        assert_eq!(e.0, INVALID_ARGUMENT, "{target}: {}", e.1);
        assert!(s(&e.1["message"]).contains("differs"), "{target}: {}", e.1);
    }

    // Nobody listens on port 1: a transport error, and nothing stays locked.
    let e = send_http(
        alice,
        json!({"amount": amount, "min_confirms": 1, "recipient_url_or_address": "http://127.0.0.1:1"}),
    )
    .unwrap_err();
    assert_eq!(e.0, HTTP, "{}", e.1);
    let b = balance(alice, false);
    assert_eq!(u(&b["amount_currently_spendable"]), before);
    assert_eq!(u(&b["amount_locked"]), 0);

    // A real round trip to Bob behind a local foreign API, with a payment
    // proof requested from Bob's address.
    let (url, server) = foreign_api(bob);
    let v = send_http(
        alice,
        json!({
            "amount": amount,
            "min_confirms": 1,
            "recipient_url_or_address": format!("{url}/v2/foreign"),
            "recipient_address": bob_addr,
            "fluff": false,
            "post": true,
        }),
    )
    .unwrap();
    assert_eq!(server.join().unwrap(), vec!["check_version", "receive_tx"]);
    assert_eq!(v["state"], "S3");
    assert_eq!(v["posted"], true);
    assert_eq!(u(&v["amount"]), amount);
    assert!(u(&v["fee"]) > 0);
    let slate_id = s(&v["slate_id"]).to_owned();
    let excess = s(&v["kernel_excess"]).to_owned();
    assert_eq!(mine(alice, 1), 7);

    assert_eq!(u(&balance(bob, true)["amount_currently_spendable"]), amount);
    let a = entry(&txs(alice, true), &slate_id);
    assert_eq!(a["confirmed"], true);
    assert_eq!(a["slate_state"], "S3");
    assert_eq!(a["kernel_excess"], excess.as_str());
    assert_eq!(a["counterparty_address"], bob_addr.as_str());
    assert!(a["armored_slatepack"].is_null());
    let proof = call!(grin_payment_proof_export(alice, -1, cs(&slate_id).as_ptr())).unwrap();
    let v = call!(grin_payment_proof_verify(
        bob,
        cs(&proof.to_string()).as_ptr(),
    ))
    .unwrap();
    assert_eq!(v["is_recipient"], true);
    close(alice);
    close(bob);
}

#[test]
fn argument_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let (alice, bob) = alice_and_bob(tmp.path(), 5);

    // Bad armor and wrong states.
    let bad = "BEGINSLATEPACK. 4H1qx1wHe668tFW yC2gfL8PPd8kSgv. ENDSLATEPACK.";
    assert_eq!(code_of(receive(bob, bad)), SLATE);
    assert_eq!(code_of(decode(0, bad)), SLATE);
    assert_eq!(code_of(decode(bob, "")), SLATE);
    let s1 = init_send(alice, json!({"amount": GRIN, "min_confirms": 1})).unwrap();
    let e = finalize(alice, &armored(&s1), false).unwrap_err();
    assert_eq!(e.0, SLATE);
    assert!(s(&e.1["message"]).contains("expected an S2"), "{}", e.1);
    let e = call!(grin_tx_process_invoice(
        bob,
        cs(&armored(&s1)).as_ptr(),
        cs("{}").as_ptr(),
    ))
    .unwrap_err();
    assert_eq!(e.0, SLATE);
    assert!(s(&e.1["message"]).contains("expected an I1"), "{}", e.1);
    let e = call!(grin_tx_finalize_invoice(
        bob,
        cs(&armored(&s1)).as_ptr(),
        false,
        false,
    ))
    .unwrap_err();
    assert!(s(&e.1["message"]).contains("expected an I2"), "{}", e.1);
    call!(grin_tx_cancel(alice, -1, cs(s(&s1["slate_id"])).as_ptr())).unwrap();

    // tx_id -1 with no slate id.
    assert_eq!(
        code_of(call!(grin_tx_repost(alice, -1, ptr::null(), false))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        code_of(call!(grin_tx_cancel(alice, -1, cs("").as_ptr()))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        code_of(call!(grin_payment_proof_export(alice, -1, ptr::null()))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        code_of(call!(grin_tx_cancel(alice, -1, cs("nope").as_ptr()))),
        INVALID_ARGUMENT
    );

    // Arguments JSON.
    assert_eq!(
        code_of(call!(grin_tx_init_send(alice, ptr::null()))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        code_of(call!(grin_tx_init_send(alice, cs("{").as_ptr()))),
        JSON
    );
    assert_eq!(code_of(init_send(alice, json!({"min_confirms": 1}))), JSON);
    // null takes the default for every optional key: 10 confirmations, which
    // nothing on this 5-block chain has.
    let e = init_send(
        alice,
        json!({"amount": GRIN, "min_confirms": null, "selection_strategy_is_all": null,
               "amount_includes_fee": null, "recipient_address": null, "ttl_blocks": null,
               "late_lock": null}),
    )
    .unwrap_err();
    assert_eq!(e.0, INSUFFICIENT_FUNDS, "{}", e.1);
    let i1 = call!(grin_tx_issue_invoice(
        bob,
        cs(r#"{"amount": 1000000000, "dest_acct_name": null, "recipient_address": null}"#).as_ptr(),
    ))
    .unwrap();
    let e = call!(grin_tx_process_invoice(
        alice,
        cs(&armored(&i1)).as_ptr(),
        cs(r#"{"min_confirms": null, "selection_strategy_is_all": null, "ttl_blocks": null}"#)
            .as_ptr(),
    ))
    .unwrap_err();
    assert_eq!(e.0, INSUFFICIENT_FUNDS, "{}", e.1);
    assert_eq!(code_of(init_send(alice, json!({"amount": "1"}))), JSON);
    assert_eq!(
        code_of(init_send(alice, json!({"amount": 0}))),
        INVALID_ARGUMENT
    );
    // Values libwallet would overflow on are refused before anything is
    // built or locked.
    for args in [
        json!({"amount": u64::MAX, "min_confirms": 1}),
        json!({"amount": GRIN, "min_confirms": 1, "ttl_blocks": u64::MAX}),
    ] {
        assert_eq!(code_of(init_send(alice, args.clone())), INVALID_ARGUMENT);
        let mut http = args;
        http["recipient_url_or_address"] = json!("http://127.0.0.1:1");
        assert_eq!(code_of(send_http(alice, http)), INVALID_ARGUMENT);
    }
    assert_eq!(
        code_of(call!(grin_tx_issue_invoice(
            bob,
            cs(&json!({ "amount": u64::MAX }).to_string()).as_ptr()
        ))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        code_of(call!(grin_tx_process_invoice(
            alice,
            cs(&armored(&i1)).as_ptr(),
            cs(&json!({"min_confirms": 1, "ttl_blocks": u64::MAX}).to_string()).as_ptr(),
        ))),
        INVALID_ARGUMENT
    );
    // Bob's key as a mainnet address is refused by a test chain wallet.
    let onion = s(&call!(grin_wallet_get_address(bob, 0)).unwrap()["onion_v3_url"]).to_owned();
    let mainnet = call!(grin_address_validate(cs(&onion).as_ptr(), 0)).unwrap();
    let mainnet = s(&mainnet["slatepack_address"]).to_owned();
    assert!(mainnet.starts_with("grin1"), "{mainnet}");
    assert_eq!(
        code_of(init_send(
            alice,
            json!({"amount": GRIN, "recipient_address": mainnet})
        )),
        INVALID_ADDRESS
    );
    assert_eq!(
        code_of(call!(grin_tx_issue_invoice(
            bob,
            cs(r#"{"amount": 0}"#).as_ptr()
        ))),
        INVALID_ARGUMENT
    );
    assert_eq!(
        code_of(call!(grin_payment_proof_verify(alice, cs("[]").as_ptr()))),
        JSON
    );

    // Not enough funds, with the amounts in the details.
    let e = call!(grin_tx_estimate_fee(alice, 1_000 * GRIN, 1, false, false)).unwrap_err();
    assert_eq!(e.0, INSUFFICIENT_FUNDS);
    assert_eq!(u(&e.1["details"]["available"]), 2 * REWARD);
    assert!(u(&e.1["details"]["required"]) > 1_000 * GRIN);
    assert_eq!(
        code_of(call!(grin_tx_estimate_fee(alice, 0, 1, false, false))),
        INVALID_ARGUMENT
    );
    // With the fee taken out of it, one nanogrin cannot pay the fee.
    assert_eq!(
        code_of(call!(grin_tx_estimate_fee(alice, 1, 1, false, true))),
        TRANSACTION
    );
    // Nothing is locked by an estimate.
    assert_eq!(u(&balance(alice, false)["amount_locked"]), 0);

    // Unknown handles.
    let args = cs(r#"{"amount": 1}"#);
    for h in [0u64, 987_654] {
        assert_eq!(code_of(call!(grin_txs_get(h, false))), INVALID_HANDLE);
        assert_eq!(
            code_of(call!(grin_tx_estimate_fee(h, 1, 1, false, false))),
            INVALID_HANDLE
        );
        assert_eq!(
            code_of(call!(grin_tx_init_send(h, args.as_ptr()))),
            INVALID_HANDLE
        );
        assert_eq!(
            code_of(call!(grin_tx_send_http(h, args.as_ptr()))),
            INVALID_HANDLE
        );
        assert_eq!(
            code_of(call!(grin_tx_issue_invoice(h, args.as_ptr()))),
            INVALID_HANDLE
        );
        assert_eq!(
            code_of(call!(grin_tx_cancel(h, 1, ptr::null()))),
            INVALID_HANDLE
        );
        assert_eq!(
            code_of(call!(grin_tx_repost(h, 1, ptr::null(), false))),
            INVALID_HANDLE
        );
        assert_eq!(
            code_of(call!(grin_payment_proof_export(h, 1, ptr::null()))),
            INVALID_HANDLE
        );
        assert_eq!(
            code_of(call!(grin_payment_proof_verify(h, args.as_ptr()))),
            INVALID_HANDLE
        );
    }
    // Handle 0 means "no wallet" only for decode.
    assert_eq!(
        code_of(call!(grin_slatepack_decode(
            987_654,
            AUTOMATED,
            cs(bad).as_ptr(),
            0
        ))),
        INVALID_HANDLE
    );
    assert_eq!(
        code_of(call!(grin_slatepack_decode(0, 9, cs(bad).as_ptr(), 0))),
        INVALID_ARGUMENT
    );
    close(alice);
    close(bob);
}
