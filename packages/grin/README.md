# grin

The backend-neutral Grin wallet API, in pure Dart. It holds the types and
interfaces that wallet code is written against, and no implementation of a
wallet.

Apps do not depend on this package directly. They depend on a backend
package, such as `grin_native` (the wallet over the `grin-ffi` Rust library),
which depends on `grin` and re-exports all of `package:grin/grin.dart`. One
import of the backend package gives the whole API. See
[`packages/grin_native/README.md`](../grin_native/README.md) for usage.

Requires Dart 3.13+.

## What it holds

`lib/grin.dart` exports:

- Models: balances, transactions, slates and Slatepacks, addresses, node and
  scan results, send and invoice arguments, and `GrinWalletConfig`. Amounts
  are `BigInt` nanogrin.
- Units: `formatGrin` and `parseGrin` convert to and from decimal grin text
  without floating point.
- A sealed `GrinException` hierarchy for everything a wallet library
  reports.
- `GrinWalletEvent`, a sealed class of wallet events (scan started and
  finished, wallet closed).
- `GrinWalletApi`: one open wallet. It has every instance operation of a
  wallet: addresses, balance, scan, transactions, the Slatepack send,
  receive, invoice and finalize flows, payment proofs and `close`.
- `GrinWalletBackend`: what needs no open wallet, and the way to get one.
  `generateMnemonic`, `validateMnemonic`, `validateAddress`,
  `validateAddressSync`, `testNodeConnection`, `create`, `open`, `delete`
  and `decodeSlatepackWithoutWallet`. `create` and `open` return a
  `GrinWalletApi`.

The interface is named `GrinWalletApi` and not `GrinWallet` because Stack
Wallet's own wallet class for Grin is `GrinWallet`, and it holds one of these.

## No native code

The package has no build hook, no `dart:ffi`, no `dart:isolate` and no
runtime dependencies (`pubspec.yaml` lists only `lints` and `test` for
development). It compiles to JavaScript. `tool/web_smoke.dart` is compiled
with `dart compile js` by the `web-smoke` stage of `tool/ci.dart` and run
with node, so an import of anything native fails that gate.

## Implementing a backend

A backend package depends on `grin`, implements `GrinWalletBackend` and
returns a `GrinWalletApi` from `create` and `open`. It should re-export
`package:grin/grin.dart` so that its users import the backend alone.
`grin_native` does this: `GrinWalletClient implements GrinWalletApi`, and
`GrinNativeBackend` is a `const` `GrinWalletBackend` that delegates to the
static methods of `GrinWalletClient`.

The documentation on the interfaces describes the behaviour every backend
should match: operations of one wallet run in call order, `scanProgress` and
`cancelScan` answer while a scan runs, a bad argument or a call after `close`
throws synchronously, and everything the wallet library reports arrives as a
`GrinException`.

`package:grin/codec.dart` is a separate library, not exported by `grin.dart`.
It exposes the JSON helpers that the models use, so a backend that reads the
`grin-ffi` JSON contract (`docs/ffi-json.md`) decodes it by the same rules
and needs no `implementation_imports`. Apps do not need it.

Members that only make sense for a native backend, such as a native handle
or a finalizer, are not on the interfaces.

## Testing app code with a fake

Write app code against `GrinWalletBackend` and `GrinWalletApi`, and give it a
hand-written fake in unit tests. No native library is built or loaded:

```dart
import 'package:grin/grin.dart';

Future<String> describe(GrinWalletBackend backend, GrinWalletConfig config) async {
  final wallet = await backend.open(config: config, password: 'pw');
  try {
    final address = await wallet.getAddress();
    return address.slatepackAddress;
  } finally {
    await wallet.close();
  }
}
```

`test/wallet_interface_test.dart` has fakes of both interfaces and an
example function of this shape. To test against a real wallet, use
`GrinTestChain` from `package:grin_native/testing.dart`.

## Tests

```sh
dart test
```

The tests (`models_test.dart`, `tx_models_test.dart`,
`wallet_interface_test.dart`) need no native library and no Rust toolchain.
