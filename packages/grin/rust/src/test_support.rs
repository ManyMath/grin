//! In-process chain and wallets for tests (docs/architecture.md 7.1). Only
//! built with the `test-utils` feature; release artifacts must not export
//! any `grin_test_*` symbol.
//!
//! Each `chain_dir` gets one `WalletProxy` over a real `grin_chain::Chain`
//! at `ChainTypes::AutomatedTesting` (coinbase maturity 3), running on its
//! own thread. Test wallets use `LocalWalletClient` through the same
//! `FfiNodeClient` enum, session registry and entry points as real wallets.
//!
//! Network choice: `grin_test_create_wallet` accepts every network so tests
//! can check keychain derivation and address prefixes, but the chain itself
//! is always AutomatedTesting. grin_chain validates blocks against the
//! calling thread's chain type (proof of work size, maturity, genesis), so
//! `grin_test_mine_blocks` builds blocks under AutomatedTesting whatever the
//! wallet's network is, and a Mainnet or Testnet wallet applies its own
//! coinbase maturity (1440 blocks) when computing balances. Funded flows
//! therefore use AutomatedTesting wallets, and the Mainnet + Testnet
//! concurrency test uses `grin_wallet_create` against an unreachable node.
//!
//! Harness limits: `LocalWalletClient` matches replies to requests only by
//! order, so do not run two node-touching calls on the same test wallet at
//! once; `WalletProxy` panics on an unknown client id, so every client is
//! registered before its wallet makes a node call; and `mine_lock` only
//! serializes `grin_test_mine_blocks`, while a `post_tx` handled on the proxy
//! thread also mines a block, so do not mine and broadcast on one chain at
//! the same moment.

use std::collections::HashMap;
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use grin_chain::Chain;
use grin_core::global::{self, ChainTypes};
use grin_keychain::ExtKeychain;
use grin_util::secp::key::SecretKey;
use grin_wallet_impls::test_framework::{self, LocalWalletClient, WalletProxy};
use serde::Serialize;

use crate::config::{
    chain_type_from_u8, FfiNodeClient, NodeSettings, DEFAULT_REQUEST_TIMEOUT_SECS,
};
use crate::error::{
    panic_message, read_opt_secret, read_secret, read_str, to_json, GrinError, GrinFfiResult,
};
use crate::handle::{
    canonical_data_dir_create, get_session, lookup_session, register_session, DirLease,
    WalletInstance, WalletLc, WalletSession,
};
use crate::lifecycle::{
    check_data_dir_name, open_backend, validate_mnemonic_words, OpenMode, SEED_FILE,
};

type Proxy = WalletProxy<'static, WalletLc, FfiNodeClient, ExtKeychain>;

enum ProxyCommand {
    AddWallet {
        client: LocalWalletClient,
        wallet: WalletInstance,
        mask: Option<SecretKey>,
        ack: mpsc::Sender<()>,
    },
}

/// One in-process chain and the proxy thread serving it.
pub struct TestChain {
    pub chain_dir: PathBuf,
    pub chain: Arc<Chain>,
    make_client: Box<dyn Fn(&str) -> LocalWalletClient + Send + Sync>,
    commands: mpsc::Sender<ProxyCommand>,
    running: Arc<AtomicBool>,
    /// Serializes block production so two callers never build on the same tip.
    mine_lock: Mutex<()>,
}

impl TestChain {
    fn client(&self, client_id: &str) -> LocalWalletClient {
        (self.make_client)(client_id)
    }

