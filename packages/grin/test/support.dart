import 'dart:io';

import 'package:grin/grin.dart';
import 'package:test/test.dart';

/// A fresh temporary directory that is deleted when the current test ends.
Future<Directory> makeTempDir([String prefix = 'grin_test_']) async {
  final dir = await Directory.systemTemp.createTemp(prefix);
  addTearDown(() async {
    try {
      await dir.delete(recursive: true);
    } on FileSystemException {
      // Best effort; the system temp directory is cleaned up anyway.
    }
  });
  return dir;
}

/// A configuration for a wallet in [dir]. The node URL points at a port
/// nothing listens on, so a call that needs a node fails fast.
GrinWalletConfig testConfig(
  Directory dir, {
  GrinNetwork network = GrinNetwork.automatedTesting,
}) {
  return GrinWalletConfig(
    walletDir: dir.path,
    network: network,
    nodeUrl: 'http://127.0.0.1:1',
    requestTimeout: const Duration(seconds: 5),
  );
}

/// Creates a wallet and closes it when the current test ends.
Future<GrinWalletClient> createTestWallet(
  Directory dir, {
  GrinNetwork network = GrinNetwork.automatedTesting,
  String? mnemonic,
  String password = 'pw',
}) async {
  final wallet = await GrinWalletClient.create(
    config: testConfig(dir, network: network),
    mnemonic: mnemonic,
    password: password,
  );
  addTearDown(wallet.close);
  return wallet;
}

/// A valid 12-word phrase (all zero entropy).
const String knownMnemonic =
    'abandon abandon abandon abandon abandon abandon '
    'abandon abandon abandon abandon abandon about';
