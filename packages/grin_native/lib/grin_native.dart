/// The native Grin wallet backend, over the grin-ffi Rust library.
///
/// The library is built by this package's build hook. This library
/// re-exports all of `package:grin/grin.dart`, so an app depends on and
/// imports grin_native alone. Start with [GrinWalletClient].
library;

export 'package:grin/grin.dart';

export 'src/wallet/grin_wallet_client.dart';
