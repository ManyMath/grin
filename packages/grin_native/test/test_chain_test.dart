import 'dart:io';

import 'package:grin_native/grin_native.dart';
import 'package:grin_native/testing.dart';
import 'package:path/path.dart' as p;
import 'package:test/test.dart';

import 'support.dart';

void main() {
  test('mined coinbase becomes spendable and a scan finds it', () async {
    final root = await makeTempDir('grin_chain_');
    final chainDir = Directory(p.join(root.path, 'chain'))..createSync();

    Future<GrinWalletClient> wallet(String id) async {
      final dir = Directory(p.join(root.path, id))..createSync();
      final client = await GrinTestChain.createWallet(
        chainDir: chainDir.path,
        walletDir: dir.path,
        clientId: id,
        password: 'pw',
      );
      addTearDown(client.close);
      return client;
    }

    final alice = await wallet('alice');
    final bob = await wallet('bob');
    expect(alice.network, GrinNetwork.automatedTesting);
    expect(
      (await alice.getAddress()).slatepackAddress,
      isNot((await bob.getAddress()).slatepackAddress),
    );

    final events = <GrinWalletEvent>[];
    final subscription = alice.events.listen(events.add);
    addTearDown(subscription.cancel);

    final height = await GrinTestChain.mineBlocks(alice, 5);
    expect(height, greaterThanOrEqualTo(5));

    final tip = await alice.getChainHeight();
    expect(tip.height, height);

    final balance = await alice.getBalance(minimumConfirmations: 1);
    expect(balance.refreshedFromNode, isTrue);
    expect(balance.amountCurrentlySpendable, greaterThan(BigInt.zero));
    expect(balance.total, greaterThanOrEqualTo(balance.spendable));
    expect((await bob.getBalance(minimumConfirmations: 1)).total, BigInt.zero);

    expect(alice.scanProgress().isScanning, isFalse);
    final result = await alice.scan();
    expect(result.tipHeight, height);
    expect(result.scannedHeight, result.tipHeight);

    final progress = alice.scanProgress();
    expect(progress.isScanning, isFalse);
    expect(progress.tipHeight, height);
    expect(progress.progressPct, 100);

    await Future<void>.delayed(Duration.zero);
    expect(events.whereType<GrinScanStarted>(), hasLength(1));
    final finished = events.whereType<GrinScanFinished>().single;
    expect(finished.error, isNull);
    expect(finished.result, result);
    expect(
      events.indexWhere((e) => e is GrinScanStarted),
      lessThan(events.indexWhere((e) => e is GrinScanFinished)),
    );

    final after = await alice.getBalance(minimumConfirmations: 1);
    expect(after.amountCurrentlySpendable, balance.amountCurrentlySpendable);
  });

  test('reusing a client id re-attaches the same wallet', () async {
    final root = await makeTempDir('grin_chain_');
    final chainDir = Directory(p.join(root.path, 'chain'))..createSync();
    final walletDir = Directory(p.join(root.path, 'w'))..createSync();

    Future<GrinWalletClient> attach() => GrinTestChain.createWallet(
      chainDir: chainDir.path,
      walletDir: walletDir.path,
      clientId: 'carol',
      password: 'pw',
    );

    final first = await attach();
    final address = await first.getAddress();
    await first.close();
    final second = await attach();
    addTearDown(second.close);
    expect(await second.getAddress(), address);
  });

  test('mineBlocks on a closed wallet throws StateError', () async {
    final root = await makeTempDir('grin_chain_');
    final chainDir = Directory(p.join(root.path, 'chain'))..createSync();
    final walletDir = Directory(p.join(root.path, 'w'))..createSync();
    final wallet = await GrinTestChain.createWallet(
      chainDir: chainDir.path,
      walletDir: walletDir.path,
      clientId: 'dave',
      password: 'pw',
    );
    await wallet.close();
    expect(() => GrinTestChain.mineBlocks(wallet, 1), throwsStateError);
  });

  test('mineBlocks refuses a count below 1', () async {
    final root = await makeTempDir('grin_chain_');
    final chainDir = Directory(p.join(root.path, 'chain'))..createSync();
    final walletDir = Directory(p.join(root.path, 'w'))..createSync();
    final wallet = await GrinTestChain.createWallet(
      chainDir: chainDir.path,
      walletDir: walletDir.path,
      clientId: 'erin',
      password: 'pw',
    );
    addTearDown(wallet.close);
    expect(() => GrinTestChain.mineBlocks(wallet, 0), throwsArgumentError);
    expect(() => GrinTestChain.mineBlocks(wallet, -1), throwsArgumentError);
  });
}
