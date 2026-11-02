//! Payment proofs. The bodies arrive in Phase 1b; until then each returns
//! NotSupported. Signatures are final (docs/architecture.md 4.6).

use std::os::raw::c_char;

use crate::error::{GrinError, GrinFfiResult};

/// Exports the payment proof of a transaction by id or slate id.
#[no_mangle]
pub extern "C" fn grin_payment_proof_export(
    handle: u64,
    tx_id: i64,
    slate_id: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let _ = (handle, tx_id, slate_id);
        Err(GrinError::not_supported())
    }
}

/// Verifies a payment proof and returns `{"is_sender", "is_recipient"}`.
#[no_mangle]
pub extern "C" fn grin_payment_proof_verify(
    handle: u64,
    proof_json: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let _ = (handle, proof_json);
        Err(GrinError::not_supported())
    }
}
