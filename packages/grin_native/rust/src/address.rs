//! Slatepack addresses (grin1/tgrin1), onion v3 addresses and HTTP URLs.

use std::convert::TryFrom;
use std::os::raw::c_char;

use grin_core::global;
use grin_wallet_libwallet::api_impl::owner;
use grin_wallet_libwallet::SlatepackAddress;
use grin_wallet_util::OnionV3Address;
use serde::Serialize;

use crate::config::chain_type_from_u8;
use crate::error::{read_str, to_json, GrinError, GrinErrorCode, GrinFfiResult};
use crate::handle::get_session;

#[derive(Serialize)]
struct WalletAddressOut {
    slatepack_address: String,
    onion_v3_url: String,
    index: u32,
}

/// Result of `grin_address_validate`.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ParsedAddress {
    pub kind: &'static str,
    pub slatepack_address: Option<String>,
    pub onion_v3_url: Option<String>,
    /// Base URL a synchronous send posts to (without `/v2/foreign`).
    pub url: String,
}

fn invalid_address(message: impl Into<String>) -> GrinError {
    GrinError::new(GrinErrorCode::InvalidAddress, message)
}

fn slatepack_string(addr: &SlatepackAddress) -> Result<String, GrinError> {
    String::try_from(addr).map_err(GrinError::from)
}

/// Strips a trailing `/v2/foreign` and slashes so the result is a base URL.
fn trim_base(path: &str) -> &str {
    let p = path.trim_end_matches('/');
    p.strip_suffix("/v2/foreign")
        .unwrap_or(p)
        .trim_end_matches('/')
}

/// Parses an onion host label (without `.onion`) and builds the result. The
/// caller has set the thread's chain type, which picks the slatepack HRP.
fn parse_onion(label: &str, scheme: &str) -> Result<ParsedAddress, GrinError> {
    if label.len() != 56 {
        return Err(invalid_address(
            "onion v3 addresses have 56 characters before .onion",
        ));
    }
    let onion = OnionV3Address::try_from(label)
        .map_err(|e| invalid_address(format!("not a valid onion v3 address: {e}")))?;
    let slatepack = SlatepackAddress::try_from(onion.clone())?;
    let ov3 = onion.to_http_str();
    let url = format!("{scheme}://{}.onion", onion.to_ov3_str());
    Ok(ParsedAddress {
        kind: "onion",
        slatepack_address: Some(slatepack_string(&slatepack)?),
        onion_v3_url: Some(ov3),
        url,
    })
}

/// Classifies and normalizes an address on the thread's current chain type.
pub fn parse_address(input: &str) -> Result<ParsedAddress, GrinError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(invalid_address("address is empty"));
    }
    let lower = s.to_ascii_lowercase();

    if lower.starts_with("grin1") || lower.starts_with("tgrin1") {
        let addr = SlatepackAddress::try_from(s).map_err(|e| invalid_address(e.to_string()))?;
        let ov3 = OnionV3Address::from(&addr).to_http_str();
        return Ok(ParsedAddress {
            kind: "slatepack",
            slatepack_address: Some(slatepack_string(&addr)?),
            onion_v3_url: Some(ov3.clone()),
            url: ov3,
        });
    }

    if lower.starts_with("http://") || lower.starts_with("https://") {
        let parsed = url::Url::parse(s).map_err(|e| invalid_address(format!("bad URL: {e}")))?;
        // A fully qualified name may end in a dot. Without it an onion host
        // written that way would be taken for a plain HTTP one.
        let host = parsed
            .host_str()
            .map(|h| h.trim_end_matches('.'))
            .filter(|h| !h.is_empty())
            .ok_or_else(|| invalid_address("URL has no host"))?;
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(invalid_address("URL must not have a query or fragment"));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(invalid_address("URL must not contain credentials"));
        }
        if let Some(label) = host.strip_suffix(".onion") {
            if parsed.port().is_some() || !trim_base(parsed.path()).is_empty() {
                return Err(invalid_address(
                    "onion URLs must not carry a port or a path other than /v2/foreign",
                ));
            }
            return parse_onion(label, parsed.scheme());
        }
        let mut base = format!("{}://{}", parsed.scheme(), host);
        if let Some(port) = parsed.port() {
            base.push_str(&format!(":{port}"));
        }
        base.push_str(trim_base(parsed.path()));
        return Ok(ParsedAddress {
            kind: "http",
            slatepack_address: None,
            onion_v3_url: None,
            url: base,
        });
    }

    let bare = trim_base(&lower).trim_end_matches('.');
    if let Some(label) = bare.strip_suffix(".onion") {
        return parse_onion(label, "http");
    }
    if bare.len() == 56 && !bare.contains(['.', '/', ':']) {
        return parse_onion(bare, "http");
    }
    Err(invalid_address(
        "expected a grin1/tgrin1 slatepack address, a v3 .onion address or an http(s) URL",
    ))
}

