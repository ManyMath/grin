//! Wallet, node and SOCKS5 proxy configuration, plus the node client type
//! every wallet in this crate is built with.

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use grin_core::core::{Transaction, TxKernel};
use grin_core::global::ChainTypes;
use grin_util::secp::pedersen;
use grin_wallet_impls::HTTPNodeClient;
use grin_wallet_libwallet::{Error as LibwalletError, NodeClient, NodeVersionInfo};
use serde::Deserialize;

use crate::error::{GrinError, GrinErrorCode};

/// Request timeout used when the caller does not give one.
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

/// reqwest proxy scheme. `socks5h` resolves names (including .onion) on the
/// proxy side, so no DNS query leaves the device.
const SOCKS_SCHEME: &str = "socks5h://";

/// Maps the FFI `network: u8` to a chain type. The numbering is our own and
/// does not follow the order of `ChainTypes`.
pub fn chain_type_from_u8(network: u8) -> Result<ChainTypes, GrinError> {
    match network {
        0 => Ok(ChainTypes::Mainnet),
        1 => Ok(ChainTypes::Testnet),
        2 => Ok(ChainTypes::UserTesting),
        3 => Ok(ChainTypes::AutomatedTesting),
        n => Err(GrinError::invalid_argument(format!(
            "network must be 0 (mainnet), 1 (testnet), 2 (user testing) or 3 (automated testing), got {n}"
        ))),
    }
}

/// `config_json` accepted by `grin_wallet_create` and `grin_wallet_open`.
/// Unknown keys are ignored so newer callers can add fields.
#[derive(Deserialize)]
pub struct WalletConfigJson {
    pub wallet_dir: String,
    pub network: u8,
    pub node_url: String,
    #[serde(default)]
    pub node_api_secret: Option<String>,
    #[serde(default)]
    pub socks_proxy: Option<String>,
    #[serde(default)]
    pub request_timeout_secs: Option<u64>,
}

/// Shown in place of the node API secret by the `Debug` impls.
const REDACTED: &str = "<redacted>";

impl fmt::Debug for WalletConfigJson {
    // Written by hand so the node API secret never reaches a log.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WalletConfigJson")
            .field("wallet_dir", &self.wallet_dir)
            .field("network", &self.network)
            .field("node_url", &self.node_url)
            .field(
                "node_api_secret",
                &self.node_api_secret.as_ref().map(|_| REDACTED),
            )
            .field("socks_proxy", &self.socks_proxy)
            .field("request_timeout_secs", &self.request_timeout_secs)
            .finish()
    }
}

/// Validated form of `WalletConfigJson`.
#[derive(Debug)]
pub struct WalletConfig {
    pub wallet_dir: String,
    pub chain_type: ChainTypes,
    pub node: NodeSettings,
}

impl WalletConfig {
    pub fn parse(json: &str) -> Result<Self, GrinError> {
        let raw: WalletConfigJson = serde_json::from_str(json)?;
        let chain_type = chain_type_from_u8(raw.network)?;
        if raw.wallet_dir.trim().is_empty() {
            return Err(GrinError::invalid_argument("wallet_dir must not be empty"));
        }
        let timeout = match raw.request_timeout_secs {
            None => DEFAULT_REQUEST_TIMEOUT_SECS,
            Some(0) => {
                return Err(GrinError::invalid_argument(
                    "request_timeout_secs must be greater than zero",
                ))
            }
            Some(t) => t,
        };
        let node = NodeSettings::new(
            &raw.node_url,
            raw.node_api_secret.as_deref(),
            raw.socks_proxy.as_deref(),
            Duration::from_secs(timeout),
        )?;
        Ok(WalletConfig {
            wallet_dir: raw.wallet_dir,
            chain_type,
            node,
        })
    }
}

/// Node connection settings kept per session.
#[derive(Clone)]
pub struct NodeSettings {
    pub url: String,
    pub api_secret: Option<String>,
    pub socks_proxy: Option<SocketAddr>,
    pub timeout: Duration,
}

