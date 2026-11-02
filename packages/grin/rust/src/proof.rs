//! Payment proofs: export in libwallet's own `PaymentProof` JSON and verify
//! that JSON again.

use std::os::raw::c_char;

use grin_wallet_libwallet::api_impl::owner;
use grin_wallet_libwallet::PaymentProof;
use serde::Serialize;
use serde_json::Value;

use crate::error::{read_str, to_json, GrinFfiResult};
use crate::handle::get_session;
use crate::transaction::tx_selector;

#[derive(Serialize)]
struct ExportOut {
    proof: PaymentProof,
}

#[derive(Serialize)]
struct VerifyOut {
    is_sender: bool,
    is_recipient: bool,
}

/// Exports the payment proof of a transaction by id or slate id.
#[no_mangle]
pub extern "C" fn grin_payment_proof_export(
    handle: u64,
    tx_id: i64,
    slate_id: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        let (tx_id, slate_id) = tx_selector(tx_id, slate_id)?;
        let mask = session.mask();
        let proof = owner::retrieve_payment_proof(
            session.wallet.clone(),
            mask.as_ref(),
            &None,
            false,
            tx_id,
            slate_id,
        )?;
        to_json(&ExportOut { proof })
    }
}

/// Verifies a payment proof and returns `{"is_sender", "is_recipient"}`.
#[no_mangle]
pub extern "C" fn grin_payment_proof_verify(
    handle: u64,
    proof_json: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        let mut value: Value = serde_json::from_str(read_str(proof_json, "proof_json")?)?;
        // Accept the export wrapper `{"proof": {...}}` as well as the bare
        // proof object.
        if let Some(inner) = value.get_mut("proof").filter(|p| p.is_object()) {
            value = inner.take();
        }
        // Addresses are checked against the wallet's network while parsing.
        let proof: PaymentProof = serde_json::from_value(value)?;
        let mask = session.mask();
        let (is_sender, is_recipient) =
            owner::verify_payment_proof(session.wallet.clone(), mask.as_ref(), &proof)?;
        to_json(&VerifyOut {
            is_sender,
            is_recipient,
        })
    }
}