    /// `WalletProxy::run` owns the proxy while it loops, so new wallets are
    /// handed to the proxy thread, which stops the loop, registers them and
    /// starts it again. The stop flag is re-sent until the thread confirms,
    /// because `run` sets it back to true on entry.
    fn add_wallet(
        &self,
        client: LocalWalletClient,
        wallet: WalletInstance,
        mask: Option<SecretKey>,
    ) -> Result<(), GrinError> {
        let (ack, ack_rx) = mpsc::channel();
        self.commands
            .send(ProxyCommand::AddWallet {
                client,
                wallet,
                mask,
                ack,
            })
            .map_err(|_| GrinError::internal("test proxy thread has stopped"))?;
        for _ in 0..500 {
            self.running.store(false, Ordering::SeqCst);
            match ack_rx.recv_timeout(Duration::from_millis(20)) {
                Ok(()) => return Ok(()),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        Err(GrinError::internal(
            "test proxy thread did not register the wallet",
        ))
    }
}

static TEST_CHAINS: LazyLock<Mutex<HashMap<PathBuf, Arc<TestChain>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn proxy_thread(mut proxy: Proxy, commands: mpsc::Receiver<ProxyCommand>) {
    global::set_local_chain_type(ChainTypes::AutomatedTesting);
    loop {
        loop {
            match commands.try_recv() {
                Ok(ProxyCommand::AddWallet {
                    client,
                    wallet,
                    mask,
                    ack,
                }) => {
                    let id = client.id.clone();
                    proxy.add_wallet(&id, client.get_send_instance(), wallet, mask);
                    let _ = ack.send(());
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        match catch_unwind(AssertUnwindSafe(|| proxy.run())) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => log::warn!("test proxy: {e}"),
            Err(p) => log::error!("test proxy panicked: {}", panic_message(p.as_ref())),
        }
    }
}

/// Returns the chain for `chain_dir`, creating it and its proxy thread on
/// first use. Leaves this thread's chain type at AutomatedTesting.
fn chain_for(chain_dir: &str) -> Result<Arc<TestChain>, GrinError> {
    std::fs::create_dir_all(chain_dir)?;
    let key = std::fs::canonicalize(chain_dir)?;
    let mut chains = TEST_CHAINS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(c) = chains.get(&key) {
        return Ok(c.clone());
    }
    let dir = key
        .to_str()
        .ok_or_else(|| GrinError::invalid_argument("chain_dir is not a valid UTF-8 path"))?;
    // Sets this thread's chain type to AutomatedTesting and mines genesis.
    let proxy: Proxy = WalletProxy::new(dir);
    let chain = proxy.chain.clone();
    let proxy_tx = proxy.tx.clone();
    let running = proxy.running.clone();
    let (commands, commands_rx) = mpsc::channel();
    thread::Builder::new()
        .name("grin-test-proxy".to_owned())
        .spawn(move || proxy_thread(proxy, commands_rx))?;
    let test_chain = Arc::new(TestChain {
        chain_dir: key.clone(),
        chain,
        make_client: Box::new(move |id| LocalWalletClient::new(id, proxy_tx.clone())),
        commands,
        running,
        mine_lock: Mutex::new(()),
    });
    chains.insert(key, test_chain.clone());
    Ok(test_chain)
}

#[derive(Serialize)]
struct HandleOut {
    handle: u64,
}

#[derive(Serialize)]
struct HeightOut {
    height: u64,
}

/// Creates (or opens, if `wallet.seed` exists) a wallet in
/// `<wallet_dir>/wallet_data` connected to the in-process chain in
/// `chain_dir` as `client_id`. Returns `{"handle": u64}`.
#[no_mangle]
pub extern "C" fn grin_test_create_wallet(
    chain_dir: *const c_char,
    wallet_dir: *const c_char,
    client_id: *const c_char,
    network: u8,
    mnemonic: *const c_char,
    password: *const c_char,
) -> GrinFfiResult {
    ffi_guard! {
        let chain_type = chain_type_from_u8(network)?;
        let chain_dir = read_str(chain_dir, "chain_dir")?;
        let wallet_dir = read_str(wallet_dir, "wallet_dir")?;
        let client_id = read_str(client_id, "client_id")?;
        if client_id.is_empty() || client_id == "node" {
            return Err(GrinError::invalid_argument(
                "client_id must be non-empty and not \"node\"",
            ));
        }
        let mnemonic = read_opt_secret(mnemonic, "mnemonic")?;
        let password = read_secret(password, "password")?;
        if let Some(m) = &mnemonic {
            validate_mnemonic_words(m)?;
        }

        let test_chain = chain_for(chain_dir)?;
        // chain_for may have switched this thread to AutomatedTesting.
        global::set_local_chain_type(chain_type);

        let (data_dir, existed) = canonical_data_dir_create(wallet_dir)?;
        check_data_dir_name(&data_dir)?;
        let lease = DirLease::acquire(&data_dir)?;
        let mode = if data_dir.join(SEED_FILE).exists() {
            OpenMode::Open
        } else {
            OpenMode::Create(mnemonic)
        };
        let client = test_chain.client(client_id);
        let opened = open_backend(FfiNodeClient::Local(client.clone()), &data_dir, password, mode)
            .and_then(|(wallet, mask)| {
                test_chain.add_wallet(client, wallet.clone(), mask.clone())?;
                Ok((wallet, mask))
            });
        let (wallet, mask) = match opened {
            Ok(v) => v,
            Err(e) => {
                if !existed {
                    let _ = std::fs::remove_dir_all(&data_dir);
                }
                return Err(e);
            }
        };

        let node = NodeSettings {
            url: format!("local:{client_id}"),
            api_secret: None,
            socks_proxy: None,
            timeout: Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS),
        };
        let handle = register_session(|h| {
            let mut s = WalletSession::new(h, chain_type, wallet, mask, node, lease);
            s.test_chain = Some(test_chain);
            s
        });
        to_json(&HandleOut { handle })
    }
}

/// Mines `count` blocks whose coinbase goes to `handle`'s wallet. Returns
/// `{"height": u64}`, the chain height afterwards.
#[no_mangle]
pub extern "C" fn grin_test_mine_blocks(handle: u64, count: u64) -> GrinFfiResult {
    ffi_guard! {
        let session = get_session(handle)?;
        let test_chain = session
            .test_chain
            .clone()
            .ok_or_else(|| GrinError::invalid_argument("handle is not a test wallet"))?;
        let count = usize::try_from(count)
            .map_err(|_| GrinError::invalid_argument("count is too large"))?;
        let mask = session.mask();
        let height = {
            let _mining = test_chain
                .mine_lock
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            // Blocks are built and validated with the chain's own parameters.
            global::set_local_chain_type(ChainTypes::AutomatedTesting);
            let res = test_framework::award_blocks_to_wallet(
                &test_chain.chain,
                session.wallet.clone(),
                mask.as_ref(),
                count,
                false,
            );
            global::set_local_chain_type(session.chain_type);
            res?;
            test_chain
                .chain
                .head()
                .map_err(|e| GrinError::internal(format!("chain head: {e}")))?
                .height
        };
        to_json(&HeightOut { height })
    }
}

/// Panics inside the guard so callers can check InternalPanic handling.
#[no_mangle]
pub extern "C" fn grin_test_panic() -> GrinFfiResult {
    ffi_guard! {
        panic!("grin_test_panic")
    }
}

/// Clones a registered session's `Arc`, standing in for an in-flight call
/// that outlives `grin_wallet_close`. Plain Rust, not exported.
pub fn retain_session(handle: u64) -> Option<Arc<WalletSession>> {
    lookup_session(handle)
}

/// Called by `grin_wallet_scan` after each batch is saved, with the last
/// height of that batch.
pub type AfterBatch = Box<dyn Fn(u64) + Send + Sync>;

/// Per-session scan knobs for tests. The test chain is far shorter than one
/// production batch, so tests shrink the batch and use the callback to act
/// at an exact batch boundary.
#[derive(Default)]
pub struct ScanTestHooks {
    /// Heights per scan batch; 0 keeps the production size.
    pub(crate) batch_heights: AtomicU64,
    pub(crate) after_batch: Mutex<Option<AfterBatch>>,
}

impl ScanTestHooks {
    pub(crate) fn run_after_batch(&self, height: u64) {
        let hook = self
            .after_batch
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(f) = hook.as_ref() {
            f(height);
        }
    }
}

/// Sets the scan batch size (0 for the production size) and the per-batch
/// callback of a registered session. Returns false for an unknown handle.
/// Plain Rust, not exported.
pub fn set_scan_test_hooks(
    handle: u64,
    batch_heights: u64,
    after_batch: Option<AfterBatch>,
) -> bool {
    let Some(session) = lookup_session(handle) else {
        return false;
    };
    session
        .scan_hooks
        .batch_heights
        .store(batch_heights, Ordering::Release);
    *session
        .scan_hooks
        .after_batch
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = after_batch;
    true
}
