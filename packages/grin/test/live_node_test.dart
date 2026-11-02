// Smoke test against a running Grin node over HTTP: the node client path
// that the in-process chain tests do not exercise. Skipped unless
// GRIN_NODE_URL names a reachable node's foreign API base URL, for example
//
//   GRIN_NODE_URL=http://127.0.0.1:13413 dart test test/live_node_test.dart
//
// GRIN_NODE_NETWORK selects the wallet network (testnet by default; mainnet
// is allowed only for reading the tip and scanning an empty wallet).
library;

import 'dart:io';

import 'package:grin/grin.dart';
import 'package:test/test.dart';

import 'support.dart';

void main() {
  final nodeUrl = Platform.environment['GRIN_NODE_URL'];
  final network = Platform.environment['GRIN_NODE_NETWORK'] == 'mainnet'
      ? GrinNetwork.mainnet
      : GrinNetwork.testnet;
  final skip = nodeUrl == null || nodeUrl.isEmpty
      ? 'set GRIN_NODE_URL to run the live node smoke test'
      : null;

  test(
    'tip, create, scan and balance against a live node',
    () async {
      final info = await GrinWalletClient.testNodeConnection(nodeUrl: nodeUrl!);
      expect(info.nodeVersion, isNotEmpty);
      expect(info.height, greaterThanOrEqualTo(0));

      final dir = (await makeTempDir()).path;
      final config = GrinWalletConfig(
        walletDir: dir,
        network: network,
        nodeUrl: nodeUrl,
      );
      final wallet = await GrinWalletClient.create(
        config: config,
        password: 'live',
      );
      try {
        final tip = await wallet.getChainHeight();
        expect(tip.height, greaterThanOrEqualTo(info.height));

        final address = await wallet.getAddress();
        expect(
          address.slatepackAddress,
          startsWith(network == GrinNetwork.mainnet ? 'grin1' : 'tgrin1'),
        );

        final scan = await wallet.scan();
        expect(scan.tipHeight, greaterThanOrEqualTo(0));
        expect(wallet.scanProgress().isScanning, isFalse);

        final balance = await wallet.getBalance(minimumConfirmations: 1);
        expect(balance.refreshedFromNode, isTrue);
        expect(balance.total, BigInt.zero);
      } finally {
        await wallet.close();
      }

      final reopened = await GrinWalletClient.open(
        config: config,
        password: 'live',
      );
      await reopened.close();
      await GrinWalletClient.delete(walletDir: dir);
    },
    skip: skip,
    timeout: const Timeout(Duration(minutes: 5)),
  );
}
