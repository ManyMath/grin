# Grin in Stack Wallet: implementation plan

Date: 2026-11-07.

Baselines:
- Stack Wallet `github-cs/staging` 4bc21f9e1 (2026-11-05). Local `staging` (7ce7b34f2) and `cypherstack/staging` (41e8fab9c) are both older.
- grin repo `main` = `hardening` 30d8fb0.
- grin-wallet fork `hardening` 377141e.

Unless noted, Stack Wallet paths are relative to the repo root, with line numbers at 4bc21f9e1. Grin paths are relative to `/home/user/src/stack_wallet/grin`.

This file is the plan as written on 2026-11-07, kept in the repository; only the status column of section 3 was added later. It supersedes section 9 of `docs/architecture.md`. Section 9 was written against staging 7ce7b34f2 and the unmerged `refactor/epic-mwc-unify` branch. Section 12 lists what in it is stale.

## 0. Resolved decisions (user, 2026-11-07)

- Hosting stays private on git.cypherstack.com until responsible disclosure is complete. No public mirrors, nothing on GitHub. Testing is limited to people with Gitea access, and the senior reviews on Gitea. This replaces G1: dependency URLs are `ssh://git@git.cypherstack.com:222/sneurlax/grin_native.git` (pub, which runs with the full environment and the ssh agent) and the fork stays `ssh://git@git.cypherstack.com:222/sneurlax/grin-wallet.git` (cargo inside the hook, which has no ssh agent). A consumer therefore runs a one-time `cargo fetch --locked` for the grin_native crate, with `CARGO_HOME` unset or set to `$HOME/.cargo`, before the first build, or keeps a passphrase-less deploy key in `~/.ssh`. Stack Wallet gets a helper script for this. A1 stands, because these branches are only built by people with Gitea access; Campfire and Duo builds on these branches need it too.
- Every open decision in section 11 takes my recommendation, with these specifics:
  - Default mainnet node `https://grinnode.live`, port 3413. It answered `get_tip` on 2026-11-07, height 4096748. No public testnet node answered (testnet.grinnode.live, main.grinnode.live, api.grin.money and grincoin.org were all tried), so `Grin(test)` is defined with default `http://127.0.0.1`, port 13413, and left out of the default coin list.
  - CI work in S6 is limited to scripts that run locally (symbol gate, prefetch). GitHub workflows are untouched.
  - Stripping symbols and LTO wait for device runs.

## 1. Summary

Grin ships as a new coin, `Grin` (identifier `grin`; testnet `grinTestNet`), backed by the grin repo's two packages:
- `package:grin` is pure Dart with no runtime dependencies. It becomes an unconditional Stack Wallet dependency.
- `package:grin_native` holds the FFI, the Rust crate and the build hook. It sits behind a `%%ENABLE_GRIN%%` pubspec block and a wl_gen template, so Campfire and Stack Duo never compile Rust.

`GrinWallet extends Bip39Wallet<Grin>` and talks to `GrinWalletApi` directly.

Five blockers come before any Stack Wallet code:
1. The grin repo and the grin-wallet fork can only be reached over SSH from git.cypherstack.com. Flutter's hook runner strips `SSH_AUTH_SOCK`, so even a developer with access cannot build through the hook without a key file. CI and outside developers cannot fetch either repo at all.
2. On sandboxed macOS (and possibly iOS), LMDB's default SysV semaphores fail. MWC hit this and works around it with CFLAGS in its hook. The Grin hook has nothing equivalent, and Grin has never run on macOS.
3. Rust is pinned to 1.94.0. Rust before 1.98 can emit Apple binaries that iOS 27 and macOS 27 `dyld` rejects. Stack Wallet, Epic and MWC are all on 1.99.1.
4. A wallet created with a mnemonic is marked `InitNeedsScanning`. Stack always creates wallets with a mnemonic, so the first `refreshFromNode: true` call runs a full scan from genesis that `cancelScan` cannot stop. I checked this against the fork: `impls/src/lifecycle/default.rs` saves `InitNeedsScanning` for any mnemonic, and `update_wallet_state` starts from `last_scanned_block`, which is 0 (`libwallet/src/api_impl/owner.rs:1182-1213`).
5. Flutter 3.47.2's hook runner drops `RUSTUP_HOME` and `CARGO_HOME`, so the toolchains preinstalled in the CI images are invisible to hooks. Builds still work, but rustup downloads the pinned toolchain into `$HOME/.rustup` on every run.

Order of work:
1. Phase G: grin repo prerequisites.
2. Phase S1: rebase `refactor/epic-mwc-unify` onto staging as its own PR.
3. Phase S2: Grin coin and wallet core, with no new screens.
4. Phase S3: generalized Slatepack UI.
5. Phase S4: Grin send, receive and transaction-details UI.
6. Phase S5: invoices and payment proofs.
7. Phase S6: CI, release and theme assets.

S2 does not depend on S1. S3 and later do.

## 2. Architecture decisions

### A1. Dependency shape

- `package:grin` is an unconditional dependency (git, `path: packages/grin`). It has no runtime dependencies. With it in every build, `grin.dart`, `GrinWallet` and the tests can use `GrinWalletApi`, `GrinWalletBackend`, the models and the sealed `GrinException` directly.
- `package:grin_native` sits behind `%%ENABLE_GRIN%%`. The wl_gen boundary shrinks to a single getter:
  ```dart
  // lib/wl_gen/interfaces/libgrin_interface.dart
  export '../generated/libgrin_interface_impl.dart';
  abstract final class LibGrin { GrinWalletBackend get backend; }
  // tool/wl_templates/GRIN_libgrin_interface_impl.template.dart
  //ON  import 'package:grin_native/grin_native.dart';  backend => const GrinNativeBackend();
  //OFF backend => throw StateError("GRIN not enabled");
  ```
- This replaces section 9.2 item 5 and the matching agent proposal: a 25-method `GrinSession` facade plus about 15 Stack-side copies of the Grin types. Those exist only so OFF builds compile, and pure-Dart `package:grin` already solves that.
- Precondition: `package:grin` must be reachable over HTTPS from every app config (G1). If the public mirror slips, the fallback is the copied-types design. It costs about a day more and adds a translation layer to maintain.
- Still to prove (G7): pub must accept a git dependency on a package whose pubspec says `resolution: workspace`. `grin_native` declares `grin: ^0.1.0` (hosted), so Stack Wallet needs a `dependency_overrides` entry for `grin` at the same git ref until `grin` is published. Nobody has tried an outside consumer yet.

### A2. Class shape

- `class GrinWallet extends Bip39Wallet<Grin>` with an injectable backend: `GrinWallet(network, {GrinWalletBackend? backend})`. This follows `LibXelisWallet(super.currency, {LibXelisInterface? native})` at `lib/wallets/wallet/intermediate/lib_xelis_wallet.dart:20-24`.
- `Bip39Wallet` gives `MnemonicInterface` and the standard address filters. Epic and MWC extend it the same way.
- No `MimblewimbleWallet` parent. It exists only on the unify branch, and six of its members would be stubs or wrong for Grin: the relay pair, the listener pair, chunked `scanOutputs` and `double` balances.
- No mixin in S2. In S3, after the unify rebase, I extract `SlatepackInterface<T>` with members named so `GrinWallet` already matches. Section 6 lists them.

