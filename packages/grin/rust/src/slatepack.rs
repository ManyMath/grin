//! Slatepack decoding, plus the slate helpers the transaction functions
//! share: state names, address strings, the final kernel excess and the V4
//! slate JSON.

use std::convert::TryFrom;
use std::os::raw::c_char;

use grin_core::global;
use grin_util::ToHex;
use grin_wallet_libwallet::api_impl::owner;
use grin_wallet_libwallet::{
    Slate, SlateState, SlateVersion, Slatepack, SlatepackAddress, Slatepacker, SlatepackerArgs,
    VersionedSlate,
};
use serde::Serialize;

use crate::config::chain_type_from_u8;
use crate::error::{read_str, to_json, GrinError, GrinErrorCode, GrinFfiResult};
use crate::handle::{get_session, WalletSession};

/// Slatepack delivery mode for an age-encrypted payload.
const MODE_ENCRYPTED: u8 = 1;

pub(crate) fn slate_error(message: impl Into<String>) -> GrinError {
    GrinError::new(GrinErrorCode::Slate, message)
}

/// Name of a slate state in JSON. libwallet prints `Unknown` as "UN"; we
/// spell it out.
pub(crate) fn state_name(state: &SlateState) -> &'static str {
    match state {
        SlateState::Unknown => "Unknown",
        SlateState::Standard1 => "S1",
        SlateState::Standard2 => "S2",
        SlateState::Standard3 => "S3",
        SlateState::Invoice1 => "I1",
        SlateState::Invoice2 => "I2",
        SlateState::Invoice3 => "I3",
    }
}

/// Fails with Slate unless `slate` is in `expected`.
pub(crate) fn require_state(slate: &Slate, expected: SlateState) -> Result<(), GrinError> {
    if slate.state == expected {
        Ok(())
    } else {
        Err(slate_error(format!(
            "expected an {} slatepack, got {}",
            state_name(&expected),
            state_name(&slate.state)
        )))
    }
}

/// bech32 form of a slatepack address, with the HRP it was built with.
pub(crate) fn address_string(addr: &SlatepackAddress) -> Result<String, GrinError> {
    String::try_from(addr).map_err(GrinError::from)
}

/// Parses a slatepack address for the thread's chain type.
pub(crate) fn parse_slatepack_address(input: &str) -> Result<SlatepackAddress, GrinError> {
    SlatepackAddress::try_from(input.trim()).map_err(|e| {
        GrinError::new(
            GrinErrorCode::InvalidAddress,
            format!("not a slatepack address for this network: {e}"),
        )
    })
}

/// Hex of the final kernel excess. Only S3 and I3 carry one: a slate on an
/// earlier leg has at most one party's public excess in its kernel.
pub(crate) fn kernel_excess_hex(slate: &Slate) -> Option<String> {
    if !matches!(slate.state, SlateState::Standard3 | SlateState::Invoice3) {
        return None;
    }
    let excess = slate.tx.as_ref()?.kernels().first()?.excess;
    if excess.0.iter().all(|b| *b == 0) {
        None
    } else {
        Some(excess.0.to_hex())
    }
}

/// Fee of a slate: the fee fields, or the kernel fees once libwallet has
/// cleared the fee fields on the return journey.
pub(crate) fn slate_fee(slate: &Slate) -> u64 {
    let fee = slate.fee_fields.fee();
    if fee != 0 {
        return fee;
    }
    slate.tx.as_ref().map(|tx| tx.fee()).unwrap_or(0)
}

/// The slate as V4 JSON, the format foreign APIs exchange.
pub(crate) fn slate_json_v4(slate: &Slate) -> Result<String, GrinError> {
    let versioned = VersionedSlate::into_version(slate.clone(), SlateVersion::V4)?;
    Ok(serde_json::to_string(&versioned)?)
}

fn plain_packer() -> Slatepacker<'static> {
    Slatepacker::new(SlatepackerArgs {
        sender: None,
        recipients: vec![],
        dec_key: None,
    })
}

/// A Slatepack after armor parsing and, if needed, decryption.
pub(crate) struct DecodedSlatepack {
    pub slate: Slate,
    pub sender: Option<SlatepackAddress>,
    pub recipients: Vec<SlatepackAddress>,
    pub is_encrypted: bool,
}

/// Parses armor (or a raw binary or JSON slatepack) without decrypting.
/// The thread's chain type must be set: the size limit depends on it.
fn parse_pack(armored: &str) -> Result<Slatepack, GrinError> {
    plain_packer()
        .deser_slatepack(armored.trim().as_bytes(), false)
        .map_err(|e| slate_error(format!("not a valid slatepack: {e}")))
}

fn finish(pack: Slatepack, is_encrypted: bool) -> Result<DecodedSlatepack, GrinError> {
    let slate = plain_packer()
        .get_slate(&pack)
        .map_err(|e| slate_error(format!("the slatepack does not hold a valid slate: {e}")))?;
    Ok(DecodedSlatepack {
        slate,
        sender: pack.sender.clone(),
        recipients: pack.recipients().to_vec(),
        is_encrypted,
    })
}

