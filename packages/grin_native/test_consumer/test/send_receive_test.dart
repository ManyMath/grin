import 'dart:io';

import 'package:grin_native/grin_native.dart';
import 'package:grin_native/testing.dart';
import 'package:grin_native_test_consumer/payments.dart';
import 'package:test/test.dart';

void main() {
  test('send, receive and finalize through GrinWallet', () async {
    final root = await Directory.systemTemp.createTemp('grin_consumer_');
    addTearDown(() => root.delete(recursive: true));
    final chainDir = Directory('${root.path}/chain')..createSync();

    // The harness is native-only and hands out the concrete client; mining
    // needs it. Everything else goes through GrinWallet.
    Future<GrinWalletClient> wallet(String id) async {
      final dir = Directory('${root.path}/$id')..createSync();
      final created = await GrinTestChain.createWallet(
        chainDir: chainDir.path,
        walletDir: dir.path,
        clientId: id,
        password: 'pw',
      );
      addTearDown(created.close);
      return created;
    }

    final miner = await wallet('alice');
    final GrinWallet alice = miner;
    final GrinWallet bob = await wallet('bob');
    await GrinTestChain.mineBlocks(miner, 5);

    const GrinWalletBackend backend = GrinNativeBackend();
    final bobAddress = (await bob.getAddress()).slatepackAddress;
    expect(
      isValidAddress(backend, bobAddress, GrinNetwork.automatedTesting),
      isTrue,
    );
    expect(
      isValidAddress(backend, 'nope', GrinNetwork.automatedTesting),
      isFalse,
    );

    final amount = parseGrin('1.5');
    final result = await payBySlatepack(
      sender: alice,
      recipient: bob,
      amount: amount,
    );
    expect(result.state, GrinSlateState.s3);
    expect(result.posted, isTrue);
    expect(result.amount, amount);

    await GrinTestChain.mineBlocks(miner, 1);

    final bobBalance = await bob.getBalance(minimumConfirmations: 1);
    expect(bobBalance.amountCurrentlySpendable, amount);
    final received = (await bob.getTransactions()).bySlateId(result.slateId)!;
    expect(received.type, GrinTxType.txReceived);
    expect(received.confirmed, isTrue);
    expect(received.amountCredited, amount);

    final sent = (await alice.getTransactions()).bySlateId(result.slateId)!;
    expect(sent.type, GrinTxType.txSent);
    expect(sent.confirmed, isTrue);
    expect(sent.fee, result.fee);
    final aliceBalance = await alice.getBalance(minimumConfirmations: 1);
    expect(aliceBalance.amountLocked, BigInt.zero);
  });
}
