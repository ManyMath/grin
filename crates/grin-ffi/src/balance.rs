//! Balances (nanogrin u64), chain tip and the stateless node check.

use std::os::raw::c_char;
use std::time::Duration;

use grin_wallet_libwallet::api_impl::owner;
use grin_wallet_libwallet::NodeClient;
use serde::Serialize;

use crate::config::{NodeSettings, DEFAULT_REQUEST_TIMEOUT_SECS};
use crate::error::{read_opt_nonempty, read_str, to_json, GrinError, GrinErrorCode, GrinFfiResult};
use crate::handle::get_session;

#[derive(Serialize)]
struct BalanceOut {
    last_confirmed_height: u64,
    minimum_confirmations: u64,
    total: u64,
    amount_awaiting_finalization: u64,
    amount_awaiting_confirmation: u64,
    amount_immature: u64,
    amount_currently_spendable: u64,
    amount_locked: u64,
    amount_reverted: u64,
    refreshed_from_node: bool,
}

#[derive(Serialize)]
struct TipOut {
    height: u64,
    hash: String,
}

#[derive(Serialize)]
struct NodeTestOut {
    node_version: String,
    block_header_version: u16,
    height: u64,
    hash: String,
}

fn node_error(e: impl std::fmt::Display) -> GrinError {
    GrinError::new(GrinErrorCode::Node, e.to_string())
}

/// Returns `{"height": u64, "hash": String}` from the wallet's node.
#[no_mangle]
pub extern "C" fn grin_wallet_get_chain_height(handle: u64) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        let client = session.node_client()?;
        let (height, hash) = client.get_chain_tip().map_err(node_error)?;
        to_json(&TipOut { height, hash })
    }
}

/// Returns every `WalletInfo` amount in nanogrin plus `refreshed_from_node`.
#[no_mangle]
pub extern "C" fn grin_wallet_get_balance(
    handle: u64,
    refresh_from_node: bool,
    min_confirms: u64,
) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        let mask = session.mask();
        let (refreshed, info) = owner::retrieve_summary_info(
            session.wallet.clone(),
            mask.as_ref(),
            &None,
            refresh_from_node,
            min_confirms,
        )?;
        to_json(&BalanceOut {
            last_confirmed_height: info.last_confirmed_height,
            minimum_confirmations: info.minimum_confirmations,
            total: info.total,
            amount_awaiting_finalization: info.amount_awaiting_finalization,
            amount_awaiting_confirmation: info.amount_awaiting_confirmation,
            amount_immature: info.amount_immature,
            amount_currently_spendable: info.amount_currently_spendable,
            amount_locked: info.amount_locked,
            amount_reverted: info.amount_reverted,
            refreshed_from_node: refreshed,
        })
    }
}

/// Checks a node without a wallet: `get_version` then `get_tip`. Neither call
/// reads the chain type, so this needs no network parameter.
#[no_mangle]
pub extern "C" fn grin_node_test_connection(
    node_url: *const c_char,
    api_secret: *const c_char,
    socks_proxy: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let url = read_str(node_url, "node_url")?;
        let secret = read_opt_nonempty(api_secret, "api_secret")?;
        let proxy = read_opt_nonempty(socks_proxy, "socks_proxy")?;
        let settings = NodeSettings::new(
            url,
            secret,
            proxy,
            Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS),
        )?;
        let mut client = settings.http_client()?;
        let version = client.get_version_info().ok_or_else(|| {
            node_error("the node did not answer get_version (offline, wrong URL or wrong API secret)")
        })?;
        let (height, hash) = client.get_chain_tip().map_err(node_error)?;
        to_json(&NodeTestOut {
            node_version: version.node_version,
            block_header_version: version.block_header_version,
            height,
            hash,
        })
    }
}
