//! UTXO scan with lock-free progress reporting.
//!
//! Cancellation limitation: `owner::scan` already works in batches of 10,000
//! blocks, but it has no hook between batches and the helper it calls first
//! (`update_outputs`) is private, so this crate cannot stop a scan that has
//! started. `grin_wallet_scan_cancel` sets a flag that makes the next
//! `grin_wallet_scan` return Cancelled without starting; a scan already
//! running finishes normally and then clears the flag. A later change can
//! drive `grin_wallet_libwallet::scan` over height ranges itself and check
//! the flag between batches.

use std::sync::atomic::Ordering;
use std::sync::mpsc;

use grin_wallet_libwallet::api_impl::owner;
use grin_wallet_libwallet::{NodeClient, StatusMessage};
use serde::Serialize;

use crate::error::{to_json, GrinError, GrinErrorCode, GrinFfiResult};
use crate::handle::{get_session, ScanProgressState};

#[derive(Serialize)]
struct ScanOut {
    scanned_height: u64,
    tip_height: u64,
}

#[derive(Serialize)]
struct ProgressOut {
    is_scanning: bool,
    cancel_requested: bool,
    start_height: u64,
    scanned_height: u64,
    tip_height: u64,
    progress_pct: u8,
}

/// Turns libwallet status messages into progress atomics until the sender
/// side is dropped.
fn drain_status(rx: mpsc::Receiver<StatusMessage>, state: &ScanProgressState) {
    let start = state.start_height.load(Ordering::Acquire);
    let tip = state.tip_height.load(Ordering::Acquire);
    while let Ok(msg) = rx.recv() {
        match msg {
            StatusMessage::Scanning(_, pct) => {
                let pct = u64::from(pct.min(100));
                let span = tip.saturating_sub(start);
                let height = start + (u128::from(span) * u128::from(pct) / 100) as u64;
                state.scanned_height.fetch_max(height, Ordering::AcqRel);
            }
            StatusMessage::ScanningComplete(_) => {
                log::debug!("scan batch complete");
            }
            StatusMessage::UpdateWarning(w) => log::warn!("scan: {w}"),
            StatusMessage::UpdatingOutputs(_)
            | StatusMessage::UpdatingTransactions(_)
            | StatusMessage::FullScanWarn(_) => {}
        }
    }
}

/// Clears `is_scanning` however the scan ends.
struct ScanningFlag<'a>(&'a ScanProgressState);

impl Drop for ScanningFlag<'_> {
    fn drop(&mut self) {
        self.0.is_scanning.store(false, Ordering::Release);
    }
}

/// Scans the chain from `start_height` (0 means from the first block) and
/// repairs wallet outputs. Runs on the calling thread and returns
/// `{"scanned_height", "tip_height"}`.
#[no_mangle]
pub extern "C" fn grin_wallet_scan(
    handle: u64,
    start_height: u64,
    delete_unconfirmed: bool,
) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        let state = &session.scan_state;

        if state
            .is_scanning
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(GrinError::new(
                GrinErrorCode::WalletBusy,
                "a scan is already running on this wallet",
            )
            .with_details(serde_json::json!({
                "wallet_dir": session.data_dir.to_string_lossy(),
                "reason": "scan_in_progress",
            })));
        }
        let _flag = ScanningFlag(state);

        if state.cancel_requested.swap(false, Ordering::AcqRel) {
            return Err(GrinError::new(GrinErrorCode::Cancelled, "scan cancelled"));
        }

        // libwallet starts at height 1 when no start height is given.
        let effective_start = start_height.max(1);
        let (tip, _) = session
            .node_client()?
            .get_chain_tip()
            .map_err(|e| GrinError::new(GrinErrorCode::Node, e.to_string()))?;
        state.start_height.store(effective_start, Ordering::Release);
        state.tip_height.store(tip, Ordering::Release);
        state.scanned_height.store(effective_start.min(tip), Ordering::Release);

        let mask = session.mask();
        let (tx, rx) = mpsc::channel();
        let result = std::thread::scope(|scope| {
            scope.spawn(|| drain_status(rx, state));
            let sender = Some(tx);
            let r = owner::scan(
                session.wallet.clone(),
                mask.as_ref(),
                if start_height == 0 { None } else { Some(start_height) },
                delete_unconfirmed,
                &sender,
            );
            // Dropping the sender ends the drain thread before the scope joins.
            drop(sender);
            r
        });
        result?;

        state.scanned_height.fetch_max(tip, Ordering::AcqRel);
        // A cancel that arrived while the scan ran could not stop it.
        state.cancel_requested.store(false, Ordering::Release);
        to_json(&ScanOut {
            scanned_height: state.scanned_height.load(Ordering::Acquire),
            tip_height: tip,
        })
    }
}

/// Reads the scan progress atomics. Takes no wallet lock.
#[no_mangle]
pub extern "C" fn grin_wallet_scan_progress(handle: u64) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        let s = &session.scan_state;
        to_json(&ProgressOut {
            is_scanning: s.is_scanning.load(Ordering::Acquire),
            cancel_requested: s.cancel_requested.load(Ordering::Acquire),
            start_height: s.start_height.load(Ordering::Acquire),
            scanned_height: s.scanned_height.load(Ordering::Acquire),
            tip_height: s.tip_height.load(Ordering::Acquire),
            progress_pct: s.progress_pct(),
        })
    }
}

/// Requests cancellation. Takes no wallet lock. A scan that is already
/// running cannot be interrupted and finishes normally; if no scan is
/// running, the next `grin_wallet_scan` returns Cancelled without starting.
#[no_mangle]
pub extern "C" fn grin_wallet_scan_cancel(handle: u64) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        session
            .scan_state
            .cancel_requested
            .store(true, Ordering::Release);
        Ok(None)
    }
}