### A3. Transactions

- txid is the `slateId`, or `grin_cb_<logId>` for coinbase. hash is `kernelExcess ?? txid`.
- A self-send produces a sent and a received log entry with the same slate id. They merge into one `sentToSelf` row; two rows would violate the unique `(txid, walletId)` index (`transaction_v2.dart:22-23`).
- Inputs and outputs are synthetic so the generic getters and the generic details branch work. Section 5.3 has the rules.
- `otherData.overrideFee` is always set. Without it `getFee` calls `reduce` on an empty list and throws (`transaction_v2.dart:146-151`).

### A4. Scanning

- The scan resume point is persisted in `WalletInfo` other data: `WalletInfoKeys.grinData = {"pendingScanFrom": int?, "creationHeight": int}`.
- One scan job runs inside `refreshMutex.protect`, which makes the base `refresh()` and the ping check stand down (`wallet.dart:452-455, 559`).
- Rule: no `refreshFromNode: true`, `estimateFee`, `cancelTransaction` or send runs while `pendingScanFrom != null`. All of them call libwallet's `update_wallet_state`, which scans from `last_scanned_block - 2880` and cannot be cancelled.
- G4 adds a library flag so a brand-new wallet is stored as `InitNoScanning` and never needs that scan. The rule stays as a second line of defence.

### A5. Transport

- A pure function `resolveGrinTransport(...)` returns `Direct | Proxy("127.0.0.1:<port>") | Blocked(reason)` (section 7).
- It runs when the transport is applied, not when a Tor event arrives. Events arrive out of order: `connectTor` fires `connected` before `useTor` becomes true, and `disconnectTor` fires `disconnected` while `useTor` is still true (`tor_settings_view.dart:557-590`).
- The library has no offline mode. In v1, Blocked means a Dart gate on every node-touching call, plus a dead proxy `127.0.0.1:1` handed to `updateNetwork` as a backstop. G6 adds a real offline mode.

### A6. Secrets

| What | Where |
|---|---|
| Mnemonic | The existing `MnemonicInterface` slot |
| Passphrase | `""`, because `Wallet.create` force-unwraps it |
| Seed-file password | A dedicated `${walletId}_grinWalletPassword` from `generatePassword()` (128-bit random) |

- The password is written to secure storage before `create`. It is never changeable from the UI, which avoids the review's change-password crash window. It is deleted in the Grin delete branch. Epic and MWC leak theirs.
- The password is not in Stack backups (SWB). On restore it is regenerated, which is safe because the seed file is rebuilt from the mnemonic.

### A7. Wallet directory and ownership

- The path is `p.normalize(p.absolute(p.join(StackFileSystem root, "grin", walletId)))`, recomputed on every open.
- On unix desktop both levels are created with mode 0700. `wallet_data` is never created by Dart and never a link.
- The absolute step matters: the desktop `-d` override root can be relative (`main.dart:100-102`), and the library accepts relative paths.
- A process-wide `GrinWallet._owners` map and a per-walletId lock, in static fields, cover two cases. Restore-cancel deletes a wallet that `Wallets` has not registered yet (`restore_wallet_view.dart:300-312`, registration at :385). The new-wallet instance is also unregistered until verify. Without the map, delete fails with `WalletBusy`.

### A8. Native delivery

- v1 is a source build through the hook, behind `ENABLE_GRIN`. CI already compiles frostdart and xelis from source, and release hosts compile every Rust plugin from source.
- Rust 1.99.1.
- No prebuilt manifest. The old manifest scheme is being retired upstream in favour of dnix. Grin moves to dnix together with the Epic/MWC dnix bump, as its own decision (open decision 2).

## 3. Phase G: grin repo prerequisites

I can do these myself on the grin `hardening` branch, pushed to cypherstack and manymath with `main` fast-forwarded, except where a decision is marked.

