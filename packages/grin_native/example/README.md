# grin_native_example

A small app that exercises grin_native through the real native library. On start it opens the testnet wallet `example` (or creates it with a freshly generated 24-word mnemonic), then shows the first three words of the phrase, the Slatepack address and the onion URL. The "Balance (local)" button reads the balance from local state only. The node URL points at a port where nothing listens, and the app does not need one.

Run it with `flutter run -d linux`. The integration test is `xvfb-run -a flutter test integration_test -d linux` when there is no display. The first build compiles the Rust library through the build hook of package:grin_native, which takes a few minutes. The wallet is stored under the application support directory, in `grin/example`.


The app depends on `grin_native` only. `lib/src/wallet_paths.dart` (`WalletPaths`, the wallet directory under the application support directory) and `lib/src/lifecycle_observer.dart` (`GrinLifecycleObserver`, which cancels a running scan when the app is paused or detached) are example code to copy into an app. Their tests are in `test/helpers_test.dart`. This app is the Flutter bundling gate of `tool/ci.dart` (`flutter-analyze`, `flutter-test`, `flutter-build`).
