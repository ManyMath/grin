# grin_native

`grin_native` implements the `grin` wallet interfaces with a Rust library
built from the Grin wallet crates. Its Dart build hook compiles and bundles
the native library as a code asset for Dart and Flutter applications.

`GrinWalletClient` implements `GrinWalletApi`, while `GrinNativeBackend`
implements `GrinWalletBackend`. The package re-exports `package:grin` so an
application can import the native backend and the shared API from one place.

Wallet calls are serialized per wallet and run outside the main isolate.
Native handles are closed deterministically, with a finalizer as a fallback.
The backend supports scanning, Slatepack sends and invoices, HTTP transport,
transaction cancellation and reposting, and payment proofs.

```dart
import 'package:grin_native/grin_native.dart';

final wallet = await GrinWalletClient.open(
  config: const GrinWalletConfig(
    walletDir: '/data/grin/main',
    network: GrinNetwork.mainnet,
    nodeUrl: 'https://grinnode.example.com:3413',
  ),
  password: 'password',
);

final balance = await wallet.getBalance();
await wallet.close();
```
