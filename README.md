# grin

A Grin wallet library for Dart and Flutter, built as a proposed replacement
for Stack Wallet's `flutter_libepiccash` and `flutter_libmwc` pattern and
as the model the Epic Cash and MWC plugins can migrate onto later. Nothing
in this repository is published; every pubspec has `publish_to: none`.

A Rust crate and two pure Dart packages, one repository:

| Layer | Where | What |
|---|---|---|
| `grin-ffi` (Rust) | `packages/grin_native/rust` | a C ABI over a thin fork of `grin-wallet` 5.5.1: 35 functions, the 32 fallible ones return `GrinFfiResult {code, data, error}` and run inside a panic guard (the three free and finalizer functions return void and catch panics); wallets are u64 handles |
| `grin` (pure Dart) | `packages/grin` | the backend-neutral contract: typed models (`BigInt` nanogrin), a sealed `GrinException` hierarchy, units, events and the `GrinWalletApi` and `GrinWalletBackend` interfaces. No native code, no runtime dependencies; it compiles to JavaScript |
| `grin_native` (pure Dart) | `packages/grin_native` | the native backend: ffigen bindings, `GrinWalletClient` (implements `GrinWalletApi`), `GrinNativeBackend` and the single native-assets build hook; re-exports all of `grin`, so an app depends on `grin_native` alone |

There is no Flutter package. Flutter apps depend on `grin_native` directly.
The example app (`packages/grin_native/example`) is the Flutter bundling
gate, and `packages/grin_native/test_consumer` is a pure Dart package that
depends on `grin_native` alone, as an app would.

The design, every decision and the Stack Wallet integration plan are in
[docs/architecture.md](docs/architecture.md). The JSON contract between the
Rust crate and `grin_native` is [docs/ffi-json.md](docs/ffi-json.md).
Each package has its own README.

## Layout

```text
pubspec.yaml                  pub workspace root (no Melos); sets test_utils: true
                              for builds made from this checkout
tool/ci.dart                  every gate in one command
tool/regen_bindings.dart      cargo build (cbindgen header) then ffigen
tool/regtest.sh               private chain run (node, grin-wallet peer, SOCKS5 proxy)
tool/verify-apple.sh          host release builds and the symbol gate on a Mac
tool/verify-windows.ps1       the same on Windows
packages/grin/                the contract: models, exceptions, units, events,
                              GrinWalletApi, GrinWalletBackend; pure Dart
  lib/codec.dart              JSON helpers for backend packages
  tool/web_smoke.dart         compiled to JavaScript by the web-smoke stage
packages/grin_native/         the native backend; re-exports package:grin
  rust/                       the grin-ffi crate (Cargo.lock, rust-toolchain.toml)
  hook/build.dart             builds rust/ with native_toolchain_rust
  lib/testing.dart            GrinTestChain, for tests of code that uses it
  example/                    Flutter example app (grin_native_example)
  test_consumer/              pure Dart package that depends on grin_native alone
```

## Building and testing

```sh
dart pub get                                        # once, at the root
dart run tool/ci.dart --list                        # the stages
dart run tool/ci.dart --skip android,flutter-build  # quick local run
dart run tool/ci.dart                               # everything
```

`dart test` in `packages/grin` needs no native library. `dart test` in
`packages/grin_native` and `flutter run` in the example build the Rust
library with the host toolchain through the hook. The workspace pubspec
turns on `test_utils`, which exports the three `grin_test_*` functions the
Dart tests drive an in-process chain with. `tool/ci.dart` checks with
`llvm-nm` that release builds made without it export exactly the 35 standard
functions and none of the test ones.

The Rust toolchain is pinned by `packages/grin_native/rust/rust-toolchain.toml`
(1.94.0). The Android stage needs NDK 28.2.13676358 and `cargo-ndk`.

## The grin-wallet fork

`packages/grin_native/rust/Cargo.toml` depends on a fork of grin-wallet at
`ssh://git@git.cypherstack.com:222/sneurlax/grin-wallet.git`, branch
`optional-tor-rustls`, pinned to commit
`b5d756bdae51c71b1b2ec06dbf0695b37d50a850` (also on
`ssh://git@git.manymath.com:2222/sneurlax/grin-wallet.git`). Cargo fetches
it with the git command line, so a machine that has not fetched it yet needs
ssh read access to that host. See "Fetching the fork" in
`packages/grin_native/README.md` for what that means inside the build hook.

## Status

Built and tested on Linux x86_64 (Rust tests on an in-process chain, Dart
tests, the Flutter example for Linux and Android), on macOS (release builds
for all five Apple targets, the macOS and iOS example builds) and on Windows
(the DLL and the example). A live testnet node and a private regtest chain
(scan, Slatepack exchange, HTTP and SOCKS5 sends) were exercised; mainnet, a
populated public chain and Tor itself were not. There are no prebuilt or
reproducible release artifacts yet; that is a later step on dnix
(`docs/architecture.md` D11). See section 13 of `docs/architecture.md` for
the record.
