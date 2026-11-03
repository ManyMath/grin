/// App code for the consumer gate: written against the interfaces that
/// grin_native re-exports, with no import of package:grin.
library;

import 'package:grin_native/grin_native.dart';

/// Pays [amount] from [sender] to [recipient] with an encrypted Slatepack
/// round trip (S1, S2, S3) and broadcasts the result.
Future<GrinFinalizeResult> payBySlatepack({
  required GrinWalletApi sender,
  required GrinWalletApi recipient,
  required BigInt amount,
}) async {
  final to = await recipient.getAddress();
  final s1 = await sender.initSend(
    GrinSendArgs(
      amount: amount,
      minimumConfirmations: 1,
      recipientAddress: to.slatepackAddress,
    ),
  );
  final s2 = await recipient.receiveSlatepack(s1.armoredSlatepack);
  return sender.finalizeSlatepack(s2.armoredSlatepack);
}

/// Whether [address] is a valid address for [network], checked without a
/// wallet.
bool isValidAddress(
  GrinWalletBackend backend,
  String address,
  GrinNetwork network,
) {
  try {
    backend.validateAddressSync(address, network);
    return true;
  } on GrinInvalidAddressException {
    return false;
  }
}