/// Decodes a Slatepack, decrypting it with the wallet's address key at
/// `secret_index` when it is encrypted.
pub(crate) fn decode_with_wallet(
    session: &WalletSession,
    armored: &str,
    secret_index: u32,
) -> Result<DecodedSlatepack, GrinError> {
    let pack = parse_pack(armored)?;
    if pack.mode != MODE_ENCRYPTED {
        return finish(pack, false);
    }
    let mask = session.mask();
    // Returns the pack still encrypted when the key does not fit.
    let pack = owner::decode_slatepack_message(
        session.wallet.clone(),
        mask.as_ref(),
        armored.trim().to_owned(),
        vec![secret_index],
    )?;
    if pack.mode == MODE_ENCRYPTED {
        return Err(slate_error(format!(
            "the slatepack is encrypted to another address; this wallet's key at index {secret_index} cannot decrypt it"
        )));
    }
    finish(pack, true)
}

/// Decodes an unencrypted Slatepack without a wallet. The caller has set the
/// thread's chain type.
pub(crate) fn decode_plain(armored: &str) -> Result<DecodedSlatepack, GrinError> {
    let pack = parse_pack(armored)?;
    if pack.mode == MODE_ENCRYPTED {
        return Err(slate_error("encrypted slatepack needs an open wallet"));
    }
    finish(pack, false)
}

#[derive(Serialize)]
struct DecodeOut {
    slate_id: String,
    state: &'static str,
    amount: u64,
    fee: u64,
    kernel_excess: Option<String>,
    sender: Option<String>,
    recipients: Vec<String>,
    ttl_cutoff_height: Option<u64>,
    num_participants: u8,
    is_encrypted: bool,
    slate_json: String,
}

/// Decodes a plain or encrypted Slatepack and returns its slate metadata.
/// `network` is used when `handle` is 0.
// # Safety
// Every pointer argument must be NULL or point to a NUL-terminated string
// that stays valid and unchanged until the call returns.
#[no_mangle]
pub unsafe extern "C" fn grin_slatepack_decode(
    handle: u64,
    network: u8,
    armored_slatepack: *const c_char,
    secret_index: u32,
) -> GrinFfiResult {
    ffi_guard! {
        let decoded = if handle == 0 {
            global::set_local_chain_type(chain_type_from_u8(network)?);
            decode_plain(read_str(&armored_slatepack, "armored_slatepack")?)?
        } else {
            let session = get_session(handle)?;
            let armored = read_str(&armored_slatepack, "armored_slatepack")?;
            decode_with_wallet(&session, armored, secret_index)?
        };
        let slate = &decoded.slate;
        to_json(&DecodeOut {
            slate_id: slate.id.to_string(),
            state: state_name(&slate.state),
            amount: slate.amount,
            fee: slate_fee(slate),
            kernel_excess: kernel_excess_hex(slate),
            sender: decoded.sender.as_ref().map(address_string).transpose()?,
            recipients: decoded
                .recipients
                .iter()
                .map(address_string)
                .collect::<Result<_, _>>()?,
            ttl_cutoff_height: match slate.ttl_cutoff_height {
                0 => None,
                h => Some(h),
            },
            num_participants: slate.num_participants(),
            is_encrypted: decoded.is_encrypted,
            slate_json: slate_json_v4(slate)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grin_core::global::ChainTypes;

    #[test]
    fn state_names_and_checks() {
        assert_eq!(state_name(&SlateState::Unknown), "Unknown");
        assert_eq!(state_name(&SlateState::Invoice2), "I2");
        let slate = Slate::blank(2, false);
        assert!(require_state(&slate, SlateState::Standard1).is_ok());
        let e = require_state(&slate, SlateState::Standard2).unwrap_err();
        assert_eq!(e.code, GrinErrorCode::Slate);
        assert!(e.message.contains("expected an S2"), "{}", e.message);
        assert!(kernel_excess_hex(&slate).is_none());
    }

    #[test]
    fn plain_round_trip_without_wallet() {
        global::set_local_chain_type(ChainTypes::AutomatedTesting);
        let mut slate = Slate::blank(2, false);
        slate.amount = 42;
        let pack = plain_packer().create_slatepack(&slate).unwrap();
        let armored = plain_packer().armor_slatepack(&pack).unwrap();
        let d = decode_plain(&format!("  {armored}\n")).unwrap();
        assert_eq!(d.slate.id, slate.id);
        assert_eq!(d.slate.amount, 42);
        assert!(!d.is_encrypted);
        assert!(d.sender.is_none());
        let json = slate_json_v4(&d.slate).unwrap();
        let back = Slate::deserialize_upgrade(&json).unwrap();
        assert_eq!(back.id, slate.id);

        assert_eq!(
            decode_plain("BEGINSLATEPACK. nope. ENDSLATEPACK.")
                .err()
                .map(|e| e.code),
            Some(GrinErrorCode::Slate)
        );
        assert_eq!(
            decode_plain("x").err().map(|e| e.code),
            Some(GrinErrorCode::Slate)
        );
    }
}