impl fmt::Debug for NodeSettings {
    // Written by hand so the node API secret never reaches a log.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeSettings")
            .field("url", &self.url)
            .field("api_secret", &self.api_secret.as_ref().map(|_| REDACTED))
            .field("socks_proxy", &self.socks_proxy)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl NodeSettings {
    /// Validates the URL and proxy. Empty strings count as absent for the
    /// secret and the proxy.
    pub fn new(
        url: &str,
        api_secret: Option<&str>,
        socks_proxy: Option<&str>,
        timeout: Duration,
    ) -> Result<Self, GrinError> {
        let url = validate_node_url(url)?;
        let api_secret = api_secret.filter(|s| !s.is_empty()).map(str::to_owned);
        let socks_proxy = match socks_proxy.map(str::trim).filter(|s| !s.is_empty()) {
            Some(p) => Some(parse_socks_proxy(p)?),
            None => None,
        };
        Ok(NodeSettings {
            url,
            api_secret,
            socks_proxy,
            timeout,
        })
    }

    /// Builds an HTTP node client, routed through the SOCKS proxy if set.
    pub fn http_client(&self) -> Result<HTTPNodeClient, GrinError> {
        let proxy = self.socks_proxy.map(|addr| (addr, SOCKS_SCHEME));
        HTTPNodeClient::new_proxy(&self.url, self.api_secret.clone(), proxy, self.timeout).map_err(
            |e| {
                GrinError::new(
                    GrinErrorCode::Node,
                    format!("could not build the node HTTP client: {e}"),
                )
            },
        )
    }
}

fn validate_node_url(url: &str) -> Result<String, GrinError> {
    let url = url.trim();
    let parsed = url::Url::parse(url)
        .map_err(|e| GrinError::invalid_argument(format!("node_url is not a valid URL: {e}")))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(GrinError::invalid_argument(
            "node_url must start with http:// or https://",
        ));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(GrinError::invalid_argument("node_url has no host"));
    }
    // HTTPNodeClient appends "/v2/foreign" itself.
    Ok(url.trim_end_matches('/').to_owned())
}

/// Parses "host:port" (optionally prefixed with socks5h:// or socks5://).
/// Only IPv4 literals are accepted: a host name would need a local DNS query
/// to turn into the `SocketAddr` HTTPNodeClient wants, and the fork formats
/// the proxy URL without IPv6 brackets.
pub fn parse_socks_proxy(input: &str) -> Result<SocketAddr, GrinError> {
    let rest = input
        .strip_prefix("socks5h://")
        .or_else(|| input.strip_prefix("socks5://"))
        .unwrap_or(input)
        .trim_end_matches('/');
    let addr = SocketAddr::from_str(rest).map_err(|_| {
        GrinError::invalid_argument(format!(
            "socks_proxy must be an IPv4 address and port such as 127.0.0.1:9050, got {input:?}"
        ))
    })?;
    if addr.is_ipv6() {
        return Err(GrinError::invalid_argument(
            "socks_proxy must be an IPv4 address; IPv6 proxies are not supported",
        ));
    }
    if addr.port() == 0 {
        return Err(GrinError::invalid_argument(
            "socks_proxy port must not be 0",
        ));
    }
    Ok(addr)
}

/// The node client inside every wallet. Production wallets talk HTTP; with
/// the `test-utils` feature, test wallets talk to the in-process chain. One
/// enum keeps a single concrete wallet type, so test wallets run through the
/// same session registry and entry points as real ones.
#[derive(Clone)]
pub enum FfiNodeClient {
    Http(HTTPNodeClient),
    #[cfg(feature = "test-utils")]
    Local(grin_wallet_impls::test_framework::LocalWalletClient),
}

macro_rules! delegate {
    ($self:ident, $c:ident => $e:expr) => {
        match $self {
            FfiNodeClient::Http($c) => $e,
            #[cfg(feature = "test-utils")]
            FfiNodeClient::Local($c) => $e,
        }
    };
}

impl NodeClient for FfiNodeClient {
    fn node_url(&self) -> &str {
        delegate!(self, c => c.node_url())
    }

    fn set_node_url(&mut self, node_url: &str) {
        delegate!(self, c => c.set_node_url(node_url))
    }

    fn node_api_secret(&self) -> Option<String> {
        delegate!(self, c => c.node_api_secret())
    }

    fn set_node_api_secret(&mut self, node_api_secret: Option<String>) {
        delegate!(self, c => c.set_node_api_secret(node_api_secret))
    }

    fn post_tx(&self, tx: &Transaction, fluff: bool) -> Result<(), LibwalletError> {
        delegate!(self, c => c.post_tx(tx, fluff))
    }

    fn get_version_info(&mut self) -> Option<NodeVersionInfo> {
        delegate!(self, c => c.get_version_info())
    }

    fn get_chain_tip(&self) -> Result<(u64, String), LibwalletError> {
        delegate!(self, c => c.get_chain_tip())
    }

