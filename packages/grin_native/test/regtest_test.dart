// End-to-end test against a private Grin chain (the node's usernet mode with
// its test miner) and a grin-wallet CLI wallet that owns the mined coins and
// runs a foreign API listener. It exercises what the in-process chain tests
// cannot: the HTTP node client against a populated chain, a Slatepack
// exchange with another implementation, a real HTTP send to a listener, and
// the same send through a SOCKS5 proxy. Skipped unless GRIN_REGTEST_NODE_URL
// is set. The full environment:
//
//   GRIN_REGTEST_NODE_URL      node foreign API, e.g. http://127.0.0.1:23413
//   GRIN_REGTEST_PEER_URL      the CLI wallet's listener, e.g. http://127.0.0.1:23415
//   GRIN_REGTEST_CLI           path to the grin-wallet binary
//   GRIN_REGTEST_CLI_DIR       the CLI wallet's top-level directory (-t)
//   GRIN_REGTEST_CLI_PASSWORD  its password
//   GRIN_REGTEST_SOCKS         optional, host:port of a SOCKS5 proxy
//
// tool/regtest.sh sets all of this up from the fork's own node and wallet.
library;

import 'dart:io';

import 'package:grin_native/grin_native.dart';
import 'package:test/test.dart';

import 'support.dart';

final _env = Platform.environment;
final _nodeUrl = _env['GRIN_REGTEST_NODE_URL'];
final _peerUrl = _env['GRIN_REGTEST_PEER_URL'];
final _cli = _env['GRIN_REGTEST_CLI'];
final _cliDir = _env['GRIN_REGTEST_CLI_DIR'];
final _cliPassword = _env['GRIN_REGTEST_CLI_PASSWORD'] ?? '';
final _socks = _env['GRIN_REGTEST_SOCKS'];

final BigInt _grin = BigInt.from(1000000000);

Future<String> _cliRun(List<String> args) async {
  final result = await Process.run(_cli!, [
    '--usernet',
    '-t',
    _cliDir!,
    '-p',
    _cliPassword,
    ...args,
  ]);
  if (result.exitCode != 0) {
    throw StateError(
      'grin-wallet ${args.join(' ')} failed (${result.exitCode}):\n'
      '${result.stdout}\n${result.stderr}',
    );
  }
  return '${result.stdout}';
}

String _armored(String text) {
  final match = RegExp(r'BEGINSLATEPACK\.[\s\S]*?ENDSLATEPACK\.')
      .firstMatch(text);
  if (match == null) throw StateError('no slatepack in:\n$text');
  return match.group(0)!;
}

Future<void> _waitUntil(
  Future<bool> Function() condition, {
  Duration timeout = const Duration(minutes: 3),
}) async {
  final deadline = DateTime.now().add(timeout);
  while (!await condition()) {
    if (DateTime.now().isAfter(deadline)) throw StateError('timed out');
    await Future<void>.delayed(const Duration(seconds: 2));
  }
}

void main() {
  final nodeUrl = _nodeUrl;
  final peerUrl = _peerUrl;
  final socks = _socks;
  final skip = nodeUrl == null || nodeUrl.isEmpty || peerUrl == null
      ? 'set GRIN_REGTEST_NODE_URL and GRIN_REGTEST_PEER_URL (see tool/regtest.sh)'
      : null;

  test(
    'scan, receive, HTTP send and proxied send on a private chain',
    () async {
      final info = await GrinWalletClient.testNodeConnection(nodeUrl: nodeUrl!);
      expect(info.height, greaterThan(10), reason: 'the test miner has run');

      final dir = (await makeTempDir()).path;
      final wallet = await GrinWalletClient.create(
        config: GrinWalletConfig(
          walletDir: dir,
          network: GrinNetwork.userTesting,
          nodeUrl: nodeUrl,
        ),
        password: 'regtest',
      );
      addTearDown(wallet.close);

      // A populated chain: the scan covers every mined block.
      final scan = await wallet.scan();
      expect(scan.tipHeight, greaterThanOrEqualTo(info.height));
      expect(scan.scannedHeight, scan.tipHeight);

      // Receive 5 GRIN from the CLI wallet by Slatepack (S1 from grin-wallet,
      // S2 from this library, finalized and posted by grin-wallet).
      final address = (await wallet.getAddress()).slatepackAddress;
      expect(address, startsWith('tgrin1'));
      final s1 = _armored(await _cliRun(['send', '-d', address, '5']));
      final s2 = await wallet.receiveSlatepack(s1);
      expect(s2.state, GrinSlateState.s2);
      expect(s2.amount, BigInt.from(5) * _grin);
      final s2File = File('$dir/s2.slatepack')
        ..writeAsStringSync(s2.armoredSlatepack);
      await _cliRun(['finalize', '-i', s2File.path]);

      await _waitUntil(() async {
        final b = await wallet.getBalance(minimumConfirmations: 1);
        return b.amountCurrentlySpendable >= BigInt.from(5) * _grin;
      });
      final received = (await wallet.getTransactions()).transactions
          .where((t) => t.slateId == s2.slateId)
          .single;
      expect(received.confirmed, isTrue);
      expect(received.kernelExcess, isNotNull);

      // A real HTTP send to grin-wallet's listener.
      final sent = await wallet.sendHttp(
        GrinHttpSendArgs(
          amount: _grin,
          recipientUrlOrAddress: peerUrl!,
          minimumConfirmations: 1,
        ),
      );
      expect(sent.posted, isTrue);
      expect(sent.state, GrinSlateState.s3);

      // Let the miner confirm that send so its change is spendable again.
      await _waitUntil(() async {
        final b = await wallet.getBalance(minimumConfirmations: 1);
        return b.amountCurrentlySpendable > _grin + _grin ~/ BigInt.from(10);
      });

      // The same send through a SOCKS5 proxy, when one is configured.
      if (socks != null && socks.isNotEmpty) {
        await wallet.updateNetwork(
          nodeUrl: nodeUrl,
          apiSecret: null,
          socksProxy: socks,
        );
        final proxied = await wallet.sendHttp(
          GrinHttpSendArgs(
            amount: _grin,
            recipientUrlOrAddress: peerUrl,
            minimumConfirmations: 1,
          ),
        );
        expect(proxied.posted, isTrue);
        await wallet.updateNetwork(
          nodeUrl: nodeUrl,
          apiSecret: null,
          socksProxy: null,
        );
      }

      // Both sends confirm on the mined chain.
      await _waitUntil(() async {
        final txs = (await wallet.getTransactions()).transactions;
        final sends = txs.where((t) => t.type == GrinTxType.txSent).toList();
        return sends.isNotEmpty && sends.every((t) => t.confirmed);
      });
      final balance = await wallet.getBalance(minimumConfirmations: 1);
      expect(balance.total, lessThan(BigInt.from(5) * _grin));
      expect(balance.amountCurrentlySpendable, greaterThan(BigInt.zero));
    },
    skip: skip,
    timeout: const Timeout(Duration(minutes: 15)),
  );
}
