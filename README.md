# grin

`grin` is a backend-neutral Grin wallet API for Dart. It defines wallet
configuration, balances, transactions, Slatepacks, payment proofs, typed
exceptions, events, amount conversion, and the `GrinWalletApi` and
`GrinWalletBackend` interfaces.

The package is pure Dart and contains no native code. Applications can write
wallet logic against these interfaces and provide a backend such as
`grin_native` in production or a lightweight fake in tests.

Amounts are represented as `BigInt` nanogrin. Slatepack send, receive,
invoice, finalization, cancellation, scanning, and payment-proof operations
are exposed through the wallet interface.

The package source is in [`packages/grin`](packages/grin).
