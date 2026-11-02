# grin

A Grin wallet library for Dart and Flutter, built as a proposed replacement
for Stack Wallet's `flutter_libepiccash` and `flutter_libmwc` pattern and
as the model the Epic Cash and MWC plugins can migrate onto later. Nothing
in this repository is published; every pubspec has `publish_to: none`.

Three layers, one repository:

| Layer | Where | What |
|---|---|---|
| `grin-ffi` (Rust) | `packages/grin/rust` | a C ABI over a thin fork of `grin-wallet` 5.5.1: 35 functions, every one returns `GrinFfiResult {code, data, error}` and runs inside a panic guard; wallets are u64 handles |
| `grin` (pure Dart) | `packages/grin` | ffigen bindings, typed models (`BigInt` nanogrin), a sealed `GrinException` hierarchy and `GrinWalletClient`; owns the single native-assets build hook |
| `grin_flutter` (Flutter) | `packages/grin_flutter` | a default wallet directory, an app lifecycle observer and re-exports; the example app lives in `example/` |

The design, every decision and the Stack Wallet integration plan are in
[docs/architecture.md](docs/architecture.md). The JSON contract between the
Rust crate and the Dart package is [docs/ffi-json.md](docs/ffi-json.md).
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
packages/grin/
  rust/                       the grin-ffi crate (Cargo.lock, rust-toolchain.toml)
  hook/build.dart             builds rust/ with native_toolchain_rust
packages/grin_flutter/        Flutter glue and example/
```

## Building and testing

```sh
dart pub get                                        # once, at the root
dart run tool/ci.dart --list                        # the stages
dart run tool/ci.dart --skip android,flutter-build  # quick local run
dart run tool/ci.dart                               # everything
```

`dart test` in `packages/grin` and `flutter run` in the example build the
Rust library with the host toolchain through the hook. The workspace pubspec
turns on `test_utils`, which exports the three `grin_test_*` functions the
Dart tests drive an in-process chain with. `tool/ci.dart` checks with
`llvm-nm` that release builds made without it export exactly the 35 standard
functions and none of the test ones.

The Rust toolchain is pinned by `packages/grin/rust/rust-toolchain.toml`
(1.94.0). The Android stage needs NDK 28.2.13676358 and `cargo-ndk`.

## The grin-wallet fork

`packages/grin/rust/Cargo.toml` depends on a fork of grin-wallet at
`ssh://git@git.cypherstack.com:222/sneurlax/grin-wallet.git`, branch
`optional-tor-rustls`, pinned to commit
`64d361f2e49fa6d544cf0236417da48a99444b2f` (also on
`ssh://git@git.manymath.com:2222/sneurlax/grin-wallet.git`). Cargo fetches
it with the git command line, so a machine that has not fetched it yet needs
ssh read access to that host. See "Fetching the fork" in
`packages/grin/README.md` for what that means inside the build hook.

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
