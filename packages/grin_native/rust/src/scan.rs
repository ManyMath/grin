//! UTXO scan with lock-free progress reporting and cancellation between
//! batches.
//!
//! `grin_wallet_scan` runs the same steps as libwallet's `owner::scan`, but
//! drives the batch loop itself so it can stop between batches:
//!
//! 1. refresh the wallet's outputs from the node (`refresh_outputs` with
//!    `update_all`, which also stores the node height as the wallet's last
//!    confirmed height); like `owner::scan`, a refresh failure other than a
//!    bad keychain mask is only logged,
//! 2. read the chain tip,
//! 3. scan from the start height to the tip in batches of 10,001 heights
//!    with libwallet's public `scan`, saving each batch's last scanned block
//!    (with the tip hash on the final batch) in its own database batch.
//!
//! Before each batch the scan checks for a cancel (from
//! `grin_wallet_scan_cancel`) or a close. If one arrived it returns
//! `Cancelled` with `{"scanned_height", "tip_height"}` in the details,
//! `scanned_height` being the last height of the last saved batch. A batch
//! that has started always runs to the end, so a cancel can wait for up to
//! one batch (10,001 blocks, including its node requests).
//!
//! A cancel only reaches a scan that is running: with no scan running
//! `grin_wallet_scan_cancel` does nothing, and every scan starts with the
//! cancel request cleared. Closing the handle stops a running scan and keeps
//! an in-flight call on the closed handle from starting one.

use std::sync::atomic::Ordering;
use std::sync::mpsc;

use grin_util::secp::key::SecretKey;
use grin_wallet_libwallet::{refresh_outputs, Error as LibwalletError, NodeClient, StatusMessage};
use serde::Serialize;

use crate::error::{to_json, GrinError, GrinErrorCode, GrinFfiResult};
use crate::handle::{get_session, ScanProgressState, ScanStart, WalletSession};

/// Heights per batch, as in `owner::scan` (from `h` to `h + 10,000`).
const BATCH_HEIGHTS: u64 = 10_001;

#[derive(Serialize)]
struct ScanOut {
    scanned_height: u64,
    tip_height: u64,
}

#[derive(Serialize)]
struct HeightOut {
    height: u64,
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

/// Clears the running and cancel bits however the scan ends.
struct ScanningFlag<'a>(&'a ScanProgressState);

impl Drop for ScanningFlag<'_> {
    fn drop(&mut self) {
        self.0.end_scan();
    }
}

fn batch_heights(session: &WalletSession) -> u64 {
    #[cfg(feature = "test-utils")]
    {
        let n = session.scan_hooks.batch_heights.load(Ordering::Acquire);
        if n > 0 {
            return n;
        }
    }
    let _ = session;
    BATCH_HEIGHTS
}

fn after_batch(session: &WalletSession, height: u64) {
    #[cfg(feature = "test-utils")]
    session.scan_hooks.run_after_batch(height);
    let _ = (session, height);
}

/// Step 1 of `owner::scan` (its private `update_outputs`): refreshes every
/// output from the node with the wallet mutex held.
fn refresh_before_scan(session: &WalletSession, mask: Option<&SecretKey>) -> Result<(), GrinError> {
    let mut w = session.wallet.lock();
    let backend = w.lc_provider()?.wallet_inst()?;
    let parent_key_id = backend.parent_key_id();
    match refresh_outputs(backend, mask, &parent_key_id, true) {
        Ok(()) => Ok(()),
        Err(LibwalletError::InvalidKeychainMask) => Err(LibwalletError::InvalidKeychainMask.into()),
        Err(e) => {
            log::warn!("scan: could not refresh outputs before scanning: {e}");
            Ok(())
        }
    }
}

/// How the batch loop ended when it did not fail.
enum BatchesEnd {
    Done,
    Stopped,
}

/// Scans `start..=tip` batch by batch, saving the last scanned block after
/// each batch and stopping before the next one once a cancel or close
/// arrives. `scanned` is kept at the last height of the last saved batch,
/// also when this returns an error.
#[allow(clippy::too_many_arguments)]
fn scan_batches(
    session: &WalletSession,
    mask: Option<&SecretKey>,
    start: u64,
    tip: u64,
    tip_hash: &str,
    delete_unconfirmed: bool,
    sender: &Option<mpsc::Sender<StatusMessage>>,
    scanned: &mut u64,
) -> Result<BatchesEnd, GrinError> {
    let state = &session.scan_state;
    let step = batch_heights(session);
    let mut total_pmmr_range = None;
    let mut h = start;
    while h <= tip {
        if state.should_stop() {
            return Ok(BatchesEnd::Stopped);
        }
        let batch_end = tip.min(h.saturating_add(step - 1));
        let (mut info, range) = grin_wallet_libwallet::scan(
            session.wallet.clone(),
            mask,
            delete_unconfirmed,
            h,
            batch_end,
            start,
            tip,
            total_pmmr_range,
            sender,
        )?;
        info.hash = if batch_end == tip {
            tip_hash.to_owned()
        } else {
            String::new()
        };
        total_pmmr_range = Some(range);
        {
            let mut w = session.wallet.lock();
            let backend = w.lc_provider()?.wallet_inst()?;
            let mut batch = backend.batch(mask)?;
            batch.save_last_scanned_block(info)?;
            batch.commit()?;
        }
        *scanned = batch_end;
        state.scanned_height.fetch_max(batch_end, Ordering::AcqRel);
        after_batch(session, batch_end);
        match batch_end.checked_add(1) {
            Some(next) => h = next,
            None => break,
        }
    }
    Ok(BatchesEnd::Done)
}

