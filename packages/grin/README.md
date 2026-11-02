# grin

Dart bindings and a typed wallet client for [Grin](https://grin.mw), over the
`grin-ffi` Rust library in `rust/`. The package is pure Dart: its build hook
builds the native library and bundles it as a code asset, so the same package
serves Dart programs, Flutter apps and tests.

Requires Dart 3.13+ (or a Flutter SDK containing it).

## Design

1. `rust/` wraps the `grin-wallet` libraries behind a C ABI. Every
   call is synchronous and returns a `GrinFfiResult` with JSON text. The JSON
   contract is in `docs/ffi-json.md` at the repository root.
2. `lib/src/ffi` holds the generated `@Native` bindings and the small layer
   that frees results, maps error JSON to a sealed `GrinException` hierarchy
   and wipes secrets (passwords, mnemonics) from native memory.
3. `GrinWalletClient` is the API you use. It runs each call in a short-lived
   isolate, queues the calls of one wallet so they run in order, and closes
   the native handle deterministically (and through a `NativeFinalizer` if you
   forget).
4. Amounts are `BigInt` nanogrin. `formatGrin` and `parseGrin` convert to and
   from decimal grin text without floating point.
5. `grin_flutter` (a separate package) adds Flutter conveniences on top.

## Usage

```dart
import 'package:grin/grin.dart';

Future<void> main() async {
  final wallet = await GrinWalletClient.create(
    config: const GrinWalletConfig(
      walletDir: '/data/grin/main',
      network: GrinNetwork.mainnet,
      nodeUrl: 'https://grinnode.example.com:3413',
    ),
    password: 'correct horse',
  );
  try {
    final address = await wallet.getAddress();
    print(address.slatepackAddress); // grin1...

    final balance = await wallet.getBalance();
    print('${formatGrin(balance.spendable)} grin spendable');
  } finally {
    await wallet.close();
  }
}
```

Things to know:

- Every failure is a `GrinException`. `GrinWalletBusyException` means the
  wallet directory is already held by an open wallet in this process (or by a
  closed one that has a call still running). It is reported and never retried.
  After `await client.close()` the directory is free again.
- Calls on a closed client throw `StateError`.
- `scanProgress()` and `cancelScan()` are direct calls that take no wallet
  lock, so they answer while a scan runs. `cancelScan()` stops a running scan
  at the end of its current batch, and `scan()` then throws
  `GrinCancelledException`; see the notes in `docs/ffi-json.md`.
- `validateAddress` has a synchronous twin, `validateAddressSync`, for
  callers that cannot await (the check is stateless and fast).
- Restoring from a mnemonic needs a `scan()` to find the wallet's outputs.
- `initSend` and `processInvoice` lock the wallet's inputs when they return.
  Cancel a transaction that will not be completed with `cancelTransaction`.
  `sendHttp` locks only after the recipient's wallet has answered.
- Do not repost a confirmed transaction, and do not run two node calls on one
  wallet from outside the client: the client queues its own calls.

## Sending with Slatepacks

A send is three steps, two on the sender's wallet and one on the recipient's.
The Slatepacks travel between the wallets by any channel you like.

```dart
// Sender: build the S1, encrypted to the recipient's address. Passing the
// recipient's address also requests a payment proof.
final s1 = await sender.initSend(
  GrinSendArgs(
    amount: parseGrin('1.5'),
    recipientAddress: recipientAddress.slatepackAddress,
  ),
);
// Hand s1.armoredSlatepack to the recipient.

// Recipient: add their output and signature.
final s2 = await recipient.receiveSlatepack(s1.armoredSlatepack);
// Hand s2.armoredSlatepack back to the sender.

// Sender: finish the transaction and broadcast it.
final done = await sender.finalizeSlatepack(s2.armoredSlatepack);
print('${done.slateId} posted: ${done.posted}');
```

`decodeSlatepack` shows what a Slatepack contains before you act on it, and
`getTransactions` lists the history with the latest Slatepack of each entry.
`sendHttp` does the whole exchange in one call when the recipient's wallet is
listening on a URL or a Tor address.

An invoice runs the other way. The payee calls `issueInvoice` and hands the I1
to the payer. The payer calls `processInvoice`, which selects and locks the
inputs, and hands the I2 back. The payee calls `finalizeInvoice` to broadcast.
`exportPaymentProof` returns the sender's proof of a finalized send, and
`verifyPaymentProof` tells either wallet whether it is the proof's sender or
recipient.

## Tests

```sh
dart test
```

The workspace `pubspec.yaml` sets `test_utils: true`, so the first run builds
the Rust library with the host toolchain and the test functions (several
minutes). `package:grin/testing.dart` exposes `GrinTestChain`, which creates
wallets on an in-process chain and mines blocks to them. It only works with a
library built with `test_utils`.

Regenerate the C header and the bindings with
`dart run tool/regen_bindings.dart` from the repository root (needs cargo and
libclang). With `--check` it fails if the committed files are out of date.

## Native library

The hook (`hook/build.dart`) builds `rust/` with the host's rustup and cargo
through `native_toolchain_rust`'s `RustBuilder`: `cargo build --locked --lib`
in release profile, with the Rust version pinned by `rust/rust-toolchain.toml`.
Android links with the NDK clang for the app's API level, so no symbol newer
than the app's floor is pulled in; Apple builds get the app's deployment
floors and keep debuginfo (Xcode strips the product itself); Linux uses the
compiler Flutter or Dart hands the hook. The library is bundled as the code
asset `package:grin/src/ffi/bindings.g.dart`.

There is nothing to configure in an app. The one user-define the hook reads
is for this repository's own tests:

```yaml
hooks:
  user_defines:
    grin:
      test_utils: true   # this workspace only; never in an app
```

`test_utils` (boolean, default false) adds `--features test-utils`, which
exports the `grin_test_*` functions that `GrinTestChain` needs. This
repository's workspace `pubspec.yaml` sets it so that `dart test` works, which
also applies to the example app; those builds are not release artifacts.
`tool/ci.dart` checks with `llvm-nm` that release builds made without it
export the 35 standard functions and none of the test ones.

There are no prebuilt or reproducible builds yet. Release artifacts are a
later step, planned on [dnix](https://github.com/cypherstack/dnix); see
`docs/architecture.md` D11.

The setting is a user-define, not an environment variable, because the hooks
runner that `dart test`, `dart run` and `flutter build` use starts hooks with a
filtered environment: only PATH, HOME, the temp and proxy variables, the
Android NDK variables, a few Windows ones, and names starting with `CARGO_`,
`CCACHE_`, `DOTNET_`, `NIX_`, `NUGET_`, `RUSTUP_` or `CONAN_` get through (see
`includeHookEnvironmentVariable` in
[hooks_runner's build_runner.dart](https://github.com/dart-lang/native/blob/main/pkgs/hooks_runner/lib/src/build_runner/build_runner.dart)).

### Fetching the fork

The Rust crate depends on a fork of grin-wallet at
`ssh://git@git.cypherstack.com:222/sneurlax/grin-wallet.git`, pinned by
commit in `rust/Cargo.toml` and `rust/Cargo.lock`. The hook passes
`CARGO_NET_GIT_FETCH_WITH_CLI=true`, so cargo fetches it with the git command
line and your ssh configuration. The filtered hook environment has no
`SSH_AUTH_SOCK`, so either the fork's commit is already in cargo's git
database (run `cargo fetch --manifest-path rust/Cargo.toml` once in a shell
that has your ssh agent) or the key in `~/.ssh` for that host needs no
passphrase. The fork's `grin` submodule comes from GitHub over https.

### Live node smoke test

`test/live_node_test.dart` talks to a real node over HTTP (connection test, create, tip, scan, balance). It is skipped unless `GRIN_NODE_URL` names a node's foreign API base URL:

```sh
GRIN_NODE_URL=http://127.0.0.1:13413 dart test test/live_node_test.dart
```

`GRIN_NODE_NETWORK=mainnet` switches the wallet to mainnet; the test only reads the tip and scans an empty wallet.

### Private chain (regtest) run

`tool/regtest.sh` (repository root) builds the node and the grin-wallet CLI from the fork, starts a usernet chain with the test miner, a CLI wallet listener and a SOCKS5 test proxy, and runs `test/regtest_test.dart`: populated-chain scan, a Slatepack exchange with grin-wallet, a real HTTP send and the same send through the proxy. The test is skipped unless the `GRIN_REGTEST_*` variables it documents are set.
