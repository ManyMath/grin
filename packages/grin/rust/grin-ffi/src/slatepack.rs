//! Slatepack decoding. The body arrives in Phase 1b; until then it returns
//! NotSupported. The signature is final (docs/architecture.md 4.6).

use std::os::raw::c_char;

use crate::error::{GrinError, GrinFfiResult};

/// Decodes a plain or encrypted Slatepack and returns its slate metadata.
/// `network` is used when `handle` is 0.
#[no_mangle]
pub extern "C" fn grin_slatepack_decode(
    handle: u64,
    network: u8,
    armored_slatepack: *const c_char,
    secret_index: u32,
) -> GrinFfiResult {
    ffi_guard! {
        let _ = (handle, network, armored_slatepack, secret_index);
        Err(GrinError::not_supported())
    }
}