/// Adds `scanned_height` and `tip_height` to the details of an error that
/// ended a scan after the tip was read.
fn with_scan_heights(mut e: GrinError, scanned: u64, tip: u64) -> GrinError {
    let mut details = match e.details.take() {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    details.insert("scanned_height".to_owned(), serde_json::json!(scanned));
    details.insert("tip_height".to_owned(), serde_json::json!(tip));
    e.details = Some(serde_json::Value::Object(details));
    e
}

/// Scans the chain from `start_height` (0 means from the first block) and
/// repairs wallet outputs. Runs on the calling thread in batches of 10,001
/// heights and returns `{"scanned_height", "tip_height"}`.
/// `grin_wallet_scan_cancel` or closing the handle stops it between batches
/// with Cancelled and `{"scanned_height", "tip_height"}` in the details.
#[no_mangle]
pub extern "C" fn grin_wallet_scan(
    handle: u64,
    start_height: u64,
    delete_unconfirmed: bool,
) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        session.require_network()?;
        let state = &session.scan_state;

        match state.begin_scan() {
            ScanStart::Started => {}
            ScanStart::Busy => {
                return Err(GrinError::new(
                    GrinErrorCode::WalletBusy,
                    "a scan is already running on this wallet",
                )
                .with_details(serde_json::json!({
                    "wallet_dir": session.data_dir.to_string_lossy(),
                    "reason": "scan_in_progress",
                })));
            }
            ScanStart::Closed => {
                return Err(GrinError::new(
                    GrinErrorCode::Cancelled,
                    "the wallet was closed before the scan started",
                ));
            }
        }
        let _flag = ScanningFlag(state);

        // libwallet starts at height 1 when no start height is given.
        let effective_start = start_height.max(1);
        state.reset(effective_start);

        let mask = session.mask();
        refresh_before_scan(&session, mask.as_ref())?;
        let (tip, tip_hash) = session
            .node_client()?
            .get_chain_tip()
            .map_err(|e| GrinError::new(GrinErrorCode::Node, e.to_string()))?;
        // Nothing is scanned yet: one below the start, or the tip when the
        // start lies beyond it and there is nothing to scan.
        let mut scanned = effective_start.saturating_sub(1).min(tip);
        state.tip_height.store(tip, Ordering::Release);
        state.scanned_height.store(scanned, Ordering::Release);

        let (tx, rx) = mpsc::channel();
        let result = std::thread::scope(|scope| {
            scope.spawn(|| drain_status(rx, state));
            let sender = Some(tx);
            let r = scan_batches(
                &session,
                mask.as_ref(),
                effective_start,
                tip,
                &tip_hash,
                delete_unconfirmed,
                &sender,
                &mut scanned,
            );
            // Dropping the sender ends the drain thread before the scope joins.
            drop(sender);
            r
        });
        // The drain thread has stopped, so this replaces its estimate with
        // the height that is actually saved.
        state.scanned_height.store(scanned, Ordering::Release);
        // A scan that fails part way has still saved every batch before
        // the failure, so the error says how far it got.
        let result = result.map_err(|e| with_scan_heights(e, scanned, tip));
        match result? {
            BatchesEnd::Done => to_json(&ScanOut {
                scanned_height: scanned,
                tip_height: tip,
            }),
            BatchesEnd::Stopped => Err(GrinError::new(GrinErrorCode::Cancelled, "scan cancelled")
                .with_details(serde_json::json!({
                    "scanned_height": scanned,
                    "tip_height": tip,
                }))),
        }
    }
}

/// Returns `{"height": u64}`, the height of the last block the wallet saved
/// as scanned (by `grin_wallet_scan` or a refresh from the node), or 0 when
/// it has never scanned. Reads the wallet database and contacts no node.
#[no_mangle]
pub extern "C" fn grin_wallet_get_last_scanned_height(handle: u64) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        let height = {
            let mut w = session.wallet.lock();
            w.lc_provider()?.wallet_inst()?.last_scanned_block()?.height
        };
        to_json(&HeightOut { height })
    }
}

/// Reads the scan progress atomics. Takes no wallet lock.
#[no_mangle]
pub extern "C" fn grin_wallet_scan_progress(handle: u64) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        let s = &session.scan_state;
        to_json(&ProgressOut {
            is_scanning: s.is_scanning(),
            cancel_requested: s.cancel_requested(),
            start_height: s.start_height.load(Ordering::Acquire),
            scanned_height: s.scanned_height.load(Ordering::Acquire),
            tip_height: s.tip_height.load(Ordering::Acquire),
            progress_pct: s.progress_pct(),
        })
    }
}

/// Asks a running scan to stop before its next batch and returns at once;
/// the `grin_wallet_scan` call then returns Cancelled with the last saved
/// height. Takes no wallet lock. With no scan running it does nothing and
/// the next scan runs normally. Closing the handle has the same effect on a
/// running scan.
#[no_mangle]
pub extern "C" fn grin_wallet_scan_cancel(handle: u64) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        if !session.scan_state.request_cancel() {
            log::debug!("wallet {handle}: scan cancel with no scan running");
        }
        Ok(None)
    }
}
