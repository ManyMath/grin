# grin

Dart bindings and a typed wallet client for [Grin](https://grin.mw), over the
`grin-ffi` Rust library in `rust/grin-ffi`. The package is pure Dart: its build
hook obtains the native library and bundles it as a code asset, so the same
package serves Dart programs, Flutter apps and tests.

Requires Dart 3.13+ (or a Flutter SDK containing it).

## Design

1. `rust/grin-ffi` wraps the `grin-wallet` libraries behind a C ABI. Every
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
  lock, so they answer while a scan runs. A scan that has started cannot be
  interrupted; see the notes in `docs/ffi-json.md`.
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

The workspace `pubspec.yaml` sets `build: cargo` and `test_utils: true`, so
the first run builds the Rust library with the host toolchain (several
minutes). `package:grin/testing.dart` exposes `GrinTestChain`, which creates
wallets on an in-process chain and mines blocks to them. It only works with a
library built with `test_utils`.

Regenerate the C header and the bindings with
`dart run tool/regen_bindings.dart` from the repository root (needs cargo and
libclang). With `--check` it fails if the committed files are out of date.

## Native library

The hook obtains the Rust library in one of four modes, chosen with the
`build` user-define in the consuming app's `pubspec.yaml`:

```yaml
hooks:
  user_defines:
    grin:
      build: prebuilt   # or source, dev or cargo; unset lets the hook pick
      # any_xcode: true # source builds of Apple targets skip the Xcode pin
      # developer_dir: /Applications/Xcode-27.0.app/Contents/Developer
      # test_utils: true # cargo mode only, see below
```

- `prebuilt` downloads the library published for the pinned native release
  and verifies it against the hash recorded in `hook/prebuilt_hashes.json`.
  A mismatch is an error, never a fallback. Downloads are cached by hash, so
  only the first build after a release needs network access.
- `source` builds the library with the Nix flake, the same build CI uses, and
  verifies it against the same hash. Apple targets require the pinned Xcode
  unless `any_xcode: true`, in which case the hash check still applies.
- `dev` builds with the Nix flake but skips the Xcode pin and the hash check,
  for working on the Rust code.
- `cargo` builds with the host's rustup and cargo
  (`cargo build --locked --release --lib` in `rust/grin-ffi`, with the Rust
  version pinned in `rust/rust-toolchain.toml`) through
  `native_toolchain_rust`'s `RustBuilder`. Android links for the app's API
  level with the NDK Flutter provides. The result is not checked against any
  hash: host builds are not reproducible.

Without a `build` user-define the hook uses `prebuilt` when the recorded
release has the target. No release is recorded yet (`hook/prebuilt_hashes.json`
is empty), so today it builds in `dev` mode where Nix can build the target and
in `cargo` mode where it cannot (Windows hosts, Intel Macs, targets the flake
does not build). It logs the reason. An explicit mode never falls back. The
full table is in the kit's `dart/native_prebuilt_hook/README.md`.

Source and dev builds rerun when `rust`, `flake.nix`, `flake.lock`, `nix` or
`build-config.json` change. Cargo builds rerun when anything under `rust`
changes.

`test_utils` (boolean, default false) adds `--features test-utils` to the
cargo build, which exports the `grin_test_*` functions that `GrinTestChain`
needs. Only the `cargo` mode reads it: prebuilt, source and dev libraries are
release builds and never export them. Never set it in an app. This
repository's workspace `pubspec.yaml` sets it so that `dart test` works, which
also applies to the example app; those builds are not release artifacts.

The settings are user-defines, not environment variables, because the hooks
runner that `dart test`, `dart run` and `flutter build` use starts hooks with a
filtered environment: only PATH, HOME, the temp and proxy variables, the
Android NDK variables, a few Windows ones, and names starting with `CARGO_`,
`CCACHE_`, `DOTNET_`, `NIX_`, `NUGET_`, `RUSTUP_` or `CONAN_` get through (see
`includeHookEnvironmentVariable` in
[hooks_runner's build_runner.dart](https://github.com/dart-lang/native/blob/main/pkgs/hooks_runner/lib/src/build_runner/build_runner.dart)).
The filtered environment also has no `SSH_AUTH_SOCK`: if the flake fetches an
input over `git+ssh://`, Nix inside the hook cannot use an SSH agent, so the
key must be usable from `~/.ssh` without one.

### Fetching the fork in cargo mode

The Rust crate depends on a fork of grin-wallet that cargo fetches over
`ssh://`. The hook passes `CARGO_NET_GIT_FETCH_WITH_CLI=true`, so cargo uses
the git command line. It cannot rely on `rust/.cargo/config.toml` for that,
because cargo reads config files from its working directory and the hook does
not run in the crate. The hooks runner also filters the environment, so no ssh
agent is reachable from the hook. In `cargo` mode the git database must
therefore already hold the fork's rev (run `cargo fetch --manifest-path
rust/Cargo.toml` once in a shell that has your ssh agent), or the fork must be
fetchable with a default ssh key in `~/.ssh` that needs no passphrase. The
`prebuilt`, `source` and `dev` modes avoid the problem.

## Local development overrides

Two things are wired to local checkouts until the repositories they point at
are pushed. Neither is committed except `tool/local-fork-env.sh`, which is
harmless once the fork is public.

1. `pubspec_overrides.yaml` at the repository root points
   `native_prebuilt_hook` at the local kit checkout
   (`/home/user/src/stack_wallet/wt/native-build-kit/dart/native_prebuilt_hook`).
   The real dependency in `packages/grin/pubspec.yaml` is a git dependency
   over ssh on the kit commit named in its `ref`, which is not reachable from
   this machine yet.
2. `tool/local-fork-env.sh` rewrites the git URL of the `grin-wallet` fork
   (`ssh://git@git.cypherstack.com:222/sneurlax/grin-wallet.git`) to the local
   clone at `/home/user/src/grin-wallet`, so that cargo can resolve and update
   it while it is unpushed. Source it only before a `cargo update` or a lock
   file regeneration:

   ```sh
   source /home/user/src/grin/tool/local-fork-env.sh
   ```

   Ordinary `cargo build`, `dart test` and `flutter build` need none of it
   once `Cargo.lock` exists and the pinned revision is in `~/.cargo/git/db`.

Dropping both after the kit and the fork are pushed:

1. Delete `/home/user/src/grin/pubspec_overrides.yaml` and run `dart pub get`
   at the repository root. The kit is then fetched over ssh at the pinned
   `ref`, which needs a key with read access.
2. Stop sourcing `tool/local-fork-env.sh` (unset `CARGO_NET_GIT_FETCH_WITH_CLI`
   and the `GIT_CONFIG_*` variables in shells that have it). Cargo then fetches
   the fork from the cypherstack URL that `Cargo.lock` already records.

`cargo update -p grin_wallet_impls` is not needed: the fork revision is pinned
in `Cargo.toml` and `Cargo.lock` already names the final source.

### Live node smoke test

`test/live_node_test.dart` talks to a real node over HTTP (connection test, create, tip, scan, balance). It is skipped unless `GRIN_NODE_URL` names a node's foreign API base URL:

```sh
GRIN_NODE_URL=http://127.0.0.1:13413 dart test test/live_node_test.dart
```

`GRIN_NODE_NETWORK=mainnet` switches the wallet to mainnet; the test only reads the tip and scans an empty wallet.