| # | Task | Acceptance | Status (2026-11-07) |
|---|---|---|---|
| G1 | Public HTTPS mirrors of the grin repo and the grin-wallet fork, with identical SHAs so rev 377141e is unchanged. Repoint the nine `git = "ssh://..."` entries in `packages/grin_native/rust/Cargo.toml:24-32` to HTTPS and regenerate `Cargo.lock`; only the `source =` lines change. Update `repository:` in both pubspecs. DECISION: mirror host and name; which Gitea URL is canonical (`grin.git` or `grin_native.git`). | `cargo fetch --locked` with no SSH key; `flutter pub get` in a throwaway app with no SSH key | Replaced by section 0: no public mirrors. Consumers use `ssh://git@git.cypherstack.com:222/sneurlax/grin_native.git` for both pub entries and prefetch the fork once (`packages/grin_native/README.md`, "Private access", added with G11). |
| G2 | Bump `packages/grin_native/rust/rust-toolchain.toml` to 1.99.1 and rerun the full gate. | All gates green; release symbols 35/0 | Done in fb03b09: 1.99.1, target list unchanged. The new clippy deprecates `AtomicU8::fetch_update`, replaced by `try_update`. Release build `36 standard, 0 test symbols` (35 plus G5's export) on the host and the three Android targets. |
| G3 | In `hook/build.dart`, set `CFLAGS_<arch>_apple_darwin=-DMDB_USE_POSIX_MUTEX=1 -DMDB_USE_ROBUST=0` as MWC does (`flutter_libmwc` `hook/build.dart:28-33`), and for iOS if it reproduces there. | Example app creates, opens, scans and sends on sandboxed macOS and on an iOS device | Done in 29ec73c for macOS and iOS (`aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-apple-ios`, `aarch64-apple-ios-sim`, `x86_64-apple-ios`). Not verified on a device yet; recorded as pending in `docs/architecture.md` section 13. |
| G4 | Add a "new wallet" option to `grin_wallet_create` (for example `is_new: bool`, or a `scan_from_tip` field in the config JSON). It saves `InitNoScanning` even when a mnemonic is given. Expose it as `GrinWalletBackend.create(..., isNewWallet: true)`. Rust and Dart tests. | A new wallet's first `getBalance(refreshFromNode: true)` scans only the reorg window | Done in 9db5a3d: `grin_wallet_create(config_json, mnemonic, password, is_new_wallet)` saves `InitNoScanning` through a wallet batch inside the same call; `GrinWalletBackend.create(..., bool isNewWallet = false)`. Rust tests check the stored status; the reorg-window behaviour itself needs a chain longer than 2880 blocks and is in the ignored release test `new_wallet_refresh_scans_only_the_reorg_window`, whose first run failed on a full disk; the rerun on 2026-11-07 passed (1 passed in 2406 s). |
| G5 | Add a `lastScannedHeight` getter (from `last_scanned_block`), so Stack doesn't have to reconstruct the resume point when a cancellation is lost. | Rust and Dart tests | Done in 9db5a3d: `grin_wallet_get_last_scanned_height(handle)` returns `{"height"}`; `Future<int> GrinWalletApi.lastScannedHeight()`, queued. |
| G6 | Add an offline mode to `updateNetwork`/config (`network_enabled: false`). Every node call then fails fast with a Node error, and no socket is opened. | Unit test proves no connect; replaces the dead proxy in S2 | Done in 9db5a3d: `network_enabled` in `config_json` and a `network_enabled` argument on `grin_wallet_update_network`; `GrinWalletConfig.networkEnabled` (default true) and `updateNetwork(..., required bool networkEnabled)`. Node calls and `sendHttp` fail with `GrinNodeException` (`details.reason` `network_disabled`); Rust and Dart tests count zero connections on a local listener. |
| G7 | Throwaway outside consumer: a Flutter app that depends on `grin_native` and `grin` by git (and by path), builds on Linux, and runs one create/open/address call. This settles `resolution: workspace` for consumers. | Builds from a clean pub cache | Not done here. |
| G8 | Device runs of the example app: Android arm64 (rustls HTTPS to a real node), iOS device, sandboxed macOS, Windows x64. | Checklist recorded in `docs/architecture.md` section 13 | Not done here (needs devices). |
| G9 | Measure armored Slatepack lengths for S1 (plain and encrypted), S2, I1 and I2 in `slatepack_e2e_test.dart`, to set the QR threshold. | Numbers recorded | Done in 1613460, from a test added in 9db5a3d: table in `docs/architecture.md` section 13. S1 plain 360, S1 encrypted 806 to 955, S2 1837 to 2185, I1 plain 348, I1 encrypted 719 to 855, I2 1881 to 2068 characters. |
| G10 | Optional: give `processInvoice` an `expectedAmount` argument so the bound is enforced in Rust, not only in `GrinWallet`. | Mismatch returns a Transaction error, nothing locked | Done in 9db5a3d: `expected_amount` in the `grin_tx_process_invoice` arguments, `GrinProcessInvoiceArgs.expectedAmount` (`BigInt?`). A mismatch is `GrinTransactionException` with `invoice_amount` and `expected_amount` details, before anything is recorded or locked. |
| G11 | Docs: fix the README claim that `CARGO_*`/`RUSTUP_*` reach the hook (they don't). Fix D3, D6 and D12. Replace section 9 with a pointer to this plan. | | Done in the commit that added this file: README hook environment, "Private access", D3, D6, D12, section 9 pointer, D14. |

G1, G2, G3 and G4 block S2. G6 can follow S2. G10 is optional because S2 guards invoice amounts in Dart anyway.

## Status (2026-11-07)

Phase S is implemented on git.cypherstack.com, sneurlax/stack_wallet, as stacked pull requests based on staging 4bc21f9e1:

| PR | Branch | Content |
|---|---|---|
| #6 | `refactor/epic-mwc-unify-rebased` -> `staging` | S1 |
| #7 | `feat/slatepack-interface` -> #6 | S3 |
| #8 | `feat/grin` -> `staging` | S2, locked to this repository at 6ff6fe6 |
| #9 | `feat/grin-e2e` -> `feat/grin` | native end-to-end tests, symbol gate script |
| #10 | `feat/grin-ui` -> `feat/slatepack-interface` | S4 and S5, merges #8 and #9 |
| #11 | `feat/grin-ui` -> `staging` | omnibus of #6 to #10 |

Still open: S6 theme assets and CI, G8 device runs (Android, iOS, sandboxed macOS, Windows), and the Epic/MWC manual retests listed in #6 and #7.

Update, 2026-11-11: G7 is settled by Stack Wallet itself, which takes `grin_native` and `grin` by git and builds from a clean pub cache. This repository and the fork were reviewed again; the changes are in `docs/architecture.md` D15, and the fork pin is now `ee6bc01`. Stack Wallet reads the new `finalized` and `released` error details.

## 4. Phase S: Stack Wallet work

Each S phase is one branch cut from `github-cs/staging`, pushed to cypherstack and manymath only. Opening the GitHub PRs is up to you.

### S1. Rebase `refactor/epic-mwc-unify` (about a day, including Epic and MWC retests)

- The branch is 5 commits on merge base 9517439be, 299 behind staging.
- A trial merge conflicts in five files, 16 hunks, about 339 lines: `send_view.dart`, `confirm_transaction_view.dart`, `desktop_send.dart`, `epiccash_wallet.dart` and `mimblewimblecoin_wallet.dart`.
- The conflicts are mechanical. Staging added multi-recipient, the EIP-1559 builder and OpenAlias. Keep staging's `shouldCheckEpicbox` (598898ae8), not the always-true test on the branch.
- S3 needs this. S2 does not.
- Leave `refactor/send-controller` alone; it needs a rewrite (37 hunks, about 2,400 lines). `refactor/tx-views-unify` is optional; if it goes in, it goes right after S1.

### S2. Grin core: plumbing, coin, wallet, lifecycle (no new screens)

Build plumbing:
1. `scripts/app_config/templates/pubspec.template.yaml`:
   - An unconditional `grin:` git dependency.
   - `# %%ENABLE_GRIN%%` around `grin_native:` (git, `path: packages/grin_native`, 40-hex ref) after the MWC block (:60-65).
   - A `dependency_overrides` entry for `grin` at the same ref (the ISAR precedent at :350-366). Explanatory comments stay outside the markers, because `process_pubspec_deps.dart:50` strips one `#` per line inside a block.
   - No `hooks:` block. A second top-level `hooks:` key would collide with `NATIVE_PREBUILTS` under `-d`, and `test_utils` defaults to false.
2. `scripts/app_config/configure_stack_wallet.sh`:
   - `GRIN` in both marker lists (:24-34 and :39-48).
   - `Grin(CryptoCurrencyNetwork.main)` after Firo (:111).
   - `Grin(CryptoCurrencyNetwork.test)` after Firo test (:130), only if open decision 4 gives a testnet node.
   - Campfire and Duo are unchanged. They get the OFF stub automatically.
3. `lib/wl_gen/interfaces/libgrin_interface.dart` and `tool/wl_templates/GRIN_libgrin_interface_impl.template.dart`, per A1. The file prefix must be `GRIN_` (`tool/gen_interfaces.dart:25-31`).
4. Regenerate and commit `pubspec.lock` with Grin enabled.
5. `lib/utilities/native_library_check.dart:60-90`: an entry for `grin_ffi` gated on `AppConfig.coins.whereType<Grin>()`.

Coin definition, `lib/wallets/crypto_currency/coins/grin.dart` (export it in `crypto_currency.dart` between firo and litecoin):

| Member | Value |
|---|---|
| base | `Bip39Currency` |
| networks | main, test (throw on others) |
| `identifier` | `grin` / `grinTestNet` (never change) |
| `mainNetId` | `grin` (theme keys) |
| `prettyName` / `ticker` | Grin / GRIN; tGrin / tGRIN |
| `uriScheme` | `grin` (open decision 7) |
| `genesisHash` | `"not used in grin"` |
| `minConfirms` | 10 (libwallet default; `GrinSendArgs` default) |
| `minCoinbaseConfirms` | 1440 (`COINBASE_MATURITY`) |
| `torSupport` | true (only because S2 routes node and HTTP traffic through the proxy) |
| `fractionDigits` / `satsPerCoin` | 9 / 10^9 |
| `defaultSeedPhraseLength` | 24 |
| `possibleMnemonicLengths` | [24, 12] |
| `hasMnemonicPassphraseSupport` / `hasBuySupport` | false / false |
| `defaultAddressType` / `getAddressType` | `AddressType.mimbleWimble` (only after validation, unlike MWC) |
| `targetBlockTimeSeconds` | 60 |
| `defaultDerivePathType` | throws |
| `defaultBlockExplorer` | open decision 7; guard the desktop explorer dialog either way (`desktop_manage_block_explorers_dialog.dart:157-162` already throws for Epic and MWC) |
| `validateAddress` | `libGrin.backend.validateAddressSync(address, network)` in a try/catch that returns false. It accepts grin1/tgrin1, onion v3 and http(s), and rejects the other network. |
| `defaultNode` | main port 3413, test port 13413, hosts are open decision 4. Host carries the scheme and `useSSL` follows it. `torEnabled` and `clearnetEnabled` true. `isPrimary` is passed through (MWC hard-codes true; don't copy that). No API secret. `Grin(test).defaultNode` must not throw, because `NodeService` calls it for every listed coin at startup (`node_service.dart:79-81`). |
| helpers | `isSlatepack(text)` (`BEGINSLATEPACK.` ... `ENDSLATEPACK.`) and `addressKind(text)` |

Wallet: `lib/wallets/wallet/impl/grin_wallet.dart`, as specified in section 5. Registration and app-wide sites:

| Site | Change |
|---|---|
| `wallet.dart` `_loadWallet` (:353-430) | `case const (Grin): return GrinWallet(net);` |
| `restore_wallet_view.dart:346-363`, `restore_create_backup.dart:481-499`, `restoring_wallet_card.dart:60-79` | `GrinWallet` case calling `init(isRestore: true)` |
| `restore_create_backup.dart:502-510` | Grin in the "restore height 0 means 0" list; otherwise SWB restores from `storedChainHeight` and silently skips older funds |
| `new_wallet_recovery_phrase_warning_view.dart:202-206` | `coin is Grin` gets passphrase `""`; otherwise `Wallet.create` crashes on `mnemonicPassphrase!` when advanced options were opened |
| `restore_options_view.dart:231-243` and pickers at :428-541 | Grin date-to-height. Genesis main 1547568086, test 1546030084, 61 s per block overestimate as Epic uses |
| `lib/services/wallets.dart:147-173` | Grin delete branch, `GrinWallet.deleteWalletFiles(walletId, secureStore)` (section 5.6) |
| `lib/utilities/test_node_connection.dart:143` | `case Grin():` calling new `test_grin_node_connection.dart`, which uses `backend.testNodeConnection` with the resolved transport. The switch has no default, so without this the test silently fails. Don't copy Epic's `/v1/version` probe; v1 sits behind `api_secret` on a Grin node. |
| `add_edit_node_view.dart:86-93, 915-919, 1081-1110, 1254-1301, 1405-1410` | Grin in `_onTestSuccess`, the SSL branches and the API-secret field; onion hosts forced tor-only |
| `node_service.dart:96-110` | `updateDefaults` keeps `nodeApiSecret` on default nodes |
| `lib/services/price.dart:28-54` | `Grin: "grin"` (CoinGecko id, verify) |
| `amount_unit.dart:98-106` | Grin in the `n${ticker}` branch (MWC shows "sats"; don't copy that) |
| `desktop_startup_and_reset.dart:29-40` | `'grin'` in the reset list |
| `wallet_network_settings_view.dart:158, 278-283, 357-370, 647-651, 971-991` | Grin cases for rescan text, blocks remaining and `highestPercent`. Show the Tor and Advanced options. |
| `wallet_settings_wallet_settings_view.dart:595-597`, `wallet_options_button.dart:501-503` | Show the refresh-height option for Grin; the generic `EditRefreshHeightView` branch already works |
| `transaction_v2.dart:99-104, 257-336, 404-423` | `isGrinTransaction` key and Grin `statusLabel` branch (section 5.3) |
| `tor_has_been_add_dialog.dart:156` | Leave Grin out of the "Tor doesn't work for" text |
| `send_view.dart:673-697, 1606, 2760-2815` and desktop equivalents | Skip fee recalculation and hide the on-chain note for Grin. Grin ignores `message`. |
| `README.md:21-22` | List Grin |
| `integration_test/wallet_creation/coin_matrix.dart`, `seed_validators.dart`, `desktop_forgot_password/harness.dart:41-45` | `'grin': 24`; Grin wallet path |

S2 shows Grin wallets with balance, history, receive address and node settings. Send, receive and finalize go through the existing per-coin UI only where a generic path exists. The real flows land in S3 and S4. If S2 has to be usable alone, the stopgap is to wire Grin into the MWC slatepack screens behind an `is GrinWallet` check. That is not recommended: it forks the screens a third time.

### S3. Generalized Slatepack UI (on top of S1)

1. Extract `SlatepackInterface<T>` (members in section 6). `MimblewimbleWallet` and `GrinWallet` mix it in. Switch the concrete casts and `coin is` gates to the interface. The sites:
   - `send_view` :891 / :978
   - `confirm_transaction_view` :160 / :241 / :395-445
   - `desktop_send` :206 / :331 / :533-543
   - `receive_view` :104 / :165
   - `desktop_receive` :118 / :178 / :417-418
   - the import dialogs, `finalize_view` and `epic_finalize_view`
   - `wallet_view` :1014-1043
   - `my_wallet` :52-66, :187-196
   - `transaction_v2_details_view` :1408-1600
   - `wallet_network_settings_view` :357-370
   - `preview_tx_button_state_provider` :28-75
2. Route Slatepack sends through `prepareSend` and then `ConfirmTransactionView`. Today mobile and desktop skip the confirm screen and never show a fee (`send_view.dart:883-1050`, `desktop_send.dart:533-543`). Grin's `initSend` locks inputs, so a confirm step is mandatory. Epic and MWC users get the fee display too. Delete Epic's dead slatepack branch on the confirm screen.
3. A state router, `SlatepackImportFlow.run(context, walletId, text)`. It calls `inspectSlatepack` and dispatches on the state:

   | State | Goes to |
   |---|---|
   | S1 | receive |
   | S2 | finalize send |
   | I1 | pay invoice, when `supportsInvoices` |
   | I2 | finalize invoice |
   | S3 / I3 | "already finalized" (offer repost if stored) |
   | unknown | "unsupported slate" |

   It replaces `fullDecodeSlatepack` and `analyzeSlatepack`. Decoding an S2 or I2 gives amount 0 (`GrinSlateInfo.amount`), so finalize summaries take amount and fee from the local transaction found by `slateId`.
4. One `SlatepackDialog` for S1, S2, I1 and I2 with copy that depends on the state:
   - Copy, share (the current share button is a "coming soon" stub, `mwc_slatepack_dialog.dart:47-54`), save to file as `<slateId>.<state>.slatepack`, and an encrypted flag.
   - The QR shows only below the G9 threshold, with tap-to-enlarge.
   - `QR` gets an `errorStateBuilder`. Today an oversized QR renders blank (`lib/widgets/qr.dart:17-22`, qr_flutter level L).
5. Input: paste, scan and file. On desktop, scan uses `QrCodeScannerDialog` as in `frost_step_field.dart:76-95`. Today `finalize_view.dart:82` and `slatepack_entry_dialog.dart:60` call the mobile-only scanner on desktop. File import uses `file_picker` with a size cap.
6. Make the transaction-method selection per wallet; today it is global (`preview_tx_button_state_provider.dart:28-36`). On desktop, always show the receive address; today it is hidden in slatepack mode (`desktop_receive.dart:608-610`).
7. Strings are inline English; the app has no localization. Use the unify branch's `slatepack_terms.dart` for "slate" versus "slatepack" wording.

### S4. Grin flows

The full screen traces are in the UI agent's report. The essentials:

Send by Slatepack:
1. Method toggle.
2. Optional grin1 address. When present, the S1 is encrypted to it and a payment proof is requested.
3. `prepareSend` calls `estimateFee`.
4. The confirm screen shows amount, fee and target.
5. `confirmSend` calls `initSend`.
6. `SlatepackDialog` shows the S1 with "funds locked until finalized or cancelled".
7. Later, finalize the S2 through the router.
8. Errors:
   - A Node error carrying `slateId` means "finalized, not broadcast", and the user is offered Repost.
   - NotFound means "not a pending send from this wallet".

Send by HTTP(S) or Tor:
- The address accepts grin1, onion or `http(s)://`. `confirmSend` calls `sendHttp(post: true)` under `SendingTransactionDialog`.
- The confirm screen enforces the transport policy in section 7:
  - A clearnet target with Tor off requires an acknowledged IP-disclosure warning.
  - An `http://` target gets a second warning, even over Tor.
  - An onion or grin1 target without a connected proxy is blocked.
- Error copy:
  - Http error: "nothing sent or locked".
  - Slate or Transaction error on the answer: "the recipient's answer failed checks; nothing locked".

Receive:
- Address card with the grin1 address and its QR. "New address" is hidden; only index 0 receives, and the copy says so.
- Import S1 through paste, scan or file, then the router, then an import dialog showing amount, sender, the encrypted flag and the TTL.
- `receiveSlatepack` then returns the S2 in `SlatepackDialog`.

Transaction details, on the generic path (do not set the Epic/MWC flags):
- Rows: slate id, kernel excess (copy), counterparty, TTL cutoff, and "finalized, not broadcast".
- Actions:
  - Cancel: merge the two copy-pasted Epic/MWC branches.
  - Show Slatepack again.
  - Repost: finalized, unconfirmed and `hasStoredTx`.
  - Export payment proof.
- Cancelling a finalized but unconfirmed transaction gets a warning.

Wallet settings:
- Rescan (non-destructive).
- Refresh height.
- Node API secret.

### S5. Invoices and payment proofs (if in v1, open decision 5)

- Issue I1:
  - Mobile: a "Request payment" button on the receive view opens `RequestInvoiceView` (amount, fiat, optional payer grin1).
  - Desktop: the same form in a dialog.
  - Both call `issueInvoice` and show the result in `SlatepackDialog`.
- Pay I1: the router opens `PayInvoiceConfirmView`, modelled on the confirm screen.
  1. It shows the payee amount large, the payee address, the encrypted flag, the TTL, the fee from `estimateFee` (the I1 carries fee 0), the total and the remaining balance.
  2. Explicit confirm and lock screen.
  3. `GrinWallet.processInvoice(i1, confirmedAmount:)` decodes again, refuses on any amount or state mismatch, and only then calls the library.
  4. If the I2's fee exceeds the estimate, warn and offer cancel before showing it.
- Finalize I2 through the router. The summary comes from the local issued invoice.
- Payment proofs:
  - Export as JSON (copy, share or save) for outgoing finalized transactions that have a proof.
  - "Verify payment proof" in wallet settings (mobile) or the options menu (desktop). Show amount, excess, both addresses and `isSender`/`isRecipient`. Label the result "kernel presence as reported by your node".

### S6. CI, release, themes

CI:
1. Fork access is no longer needed after G1.
2. In container jobs, `ln -s /usr/local/rustup "$HOME/.rustup"; ln -s /usr/local/cargo "$HOME/.cargo"`. Add Rust 1.99.1 plus Grin's targets to the Dockerfile stages. This speeds up frostdart too.
3. Cache `~/.rustup` and `~/.cargo/{git,registry}` on the macOS and Windows runners, keyed on Grin's `Cargo.lock`.
4. After each `flutter build`, a symbol gate on the bundled `grin_ffi`: exactly 35 `grin_` exports and 0 `grin_test_`.
5. Don't run Grin's own suite in Stack Wallet CI; it belongs in the grin repo's CI.

stack_releases:
- Release hosts need rustup and HTTPS access.
- Check Windows path length; the hook target sits deep under `.dart_tool`.
- No script changes are needed for Linux, macOS, iOS or Android. Flutter embeds and signs the framework or library.

Guix (`feat/guix`, not on staging):
- `RUST_SHA256` and `RUST_SRC_SHA256` for 1.99.1 in `prelude.bash`.
- `cargo vendor --locked` must reach the fork.

Themes:
- Add `assets.coins.{icons,images,secondaries}.grin`, optional cards, `colors.coin.grin`, and `assets/svg/coin_icons/{small,big}/grin.svg` to all four default theme zips (`asset_sources/default_themes/{stack_wallet,stack_duo}/{light,dark}.zip`).
- Bump theme version 18 to 19 in each `theme.json` and in `_currentDefaultThemeVersion` (`theme_service.dart:34`). Precedent: Fact0rn, d831ef4e18.
- Theme-server themes need the same keys outside this repo.
- Do not touch the legacy Isar `ThemeAssets` schema.
- Without theme entries, Grin shows a placeholder icon and does not crash.

## 5. GrinWallet specification

### 5.1 Fields

```dart
class GrinWallet extends Bip39Wallet<Grin> {
  GrinWallet(CryptoCurrencyNetwork net, {GrinWalletBackend? backend});
  static final Map<String, GrinWallet> _owners = {};     // one owner per walletId
  static final Map<String, Mutex> _dirLocks = {};        // open/create/close/delete
  GrinWalletApi? _api;
  Future<void>? _initializing, _closing, _scanJob;
  GrinTransport? _appliedTransport;
  StreamSubscription? _torStatusSub, _torPrefSub;
  double highestPercent = 0;
}
```

### 5.2 Member mapping

Conventions for every row:
- Amounts convert as `Amount.raw` to and from `BigInt` nanogrin, asserting 9 digits.
- `minimumConfirmations` is always `cryptoCurrency.minConfirms`, for both the estimate and the send.
- Every `GrinException` is caught and rethrown as a Stack exception that carries type, code and `slateId`, never the remote message (section 8).

| Member | Calls | Behaviour |
|---|---|---|
| `init({isRestore})` | `open`; on WalletNotFound, `create(isNewWallet: true)` | Await `_closing`; coalesce through `_initializing`; idempotent, because opening a wallet calls `init` again on the same instance. Subscribe to Tor events before building the config, which carries the resolved proxy (dead proxy if Blocked). With `isRestore: true`, open nothing. Never starts a scan, because startup calls `init` on every wallet (`wallets.dart:562-571`). Register in `_owners`; if another live owner exists, throw a readable error. Then `super.init()`. |
| `checkSaveInitialReceivingAddress` | `getAddress(index: 0)` | Store once as `mimbleWimble`, receiving, index 0. Not a `MultiAddressInterface`. |
| `recover(isRescan: false)` | `create(mnemonic)`, then `scan` | Under `refreshMutex`: write the password, then create. On WalletExists with `leftover_wallet_data`, call `delete` and retry (new id, mnemonic in secure storage). Store the address, set `pendingScanFrom = info.restoreHeight`, start the scan job unawaited. |
| `recover(isRescan: true)` | `cancelScan`, `scan(startHeight: restoreHeight, deleteUnconfirmed: false)` | Non-destructive; history is kept and Isar upserts. Creates the wallet first if it is missing (the SWB retry path). |
| `refresh()` | none | If no session, return. If a scan is pending, run the scan job. Otherwise `super.refresh()`, which coalesces (`wallet.dart:553-606`). |
| `updateChainHeight` | `backend.testNodeConnection(...).height` | Stateless, so it never queues behind a scan. |
| `updateTransactions` | `getTransactions(refreshFromNode: !scanPending)` | Convert per 5.3 and upsert. If `refreshedFromNode == false`, remember that we are offline. |
| `updateBalance` | `getBalance(refreshFromNode: false)` | Map per 5.4. Then throw if offline, so the base refresh fires `unableToSync`. |
| `updateUTXOs` | none | `false` |
| `updateNode` | `updateNetwork(nodeUrl, apiSecret, socksProxy)` | Through `_applyTransport()` (section 7). Cancel a running scan first; it resumes from `scanned_height + 1`. |
| `pingCheck` | `backend.testNodeConnection` | false when Blocked or on any error. Every 30 s. |
| `prepareSend` | `estimateFee(amount, amountIncludesFee: sendAll)` | Exactly one recipient. Method comes from the UI toggle or the address kind. Locks nothing. Refused while a scan is pending. |
| `confirmSend`, Slatepack | `initSend(GrinSendArgs(... recipientAddress: grin1?, ttlBlocks))` | Locks inputs. Returns `txid = slateId` and the armored S1 in a typed TxData field. |
| `confirmSend`, HTTP | `sendHttp(GrinHttpSendArgs(... post: true))` | Transport policy first. Nothing is locked if the exchange fails. |
| `estimateFeeFor` | `estimateFee` | Zero for amount <= 0 (the library rejects 0). Insufficient funds becomes `InsufficientBalanceException`. |
| `fees` | none | Constant `FeeObject` |
| `exit` | `cancelScan`, `close` | Under the dir lock: leave `_owners`, cancel Tor subscriptions, null `_api`, await close (which cancels and drains), `super.exit()`. Calls that arrive after `exit` return quietly; Grin never reopens by itself. |
| `cancelPendingTransactionAndPost(slateId)` | `cancelTransaction` for each stored log id | Returns `""` or a mapped message (the existing UI contract). Refused while a scan is pending. |
| `repost`, `exportPaymentProof`, `verifyPaymentProof`, `pendingSlatepackFor` | matching API calls | Gated on `hasStoredTx` or `hasPaymentProof` from otherData. The armored Slatepack is served from a per-refresh cache, not stored in otherData, because `_getFromOtherData` JSON-decodes on every access. |

### 5.3 GrinTxLogEntry to TransactionV2

- Group by `slateId`. A sent plus received pair whose counterparty is our own address becomes one `sentToSelf` row.
- Type mapping:

  | Entry type | TransactionType |
  |---|---|
  | `txReceived`, `confirmedCoinbase`, `txReceivedCancelled` | incoming |
  | `txSent`, `txSentCancelled` | outgoing |
  | `txReverted` | by the sign of the net amount |
  | `unknown` | unknown |

  subType is always none.
- Synthetic inputs and outputs:
  - Owned input of `amountDebited`, if it is greater than 0.
  - Coinbase: one unowned zero-value input with `coinbase: "grin"`, so `isCoinbase()` applies the 1440-block maturity.
  - Owned output of `amountCredited`, if it is greater than 0.
  - Outgoing only: an unowned output of `debited - credited - fee` addressed to the counterparty.
- Fee goes in `otherData.overrideFee`, always.
- Height:
  - null while unconfirmed, cancelled or reverted.
  - `confirmedHeight` once confirmed. If that is null, keep the stored height; if there is none, use the cached chain height at the first refresh that sees it confirmed.
  - Never `kernelLookupMinHeight`. It is a lower bound and inflates confirmations; Epic uses it today.
- otherData keys:

  | Key | Notes |
  |---|---|
  | `isGrinTransaction` | |
  | `slateId` | |
  | `isCancelled` | |
  | `grinLogIds` | list |
  | `grinSlateState` | |
  | `grinHasStoredTx` | |
  | `grinKernelExcess` | |
  | `grinTtlCutoff` | |
  | `grinHasPaymentProof` | |
  | `counterpartyAddress` | |
  | `isReverted` | |

- `statusLabel`, checked in this order:

  | Condition | Label |
  |---|---|
  | Incoming without a slate and not coinbase | "Restored funds" |
  | Cancelled | "Cancelled" |
  | Reverted | "Reverted" |
  | Confirmed | "Received" / "Sent" |
  | S1, outgoing | "Sending (waiting for response)" |
  | S2, incoming | "Receiving (waiting for sender to finalize)" |
  | I1 | "Invoice issued (waiting for payer)" |
  | I2, outgoing | "Paying invoice (waiting for invoicer)" |
  | Otherwise | "Sending" / "Receiving" with the confirmation count |

### 5.4 Balance

- spendable = `amountCurrentlySpendable`.
- pending = `amountAwaitingConfirmation + amountImmature + amountAwaitingFinalization`.
- total = spendable + pending.
- blocked = 0.
- libwallet excludes locked outputs from its total (`libwallet/src/internal/updater.rs:874-884`). Whether to show `amountLocked` somewhere is open decision 13.

### 5.5 Scan job

1. Fire `syncing`.
2. Every second, read `scanProgress()` (synchronous and lock-free) and fire `RefreshPercentChangedEvent` and `BlocksRemainingEvent`.
3. Await `scan(startHeight: pendingScanFrom)`.
   - Success: clear `pendingScanFrom`, then `refresh()`.
   - `GrinCancelledException`: set `pendingScanFrom = scanned_height + 1`, no error event. After G5, `lastScannedHeight + 1` replaces this.
   - Node error: keep the value and fire `unableToSync`.

Scans start only from `recover()` and from `refresh()` when one is pending. While a scan runs:
- Sends are refused.
- Receive, finalize and decode queue behind the current batch. An unencrypted Slatepack can be inspected statelessly with `decodeSlatepackWithoutWallet`.

### 5.6 Delete

`GrinWallet.deleteWalletFiles(walletId, secureStore)`:
1. Take the dir lock.
2. `exit()` the `_owners[walletId]` instance, which may be unregistered.
3. `backend.delete(walletDir)`, which removes only `wallet_data` and needs no open wallet.
4. Remove the now-empty `<walletId>` directory without recursion.
5. Delete `${walletId}_grinWalletPassword`.

On WalletBusy, record the id and retry at the next load. The deferred-delete pass (`wallets.dart:231-245`) currently removes only `WalletInfo`, so it needs a Grin hook.

## 6. SlatepackInterface (S3)

```dart
enum SlatepackMethod { slatepack, relay, http }
mixin SlatepackInterface<T extends CryptoCurrency> on Wallet<T> {
  List<SlatepackMethod> get supportedMethods;      // Epic/MWC: slatepack, relay; Grin: slatepack, http
  bool get supportsOnChainNote;                    // Grin: false
  bool get supportsInvoices => false;              // Grin: true
  double get restoreProgress;                      // replaces highestPercent casts
  Future<String> slatepackAddress();
  Future<SlateInspection> inspectSlatepack(String armored); // state, slateId, amount?, fee?, sender?, isEncrypted, ttlCutoff?
  Future<({Amount amount, Amount fee})?> localTxForSlate(String slateId);
  Future<SlatepackResult> createSlatepack({required Amount amount, String? recipientAddress, bool sendAll = false});
  Future<SlatepackResult> receiveSlatepack(String s1);
  Future<FinalizeResult> finalizeSlatepack(String s2);         // FinalizeResult gains posted, kernelExcess
  Future<String> cancelPendingTransactionAndPost(String slateId);
  Future<String?> pendingSlatepackFor(String txid) async => null;
  Future<void> repost(String txid) => throw UnsupportedError("repost");
  Future<SlatepackResult> issueInvoice({required Amount amount, String? payerAddress}) => throw UnsupportedError("invoices");
  Future<SlatepackResult> processInvoice(String i1, {required Amount confirmedAmount}) => throw UnsupportedError("invoices");
  Future<FinalizeResult> finalizeInvoice(String i2) => throw UnsupportedError("invoices");
  Future<String> exportPaymentProof(String txid) => throw UnsupportedError("proofs");
}
```

`SlatepackResult` gains `state`, `slateId`, `amount` and `fee`. `isSlatepack` is not called from the UI and goes.

## 7. Tor and nodes

Inputs to the policy:

| Symbol | Meaning |
|---|---|
| F | Tor feature compiled in |
| U | `prefs.useTor` |
| S | Tor status (read only if F) |
| K | `prefs.torKillSwitch` |

| F and U | S | Node option | K | Transport |
|---|---|---|---|---|
| false | any | tor-only, or onion host | any | Blocked(torOnlyNodeTorOff) |
| false | any | clear or both | any | Direct |
| true | any | clear-only | any | Blocked(clearnetNodeTorOn) |
| true | connected | tor or both | any | Proxy("127.0.0.1:<port>"), port re-read on every resolve |
| true | connecting | any | any | Hold node calls on `_torConnectingLock`; keep the current transport |
| true | disconnected | tor or both | true | Blocked(torDown) |
| true | disconnected | both | false | Direct for node calls; HTTP send stays refused (open decision 8) |

`_applyTransport()` runs under a mutex:
1. Resolve.
2. If the result equals the applied transport, stop.
3. Cancel any running scan.
4. `updateNetwork(nodeUrl, apiSecret, socksProxy: proxy, dead proxy or null)`.
5. Record the applied transport.
6. If not Blocked, refresh. If Blocked, fire `NodeConnectionStatusChangedEvent(disconnected)` and `unableToSync`.

It runs on Tor status `connected` or `disconnected`, on `TorPreferenceChangedEvent`, and on `updateNode()`.

Every node-touching call passes `_requireNetwork()`, which waits on the connecting lock and throws `NodeTorMismatchConfigException` or a Grin Tor exception when Blocked:
- `getChainHeight`
- refresh calls
- `scan`
- `estimateFee`
- `initSend`
- finalize with broadcast
- `processInvoice`
- `repost`
- `cancelTransaction`
- `verifyPaymentProof`
- `sendHttp`

Note: today only the Electrum paths honour the kill switch. Epic and MWC pass no proxy at all, and Epic's HTTP send goes over clearnet with Tor on. Grin must not copy them.

Nodes:
- Node URL builder: scheme from the host prefix or `useSSL`, then the port. Reject userinfo with a clear message. Strip the trailing `/`; the library appends `/v2/foreign`.
- Onion nodes: `http://` only, because rustls can't accept a self-signed certificate, and only with Tor connected.
- `GrinNodeInfo` has no chain id, so a testnet node set on a mainnet wallet is not detected (open decision 15).

## 8. Security requirements carried from the review

1. Invoice amount bound: decode again and compare against the confirmed amount before `processInvoice`. The fee comes from `estimateFee`. G10 moves the check into Rust.
2. Clearnet HTTP send:
   - IP-disclosure acknowledgement when Tor is off.
   - Refusal when Tor is on but down.
   - A plaintext warning for `http://`, since the Tor exit can read the slate.
3. `walletDir` is always absolute; assert `p.isAbsolute` before create, open and delete.
4. Logs never contain Slatepacks, `slateJson`, mnemonics, passwords, `NodeModel` objects (whose `toString` includes the API secret) or remote error text.
   - `GrinException.toString()` includes the remote message, and the base refresh logs the raw error (`wallet.dart:589-593`). That is why every override rethrows a sanitized exception.
   - Don't call `initLogs` in release builds; in debug, use level `warn`.
5. Remote text shown in the UI is labelled as coming from the recipient, stripped of control characters and capped in length. Never show the canonical `walletDir` from WalletBusy.
6. Only the index-0 address is shown.
7. No password change UI. The seed is protected by the 128-bit generated password.
8. Node API secret: `NodeModel.nodeApiSecret` is plaintext in Hive and goes into SWB, as for MWC today. Open decision 9.

## 9. Test plan

Unit tests in Stack Wallet CI need no native code, because `package:grin` is unconditional. Fakes go in `test/wallets/support/grin_fakes.dart`:
- `FakeGrinBackend` and `FakeGrinWalletApi`, with scripted results, a call log including `updateNetwork` arguments, scan completers and events.
- Fake Tor and prefs.
- `MemorySecrets`, moved to `test/support`.

The Xelis fakes (`test/wallets/support/xelis_test_fakes.dart`) are the precedent.

| Test | Asserts |
|---|---|
| `grin_transport_test` | The section 7 table; event sequences for enable and disable; Blocked never calls `updateNetwork(socksProxy: null)`; `cancelScan` before `updateNetwork` |
| `grin_send_policy_test` | Address kind against transport; refusals make zero `sendHttp` calls; warning flag required |
| `grin_invoice_test` | Amount or state mismatch means no `processInvoice` call |
| `grin_lifecycle_test` | Open, or create on WalletNotFound; password before create; absolute dir from a relative root; idempotent init; exit awaits close; delete order; busy delete recorded; restore-cancel delete of an unregistered instance |
| `grin_scan_test` | No `refreshFromNode: true`, cancel or send while a scan is pending; resume from `scanned_height + 1`; progress events |
| `grin_tx_mapping_test` | Self-send merge, coinbase maturity, `overrideFee`, sticky height, every status label |
| `grin_errors_test` | No remote message in user strings or captured log output; no Slatepack or mnemonic in logs |
| `grin_coin_test` | Fields, `defaultNode` main and test, `validateAddress` with a fake backend |
| `grin_node_url_test` | Scheme, port, onion, userinfo, trailing slash |
| `grin_swb_test` | Backup holds mnemonic, `""`, `restoreHeight` and grinData, and no password; restore calls create then `scan(restoreHeight)` (Isar, modelled on `xelis_swb_test`) |

Native end-to-end runs on a desktop host, opt-in, with the library built with `test_utils`.
- `integration_test/grin/grin_test_chain_test.dart` drives `GrinWallet` against `GrinTestChain`.
  - The flows: mine, Slatepack S1/S2/S3, invoice I1/I2/I3, cancel, repost, proof export and verify, restore with height, and delete.
  - Its backend wrapper must record and swallow `updateNetwork`, because the real call replaces the in-process client with HTTP.
- A SOCKS test uses the fake SOCKS5 server from `ethereum_tor_proxy_test.dart` and asserts a domain-type CONNECT, which proves socks5h with no local DNS.
- A regtest test, gated by `GRIN_REGTEST_*` like `XELIS_LOCAL_DAEMON`, reuses `tool/regtest.sh`.
- `test_utils` goes only into a generated test pubspec, never into the template. The release symbol gate (S6) proves it stayed off.

Manual on each platform before release:
- Create, restore and SWB round trip.
- Slatepack send and receive both ways with grin-wallet CLI.
- HTTP send over Tor to a CLI listener.
- Rescan, delete, and node switch during a scan.
- Tor toggle during a scan.

## 10. Risks

1. An ungated node refresh on a never-scanned wallet starts an uncancellable genesis scan. It blocks exit, delete and reopen. G4 plus the gate in A4 address it.
2. Head-of-line blocking: user operations wait behind a scan batch or behind the 2880-block refresh window, which is slow over Tor.
3. The public mirror or the outside-consumer proof (G1, G7) slips, which forces the copied-types fallback and keeps CI unable to build Grin.
4. LMDB under the macOS sandbox and on iOS stays unverified until G3 and G8.
5. Slatepack QR codes too dense to scan (G9). Copy, share and file are the primary actions.
6. Rebase drift: S1 and staging both keep moving in the send views.
7. Size: about 14.5 MB per Android ABI. Stripping symbols and LTO on non-Apple targets is optional (open decision 17).

## 11. Open decisions (my recommendation in brackets)

1. Public mirror host and name for the grin repo and the grin-wallet fork, and which Gitea URL is canonical. [github.com/cypherstack/grin_native and github.com/cypherstack/grin-wallet; canonical Gitea `grin_native.git` to match]
2. Native delivery. [Source build for v1; dnix later together with Epic and MWC]
3. Rust pin. [1.99.1]
4. Default node hosts for mainnet (3413) and testnet (13413), and whether testnet ships in the default coin list. [Ship testnet only if there is a node we run]
5. Invoices and payment-proof UI in v1. [Both, since the library supports them and the review covered them; proofs as export plus verify]
6. Land `refactor/epic-mwc-unify` first and extract `SlatepackInterface` in S3. [Yes]
7. URI scheme, receive QR content, explorer. [`grin` scheme, raw `grin1...` in the QR for interop with other wallets, no explorer link until a kernel-search URL is chosen]
8. Kill switch off and Tor down: clearnet fallback for node calls only. [Yes; HTTP send stays refused]
9. Node API secret storage. [`nodeApiSecret` for parity with MWC now; move every coin's secret to secure storage separately]
10. TTL default on Slatepack sends and invoice payments. [1440 blocks, about a day, so stuck sends unlock themselves]
11. On WalletNotFound or InvalidPassword for an existing wallet: rebuild automatically from the stored mnemonic and rescan, or ask. [Ask]
12. App pause: cancel scans on pause (copy `GrinLifecycleObserver` from the example app) or let them run. [Cancel on pause on mobile]
13. Show `amountLocked`. [In the balance details only]
14. Rename the "Finalize" nav item and tab to "Slatepack" with the router. [Yes]
15. Detect mainnet/testnet node mismatch. [Later, with a chain id in `GrinNodeInfo`]
16. QR threshold and multi-part QR. [Threshold from G9; no multi-part in v1]
17. Strip symbols and LTO on non-Apple targets. [Yes, after G8]
18. iOS backup exclusion for `<root>/grin`, and mobile "delete account" leaving native directories. [Both are app-wide issues; separate tickets]

## 12. Stale points in docs/architecture.md section 9

- Baseline 7ce7b34f2 is 113 commits behind 4bc21f9e1. Almost every line citation has moved; the agent reports have current lines.
- `MimblewimbleWallet`, `slatepack_models.dart`, `mimblewimble_transaction.dart` and `slatepack_import_dialog.dart` exist only on the unify branch. Staging has per-coin epic_/mwc_ copies.
- 9.2: the mirror-types boundary is replaced by A1.
  - The registration list misses: the three `init(isRestore)` switches, the SWB restore-height list, the passphrase list, desktop reset, the node editor, the amount unit, the theme version bump, `test_node_connection`, `native_library_check`, and the integration matrix.
  - `validateAddress` must catch the native exception.
  - The testnet default node must exist before `Grin(test)` is listed.
- 9.3: the API names are stale:

  | 9.3 says | Now |
  |---|---|
  | `getTxs` | `getTransactions` |
  | `cancelTx` | `cancelTransaction` |
  | `testConnection` | `testNodeConnection` |
  | `receiveSlatepack(message)` | takes no message |
  | config `minConfirms` | does not exist |

  It also lacks the first-scan gate. Its balance mapping is wrong against `updater.rs:874-884`. Its delete ignores unregistered instances.
- 9.4: misses the Tor event order and the fact that only Electrum honours the kill switch. Its kill switch makes the wallet impossible to open offline. Its "old proxy" fallback does not exist at open time.
- 9.5: its fields were assumed. The real ones are `slateId`, `type`, `slateState`, `confirmedHeight` and `hasStoredTx`. It lacks the self-send collision, the empty-list `getFee` crash and sticky heights.
- 9.7: the base `exit()` now sets `shouldAutoSync = false`. Startup inits every wallet. Scan persistence is resolved: per batch, resume at `scanned_height + 1`.
- 9.8: the SSH URLs are unusable for CI. The "pre-kit source builds" claim is wrong: CI uses Epic and MWC prebuilts and only release hosts build them. Epic and MWC upstream have since moved to dnix.
- D3: the Rust 1.94.0 pin. D6: still names `packages/grin/rust`. D12: says nothing of the `grin` override or the example app inheriting `test_utils: true`.
