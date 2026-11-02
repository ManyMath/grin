# grin_flutter

Flutter glue for [`grin`](../grin). It adds three things:

- `GrinFlutter.defaultWalletDirectory(walletId)` returns `<application support>/grin/<walletId>`, creates it and restricts it to the current user where the platform allows. Store only the wallet id and call it again on every open, because absolute container paths can change (on iOS, for example, after a reinstall or restore).
- `GrinLifecycleObserver` wraps `AppLifecycleListener` and calls `GrinWalletClient.cancelScan()` when the app is `paused` or `detached`. Dispose it before closing the client.
- It re-exports `package:grin/grin.dart`, so one import gives the whole API.

The native library is not built here. The build hook of `grin` builds it for every Flutter target with the host Rust toolchain; see [`packages/grin/README.md`](../grin/README.md). Its one option, `test_utils`, must stay unset in an app.

## Using it from Stack Wallet

Both packages come from the same repository, so `grin` is pinned in `dependency_overrides` at the same url and ref. Nothing goes under `hooks.user_defines` for `grin`.

```yaml
dependencies:
  grin_flutter:
    git:
      url: ssh://git@git.cypherstack.com:222/sneurlax/grin.git
      path: packages/grin_flutter
      ref: <commit>

dependency_overrides:
  grin:
    git:
      url: ssh://git@git.cypherstack.com:222/sneurlax/grin.git
      path: packages/grin
      ref: <commit>
```

## Example app and tests

`example/` creates a testnet wallet and shows its address. The root pubspec of this workspace turns on `test_utils`; the Rust toolchain is needed either way.

```
cd example
flutter run -d linux
xvfb-run -a flutter test integration_test -d linux   # when there is no display
```

`flutter test` in this directory runs the unit tests, which do not load the native library.