    fn get_kernel(
        &mut self,
        excess: &pedersen::Commitment,
        min_height: Option<u64>,
        max_height: Option<u64>,
    ) -> Result<Option<(TxKernel, u64, u64)>, LibwalletError> {
        delegate!(self, c => c.get_kernel(excess, min_height, max_height))
    }

    fn get_outputs_from_node(
        &self,
        wallet_outputs: Vec<pedersen::Commitment>,
    ) -> Result<HashMap<pedersen::Commitment, (String, u64, u64)>, LibwalletError> {
        delegate!(self, c => c.get_outputs_from_node(wallet_outputs))
    }

    #[allow(clippy::type_complexity)]
    fn get_outputs_by_pmmr_index(
        &self,
        start_height: u64,
        end_height: Option<u64>,
        max_outputs: u64,
    ) -> Result<
        (
            u64,
            u64,
            Vec<(pedersen::Commitment, pedersen::RangeProof, bool, u64, u64)>,
        ),
        LibwalletError,
    > {
        delegate!(self, c => c.get_outputs_by_pmmr_index(start_height, end_height, max_outputs))
    }

    fn height_range_to_pmmr_indices(
        &self,
        start_height: u64,
        end_height: Option<u64>,
    ) -> Result<(u64, u64), LibwalletError> {
        delegate!(self, c => c.height_range_to_pmmr_indices(start_height, end_height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_mapping() {
        assert_eq!(chain_type_from_u8(0).unwrap(), ChainTypes::Mainnet);
        assert_eq!(chain_type_from_u8(1).unwrap(), ChainTypes::Testnet);
        assert_eq!(chain_type_from_u8(2).unwrap(), ChainTypes::UserTesting);
        assert_eq!(chain_type_from_u8(3).unwrap(), ChainTypes::AutomatedTesting);
        assert_eq!(
            chain_type_from_u8(4).unwrap_err().code,
            GrinErrorCode::InvalidArgument
        );
    }

    #[test]
    fn socks_proxy_parsing() {
        assert_eq!(
            parse_socks_proxy("127.0.0.1:9050").unwrap(),
            "127.0.0.1:9050".parse().unwrap()
        );
        assert!(parse_socks_proxy("socks5h://127.0.0.1:9050").is_ok());
        assert!(parse_socks_proxy("localhost:9050").is_err());
        assert!(parse_socks_proxy("[::1]:9050").is_err());
        assert!(parse_socks_proxy("127.0.0.1").is_err());
        assert!(parse_socks_proxy("127.0.0.1:0").is_err());
    }

    #[test]
    fn config_parsing() {
        let c = WalletConfig::parse(
            r#"{"wallet_dir":"/tmp/w","network":1,"node_url":"https://n.example/","socks_proxy":""}"#,
        )
        .unwrap();
        assert_eq!(c.chain_type, ChainTypes::Testnet);
        assert_eq!(c.node.url, "https://n.example");
        assert!(c.node.socks_proxy.is_none());
        assert_eq!(c.node.timeout, Duration::from_secs(30));

        let e = WalletConfig::parse("{").unwrap_err();
        assert_eq!(e.code, GrinErrorCode::Json);
        let e = WalletConfig::parse(r#"{"wallet_dir":"/w","network":9,"node_url":"http://a"}"#)
            .unwrap_err();
        assert_eq!(e.code, GrinErrorCode::InvalidArgument);
        let e = WalletConfig::parse(r#"{"wallet_dir":"/w","network":0,"node_url":"ftp://a"}"#)
            .unwrap_err();
        assert_eq!(e.code, GrinErrorCode::InvalidArgument);
        let e = WalletConfig::parse(
            r#"{"wallet_dir":"/w","network":0,"node_url":"http://a","request_timeout_secs":0}"#,
        )
        .unwrap_err();
        assert_eq!(e.code, GrinErrorCode::InvalidArgument);
    }

    #[test]
    fn debug_never_prints_the_api_secret() {
        let json = r#"{"wallet_dir":"/w","network":0,"node_url":"http://a","node_api_secret":"hunter2-secret"}"#;
        let raw: WalletConfigJson = serde_json::from_str(json).unwrap();
        let c = WalletConfig::parse(json).unwrap();
        assert_eq!(c.node.api_secret.as_deref(), Some("hunter2-secret"));
        for text in [
            format!("{raw:?}"),
            format!("{c:?}"),
            format!("{:?}", c.node),
        ] {
            assert!(!text.contains("hunter2"), "{text}");
            assert!(text.contains(REDACTED), "{text}");
        }
    }
}