/// Validates `address` for `network` (0 mainnet, 1 testnet, 2 user testing,
/// 3 automated testing) and returns
/// `{"kind", "slatepack_address", "onion_v3_url", "url"}`.
// # Safety
// Every pointer argument must be NULL or point to a NUL-terminated string
// that stays valid and unchanged until the call returns.
#[no_mangle]
pub unsafe extern "C" fn grin_address_validate(
    address: *const c_char,
    network: u8,
) -> GrinFfiResult {
    ffi_guard! {
        let chain_type = chain_type_from_u8(network)?;
        global::set_local_chain_type(chain_type);
        let address = read_str(&address, "address")?;
        to_json(&parse_address(address)?)
    }
}

/// Returns `{"slatepack_address", "onion_v3_url", "index"}` for the
/// wallet's address at `derivation_index`.
#[no_mangle]
pub extern "C" fn grin_wallet_get_address(handle: u64, derivation_index: u32) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        let mask = session.mask();
        let addr =
            owner::get_slatepack_address(session.wallet.clone(), mask.as_ref(), derivation_index)?;
        to_json(&WalletAddressOut {
            slatepack_address: slatepack_string(&addr)?,
            onion_v3_url: OnionV3Address::from(&addr).to_http_str(),
            index: derivation_index,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grin_core::global::ChainTypes;

    const TGRIN: &str = "tgrin1xtxavwfgs48ckf3gk8wwgcndmn0nt4tvkl8a7ltyejjcy2mc6nfs9gm2lp";

    #[test]
    fn slatepack_networks() {
        global::set_local_chain_type(ChainTypes::AutomatedTesting);
        let p = parse_address(TGRIN).unwrap();
        assert_eq!(p.kind, "slatepack");
        assert_eq!(p.slatepack_address.as_deref(), Some(TGRIN));
        let ov3 = p.onion_v3_url.clone().unwrap();
        assert!(ov3.starts_with("http://") && ov3.ends_with(".onion"));
        assert_eq!(p.url, ov3);

        // The onion form maps back to the same key.
        let back = parse_address(&ov3).unwrap();
        assert_eq!(back.kind, "onion");
        assert_eq!(back.slatepack_address.as_deref(), Some(TGRIN));
        let label = ov3.trim_start_matches("http://");
        assert_eq!(
            parse_address(label).unwrap().slatepack_address,
            back.slatepack_address
        );
        assert_eq!(
            parse_address(&format!("{ov3}/v2/foreign/")).unwrap().url,
            ov3
        );
        // A trailing dot on the host does not turn it into a plain URL.
        for dotted in [
            format!("{ov3}."),
            format!("{ov3}./v2/foreign"),
            format!("{label}."),
        ] {
            let p = parse_address(&dotted).unwrap();
            assert_eq!(p.kind, "onion", "{dotted}");
            assert_eq!(p.url, ov3);
            assert_eq!(p.slatepack_address.as_deref(), Some(TGRIN));
        }

        global::set_local_chain_type(ChainTypes::Mainnet);
        let e = parse_address(TGRIN).unwrap_err();
        assert_eq!(e.code, GrinErrorCode::InvalidAddress);
        let main = parse_address(&ov3).unwrap();
        assert!(main.slatepack_address.unwrap().starts_with("grin1"));
    }

    #[test]
    fn http_urls() {
        global::set_local_chain_type(ChainTypes::Mainnet);
        let p = parse_address("https://Example.com:3415/v2/foreign").unwrap();
        assert_eq!(p.kind, "http");
        assert_eq!(p.url, "https://example.com:3415");
        assert!(p.slatepack_address.is_none());
        assert!(parse_address("http://").is_err());
        assert!(parse_address("ftp://example.com").is_err());
        assert!(parse_address("hello").is_err());
        assert!(parse_address("http://abc.onion").is_err());
        assert!(parse_address("http://abc.onion.").is_err());
        assert_eq!(
            parse_address("http://example.com./").unwrap().url,
            "http://example.com"
        );
        assert!(parse_address("http://./").is_err());
    }
}
