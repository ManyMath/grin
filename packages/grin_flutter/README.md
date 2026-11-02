# grin_flutter

Flutter glue for [`grin`](../grin). It adds three things:

- `GrinFlutter.defaultWalletDirectory(walletId)` returns `<application support>/grin/<walletId>`, creates it and restricts it to the current user where the platform allows. Store only the wallet id and call it again on every open, because absolute container paths can change (on iOS, for example, after a reinstall or restore).
- `GrinLifecycleObserver` wraps `AppLifecycleListener` and calls `GrinWalletClient.cancelScan()` when the app is `paused` or `detached`. Dispose it before closing the client.
- It re-exports `package:grin/grin.dart`, so one import gives the whole API.

The native library is not built here. The build hook of `grin` builds or downloads it for every Flutter target. See [`packages/grin/README.md`](../grin/README.md) for the build modes (`prebuilt`, `source`, `dev`, `cargo`) and the `test_utils` option, which must stay unset in an app.

## Using it from Stack Wallet

Both packages come from the same repository, so `grin` is pinned in `dependency_overrides` at the same url and ref:

```yaml
dependencies:
  grin_flutter:
    git:
      url: https://github.com/cypherstack/grin.git
      path: packages/grin_flutter
      ref: <commit>

dependency_overrides:
  grin:
    git:
      url: https://github.com/cypherstack/grin.git
      path: packages/grin
      ref: <commit>

hooks:
  user_defines:
    grin:
      build: prebuilt   # or source, dev, cargo; unset lets the hook pick
```

## Example app and tests

`example/` creates a testnet wallet and shows its address. In this workspace the root pubspec builds the library with cargo, so the Rust toolchain is needed.

```
cd example
flutter run -d linux
xvfb-run -a flutter test integration_test -d linux   # when there is no display
```

`flutter test` in this directory runs the unit tests, which do not load the native library.
