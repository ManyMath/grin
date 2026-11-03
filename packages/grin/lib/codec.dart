/// JSON decoding and native argument checks for wallet backends.
///
/// `package:grin/grin.dart` does not export this library. It is for packages
/// that implement the wallet interfaces over the grin-ffi JSON contract, such
/// as grin_native, so they read native output with the same rules as the
/// models do. Apps do not need it.
library;

export 'src/internal/json_helpers.dart';
