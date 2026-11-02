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
pubspec.yaml                  pub workspace root (no Melos); sets build: cargo and
                              test_utils: true for builds made from this checkout
tool/ci.dart                  every gate in one command
tool/regen_bindings.dart      cargo build (cbindgen header) then ffigen
tool/local-fork-env.sh        git URL rewrite for the unpushed fork (see below)
packages/grin/
  rust/                       the grin-ffi crate (Cargo.lock, rust-toolchain.toml)
  hook/build.dart             native_prebuilt_hook spec plus the cargo mode
  hook/prebuilt_hashes.json   pinned native release (none yet)
  flake.nix, nix/, build-config.json
                              reproducible reference builds through native-build-kit
packages/grin_flutter/        Flutter glue and example/
```

## Building and testing

```sh
dart pub get                                   # once, at the root
dart run tool/ci.dart --list                   # the stages
dart run tool/ci.dart --skip nix,flutter-build # quick local run
dart run tool/ci.dart --local-inputs           # everything, Nix with local inputs
```

`dart test` in `packages/grin` and `flutter run` in the example build the
Rust library with the host toolchain (the workspace pubspec selects the
hook's `cargo` mode and turns on `test_utils`, which exports the three
`grin_test_*` functions the Dart tests drive an in-process chain with).
Release artifacts come from the Nix flake and never carry those symbols;
`tool/ci.dart` checks that with `llvm-nm`.

The Rust toolchain is pinned by `packages/grin/rust/rust-toolchain.toml`
(1.94.0) and by `packages/grin/build-config.json` for Nix; the flake fails
to evaluate if they disagree.

## Local development overrides (until the kit and the fork are pushed)

Two things this repository depends on live on a private Gitea instance and
are not pushed yet. Until they are, two uncommitted pieces of local wiring
stand in for them. Both are described in `docs/architecture.md` (D2) and in
`packages/grin/README.md`.

1. `native_prebuilt_hook` (the build hook package) is pinned in
   `packages/grin/pubspec.yaml` as a git dependency on
   `ssh://git@git.cypherstack.com:222/sneurlax/native-build-kit.git` at
   `85f7eb097604e5b99db3f90bccbcada3336836b8`. An uncommitted, gitignored
   `pubspec_overrides.yaml` at the repository root points it at the local
   checkout instead:

   ```yaml
   dependency_overrides:
     native_prebuilt_hook:
       path: /home/user/src/stack_wallet/wt/native-build-kit/dart/native_prebuilt_hook
   ```

   To drop it once the kit ref is reachable: delete `pubspec_overrides.yaml`
   and run `dart pub get` at the root. `pubspec.lock` then records the git
   source.

2. The `grin-wallet` fork is named in `packages/grin/rust/Cargo.toml` by its
   cypherstack URL and commit `64d361f2e49fa6d544cf0236417da48a99444b2f`,
   and `Cargo.lock` already records that source. cargo's git database on
   this machine was filled from the local clone through a git URL rewrite,
   so ordinary builds need nothing. Only a command that has to fetch the
   fork again (`cargo update`, a lock regeneration, a fresh machine) needs
   the rewrite while the fork is unpushed:

   ```sh
   source /home/user/src/grin/tool/local-fork-env.sh
   ```

   It sets `CARGO_NET_GIT_FETCH_WITH_CLI`, a `url.<local>.insteadOf` git
   config for the cypherstack URL, and `GIT_SSH_COMMAND=/bin/false` as a
   guard. Once the fork is pushed, stop sourcing it; nothing else changes.

3. The Nix flake has the same two inputs (`kit` and `grin-wallet`, both ssh
   URLs pinned in `flake.lock`). From this machine, pass local inputs:

   ```sh
   nix build "git+file:///home/user/src/grin?dir=packages/grin#x86_64-unknown-linux-gnu" \
     --override-input kit "git+file:///home/user/src/stack_wallet/wt/native-build-kit?ref=feat/native-build-kit&rev=85f7eb097604e5b99db3f90bccbcada3336836b8" \
     --override-input grin-wallet "git+file:///home/user/src/grin-wallet?ref=optional-tor-rustls&rev=64d361f2e49fa6d544cf0236417da48a99444b2f&submodules=1" \
     --no-write-lock-file
   ```

   `dart run tool/ci.dart --local-inputs` does this for every target. Use
   the `git+file://...?dir=packages/grin` form from a checkout: a plain
   `path:` reference would copy `packages/grin/rust/target` into the store.

## Status

Built and tested on Linux x86_64 (Rust tests on an in-process chain, Dart
tests, the Flutter example, Nix reference builds for Linux and Android).
macOS, iOS and Windows builds, and a sync against a public node, have not
been run here. See the "unverified" list in the project report.
