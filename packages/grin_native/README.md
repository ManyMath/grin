# grin_native

The native [Grin](https://grin.mw) wallet backend for Dart and Flutter, over
the `grin-ffi` Rust library in `rust/`. The package is pure Dart: its build
hook builds the native library and bundles it as a code asset, so the same
package serves Dart programs, Flutter apps and tests. There is no Flutter
plugin package.

It depends on [`grin`](../grin), the backend-neutral contract, and re-exports
all of `package:grin/grin.dart`. An app lists `grin_native` alone and
imports only `package:grin_native/grin_native.dart`.

Requires Dart 3.13+ (or a Flutter SDK containing it).

## Design

1. `rust/` wraps the `grin-wallet` libraries behind a C ABI. Every call is
   synchronous and returns a `GrinFfiResult` with JSON text. The JSON
   contract is in `docs/ffi-json.md` at the repository root.
2. `lib/src/ffi` holds the generated `@Native` bindings and the small layer
   that frees results and wipes secrets (passwords, mnemonics) from native
   memory. It maps error JSON to the sealed `GrinException` hierarchy of
   `grin`, using `package:grin/codec.dart`.
3. `GrinWalletClient` is the wallet. It implements `GrinWalletApi` and
   `Finalizable`. It runs each call in a short-lived isolate, queues the
   calls of one wallet so they run in order, and closes the native handle
   deterministically (and through a `NativeFinalizer` if you forget).
4. `GrinNativeBackend` is a `const` `GrinWalletBackend` whose methods call
   the static methods of `GrinWalletClient` of the same name. Take it where
   code should depend on the interface and not on this package.
5. Amounts are `BigInt` nanogrin. `formatGrin` and `parseGrin` convert to and
   from decimal grin text without floating point.
6. The native-only members, `nativeHandle`, `runQueued`, `initLogs` and the
   finalizers, are on `GrinWalletClient` and not on the interfaces.

## Usage

```dart
import 'package:grin_native/grin_native.dart';

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
    print('${formatGrin(balance.amountCurrentlySpendable)} grin spendable');
  } finally {
    await wallet.close();
  }
}
```

Things to know:

- Every failure the native library reports is a `GrinException`.
  `GrinWalletBusyException` means the wallet directory is already held by an
  open wallet in this process (or by a closed one that has a call still
  running). It is reported and never retried. After `await client.close()`
  the directory is free again.
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

## Writing app code against the interfaces

Code that takes a `GrinWalletBackend` or a `GrinWalletApi` can run against a
fake in unit tests, without the native library:

```dart
Future<String> receiveAddress(GrinWalletApi wallet) async =>
    (await wallet.getAddress()).slatepackAddress;

// In the app:   receiveAddress(await GrinWalletClient.open(...))
//               or with GrinNativeBackend().open(...)
// In the tests: receiveAddress(FakeWallet())
```

`packages/grin/test/wallet_interface_test.dart` has fakes of both
interfaces. `test_consumer/` is a pure Dart package that depends on
`grin_native` alone, never imports `package:grin/`, and pays by Slatepack
between two wallets on the in-process test chain through `GrinWalletApi`.

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

## Flutter

Flutter apps depend on `grin_native` directly. The hook builds and bundles the
library for every Flutter target with the host Rust toolchain, so the app
needs no plugin package and no extra setup. `example/` is the Flutter bundling
gate: `tool/ci.dart` analyzes, tests and builds it for Linux and Android.

Two helpers are example code to copy, because an app's storage layout and
lifecycle policy are its own:

- `example/lib/src/wallet_paths.dart` has `WalletPaths.defaultWalletDirectory(walletId)`,
  which returns `<application support>/grin/<walletId>`, creates it and
  restricts it to the current user where the platform allows. Store only the
  wallet id and call it again on every open, because absolute container paths
  can change (on iOS, for example, after a reinstall or restore).
  `GrinWalletConfig.walletDir` takes any directory, so an app with its own
  storage root can skip it.
- `example/lib/src/lifecycle_observer.dart` has `GrinLifecycleObserver`, which
  wraps `AppLifecycleListener` and calls `cancelScan()` on a `GrinWalletApi`
  when the app is `paused` or `detached`. Dispose it before closing the
  wallet.

Their tests are `example/test/helpers_test.dart`. The example app creates a
testnet wallet and shows its address:

```sh
cd example
flutter run -d linux
xvfb-run -a flutter test integration_test -d linux   # when there is no display
```

`flutter test` in `example/` runs the unit tests, which do not load the
native library.

## Using it from Stack Wallet

Stack Wallet lists `grin_native` alone and imports only
`package:grin_native/...`. `grin` is transitive. Both packages come from the
same repository, and `grin` is not published, so it is pinned in
`dependency_overrides` at the same url and ref: `grin_native` declares
`grin: ^0.1.0`, which a git consumer would otherwise look up on pub.dev. The
pin goes away once `grin` is published. Use the same url for both entries
and move both refs together; pub counts a second url as a second source.
The repository stays private on git.cypherstack.com for now, so read
"Private access" below before the first build.
The hook reads one user-define, `test_utils`, which adds the `grin_test_*`
functions for tests and must stay false in an app; leaving the block out is
the same.

```yaml
dependencies:
  grin_native:
    git:
      url: ssh://git@git.cypherstack.com:222/sneurlax/grin_native.git
      path: packages/grin_native
      ref: <commit>

dependency_overrides:
  grin:
    git:
      url: ssh://git@git.cypherstack.com:222/sneurlax/grin_native.git
      path: packages/grin
      ref: <commit>

hooks:
  user_defines:
    grin_native:
      test_utils: false
```

Stack Wallet's own wallet class for Grin is `GrinWallet`, which is why the
interface is called `GrinWalletApi`. It can hold a `GrinWalletApi`, and its
tests can pass a fake.

## Tests

```sh
dart test
```

The workspace `pubspec.yaml` sets `test_utils: true`, so the first run builds
the Rust library with the host toolchain and the test functions (several
minutes). `package:grin_native/testing.dart` exposes `GrinTestChain`, which
creates wallets on an in-process chain and mines blocks to them. It only works
with a library built with `test_utils`.

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
asset `package:grin_native/src/ffi/bindings.g.dart`. A hook can only emit a
code asset for its own package, which is why the hook is here and not in
`grin`, and why `grin` stays free of native code.

There is nothing to configure in an app. The one user-define the hook reads
is for this repository's own tests:

```yaml
hooks:
  user_defines:
    grin_native:
      test_utils: true   # this workspace only; never in an app
```

`test_utils` (boolean, default false) adds `--features test-utils`, which
exports the `grin_test_*` functions that `GrinTestChain` needs. This
repository's workspace `pubspec.yaml` sets it so that `dart test` works, which
also applies to the example app; those builds are not release artifacts.
`tool/ci.dart` checks with `llvm-nm` that release builds made without it
export the 36 standard functions and none of the test ones.

There are no prebuilt or reproducible builds yet. Release artifacts are a
later step, planned on [dnix](https://github.com/cypherstack/dnix); see
`docs/architecture.md` D11.

The setting is a user-define, not an environment variable, because the hooks
runner that `dart test`, `dart run` and `flutter build` use starts hooks with a
filtered environment. In hooks_runner 1.5.0, the version Flutter 3.47.2 pins,
only PATH, HOME, LIBCLANG_PATH, GOPATH, the temp and proxy variables, the
Android NDK variables, a few Windows ones, and names starting with `CCACHE_`,
`DOTNET_`, `NIX_`, `NUGET_` or `CONAN_` get through (see
`includeHookEnvironmentVariable` in
[hooks_runner's build_runner.dart](https://github.com/dart-lang/native/blob/main/pkgs/hooks_runner/lib/src/build_runner/build_runner.dart)).
`CARGO_*` and `RUSTUP_*` variables do not reach the hook, so `CARGO_HOME`
and `RUSTUP_HOME` are ignored there: cargo and rustup inside the hook use
`$HOME/.cargo` and `$HOME/.rustup`, and rustup installs the pinned toolchain
into `$HOME/.rustup` if it is not there yet. `SSH_AUTH_SOCK` is dropped as
well.

### Private access

This repository and the grin-wallet fork it builds against are private on
git.cypherstack.com for now, so a build needs read access to both, and the
two are fetched in different ways.

- pub fetches this repository (the `grin_native` and `grin` git entries)
  over ssh. `dart pub get` and `flutter pub get` run with your full
  environment, so your ssh agent works.
- cargo fetches the fork
  (`ssh://git@git.cypherstack.com:222/sneurlax/grin-wallet.git`, pinned by
  commit in `rust/Cargo.toml` and `rust/Cargo.lock`) inside the build hook.
  The hook passes `CARGO_NET_GIT_FETCH_WITH_CLI=true`, so cargo uses the git
  command line and your ssh configuration, but the hook's environment has
  no `SSH_AUTH_SOCK` (see above), so the agent is out of reach.

So before the first build, do one of these:

1. Fetch the fork once from a shell that has your agent, into the cargo
   home the hook will use:

   ```sh
   cargo fetch --locked --manifest-path <grin_native>/rust/Cargo.toml
   ```

   `<grin_native>` is this package's directory: in this repository
   `packages/grin_native`, in an app the checkout pub made under
   `~/.pub-cache/git/` (the `rootUri` of `grin_native` in the app's
   `.dart_tool/package_config.json`). Run it with `CARGO_HOME` unset or set
   to `$HOME/.cargo`, because the hook ignores any other value. After that
   cargo finds the pinned commit in its git database and does not go to
   the network for it. Repeat it when the pinned fork commit changes.
2. Or keep a key without a passphrase for git.cypherstack.com in `~/.ssh`
   (a read-only deploy key is enough), which git can use without an agent.

The fork's `grin` submodule comes from GitHub over https and needs no key.

### Live node smoke test

`test/live_node_test.dart` talks to a real node over HTTP (connection test, create, tip, scan, balance). It is skipped unless `GRIN_NODE_URL` names a node's foreign API base URL:

```sh
GRIN_NODE_URL=http://127.0.0.1:13413 dart test test/live_node_test.dart
```

`GRIN_NODE_NETWORK=mainnet` switches the wallet to mainnet; the test only reads the tip and scans an empty wallet.

### Private chain (regtest) run

`tool/regtest.sh` (repository root) builds the node and the grin-wallet CLI from the fork, starts a usernet chain with the test miner, a CLI wallet listener and a SOCKS5 test proxy, and runs `test/regtest_test.dart`: populated-chain scan, a Slatepack exchange with grin-wallet, a real HTTP send and the same send through the proxy. The test is skipped unless the `GRIN_REGTEST_*` variables it documents are set.
