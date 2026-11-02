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
- Transactions, Slatepacks, invoices and payment proofs are not in the client
  yet.

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
