# grin_native_example

A small app that exercises grin_native through the real native library. On start it opens the testnet wallet `example` (or creates it with a freshly generated 24-word mnemonic), then shows the first three words of the phrase, the Slatepack address and the onion URL. The "Balance (local)" button reads the balance from local state only. The node URL points at a port where nothing listens, and the app does not need one.

Run it with `flutter run -d linux`. The integration test is `xvfb-run -a flutter test integration_test -d linux` when there is no display. The first build compiles the Rust library through the build hook of package:grin, which takes a few minutes. The wallet is stored under the application support directory, in `grin/example`.
