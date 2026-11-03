// Compiled to JavaScript by the web-smoke stage of tool/ci.dart, which runs
// the output with node when it is installed. It proves that package:grin
// has no native dependency: dart compile js fails on any dart:ffi or
// dart:isolate import.
//
//   dart compile js -o /tmp/web_smoke.js tool/web_smoke.dart && node /tmp/web_smoke.js
// ignore_for_file: avoid_print
import 'package:grin/grin.dart';

void main() {
  final amount = parseGrin('1.5');
  final balance = GrinBalance.fromJson({
    'last_confirmed_height': 7,
    'minimum_confirmations': 10,
    'total': 2500000000,
    'amount_awaiting_finalization': 0,
    'amount_awaiting_confirmation': 0,
    'amount_immature': 1000000000,
    'amount_currently_spendable': 1500000000,
    'amount_locked': 0,
    'amount_reverted': 0,
    'refreshed_from_node': false,
  });
  final error = GrinException.fromJson(18, '{"code":18,"message":"busy"}');
  final checks = [
    formatGrin(amount) == '1.5',
    balance.amountCurrentlySpendable == amount,
    error is GrinWalletBusyException,
    GrinNetwork.testnet.value == 1,
  ];
  if (checks.contains(false)) throw StateError('web_smoke failed: $checks');
  print('web_smoke ok');
}
