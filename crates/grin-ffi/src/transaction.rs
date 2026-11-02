//! Transaction entry points. The bodies arrive in Phase 1b; until then each
//! returns NotSupported. Signatures are final (docs/architecture.md 4.6).

use std::os::raw::c_char;

use crate::error::{GrinError, GrinFfiResult};

/// Returns `{"fee", "net_amount", "inputs_count"}` or InsufficientFunds.
#[no_mangle]
pub extern "C" fn grin_tx_estimate_fee(
    handle: u64,
    amount: u64,
    min_confirms: u64,
    selection_strategy_is_all: bool,
    amount_includes_fee: bool,
) -> GrinFfiResult {
    ffi_guard! {
        let _ = (handle, amount, min_confirms, selection_strategy_is_all, amount_includes_fee);
        Err(GrinError::not_supported())
    }
}

/// Lists transactions joined with the sidecar metadata.
#[no_mangle]
pub extern "C" fn grin_txs_get(handle: u64, refresh_from_node: bool) -> GrinFfiResult {
    ffi_guard! {
        let _ = (handle, refresh_from_node);
        Err(GrinError::not_supported())
    }
}

/// Builds and locks an S1 and returns it as a Slatepack.
#[no_mangle]
pub extern "C" fn grin_tx_init_send(handle: u64, args_json: *const c_char) -> GrinFfiResult {
    ffi_guard! {
        let _ = (handle, args_json);
        Err(GrinError::not_supported())
    }
}

/// Synchronous send to a slatepack address (over Tor) or an http(s) URL.
#[no_mangle]
pub extern "C" fn grin_tx_send_http(handle: u64, args_json: *const c_char) -> GrinFfiResult {
    ffi_guard! {
        let _ = (handle, args_json);
        Err(GrinError::not_supported())
    }
}

/// Receives an S1 Slatepack and returns the S2 Slatepack.
#[no_mangle]
pub extern "C" fn grin_tx_receive_slatepack(
    handle: u64,
    armored_s1: *const c_char,
    message: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let _ = (handle, armored_s1, message);
        Err(GrinError::not_supported())
    }
}

/// Finalizes an S2 Slatepack into S3, optionally broadcasting it.
#[no_mangle]
pub extern "C" fn grin_tx_finalize_slatepack(
    handle: u64,
    armored_s2: *const c_char,
    broadcast: bool,
    fluff: bool,
) -> GrinFfiResult {
    ffi_guard! {
        let _ = (handle, armored_s2, broadcast, fluff);
        Err(GrinError::not_supported())
    }
}

/// Issues an I1 invoice Slatepack.
#[no_mangle]
pub extern "C" fn grin_tx_issue_invoice(handle: u64, args_json: *const c_char) -> GrinFfiResult {
    ffi_guard! {
        let _ = (handle, args_json);
        Err(GrinError::not_supported())
    }
}

/// Pays an I1 invoice, locks the inputs and returns the I2 Slatepack.
#[no_mangle]
pub extern "C" fn grin_tx_process_invoice(
    handle: u64,
    armored_i1: *const c_char,
    args_json: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let _ = (handle, armored_i1, args_json);
        Err(GrinError::not_supported())
    }
}

/// Finalizes an I2 Slatepack into I3, optionally broadcasting it.
#[no_mangle]
pub extern "C" fn grin_tx_finalize_invoice(
    handle: u64,
    armored_i2: *const c_char,
    broadcast: bool,
    fluff: bool,
) -> GrinFfiResult {
    ffi_guard! {
        let _ = (handle, armored_i2, broadcast, fluff);
        Err(GrinError::not_supported())
    }
}

/// Re-broadcasts a finalized, unconfirmed transaction.
#[no_mangle]
pub extern "C" fn grin_tx_repost(
    handle: u64,
    tx_id: i64,
    slate_id: *const c_char,
    fluff: bool,
) -> GrinFfiResult {
    ffi_guard! {
        let _ = (handle, tx_id, slate_id, fluff);
        Err(GrinError::not_supported())
    }
}

/// Cancels a transaction by id or slate id and unlocks its inputs.
#[no_mangle]
pub extern "C" fn grin_tx_cancel(
    handle: u64,
    tx_id: i64,
    slate_id: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let _ = (handle, tx_id, slate_id);
        Err(GrinError::not_supported())
    }
}
