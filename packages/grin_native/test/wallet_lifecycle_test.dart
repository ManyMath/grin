import 'dart:async';
import 'dart:io';

import 'package:grin_native/grin_native.dart';
import 'package:path/path.dart' as p;
import 'package:test/test.dart';

import 'support.dart';

void main() {
  test('create, getAddress, close, open gives the same address', () async {
    final dir = await makeTempDir();
    final first = await createTestWallet(dir);
    final address = await first.getAddress();
    expect(address.index, 0);
    expect(address.slatepackAddress, startsWith('tgrin1'));
    expect(first.walletDir, dir.path);
    expect(first.network, GrinNetwork.automatedTesting);
    await first.close();

    final second = await GrinWalletClient.open(
      config: testConfig(dir),
      password: 'pw',
    );
    addTearDown(second.close);
    expect(await second.getAddress(), address);
    expect(
      (await second.getAddress(index: 1)).slatepackAddress,
      isNot(address.slatepackAddress),
    );
  });

  test('close during a call waits for it and frees the directory', () async {
    final dir = await makeTempDir();
    final wallet = await createTestWallet(dir);

    var balanceSettled = false;
    final inFlight = wallet
        .getBalance(refreshFromNode: false)
        .whenComplete(() => balanceSettled = true);
    final closing = wallet.close();

    expect(wallet.isClosed, isTrue);
    expect(wallet.getBalance, throwsStateError);
    expect(identical(wallet.close(), closing), isTrue);

    final balance = await inFlight;
    await closing;
    expect(balanceSettled, isTrue);
    expect(balance.total, BigInt.zero);

    final reopened = await GrinWalletClient.open(
      config: testConfig(dir),
      password: 'pw',
    );
    addTearDown(reopened.close);
    expect(reopened.isClosed, isFalse);
  });

  test('close completes even if a queued call fails', () async {
    final dir = await makeTempDir();
    final wallet = await createTestWallet(dir);
    final failing = wallet.getMnemonic('wrong');
    final failed = expectLater(failing, throwsA(isA<GrinException>()));
    await wallet.close();
    await failed;
  });

  test('operations of one client run in call order', () async {
    final dir = await makeTempDir();
    final wallet = await createTestWallet(dir);
    final order = <int>[];
    final calls = [
      for (var i = 0; i < 4; i++)
        wallet.getAddress(index: i).then((a) {
          order.add(a.index);
          return a;
        }),
    ];
    await Future.wait(calls);
    expect(order, [0, 1, 2, 3]);
  });

  test('an error in one call does not stop the queue', () async {
    final dir = await makeTempDir();
    final wallet = await createTestWallet(dir);
    final bad = wallet.getMnemonic('wrong');
    final good = wallet.getAddress();
    await expectLater(bad, throwsA(isA<GrinInvalidPasswordException>()));
    expect((await good).index, 0);
  });

  test('a second open of an open wallet throws WalletBusy', () async {
    final dir = await makeTempDir();
    final wallet = await createTestWallet(dir);
    await expectLater(
      GrinWalletClient.open(config: testConfig(dir), password: 'pw'),
      throwsA(
        isA<GrinWalletBusyException>().having(
          (e) => e.walletDir,
          'walletDir',
          contains(p.basename(dir.path)),
        ),
      ),
    );
    await expectLater(
      GrinWalletClient.create(config: testConfig(dir), password: 'pw'),
      throwsA(isA<GrinWalletBusyException>()),
    );
    await expectLater(
      GrinWalletClient.delete(walletDir: dir.path),
      throwsA(isA<GrinWalletBusyException>()),
    );
    // The wallet is still usable and nothing was deleted.
    expect((await wallet.getAddress()).index, 0);
    expect(Directory(p.join(dir.path, 'wallet_data')).existsSync(), isTrue);
  });

  test('a wrong password fails and leaves the directory free', () async {
    final dir = await makeTempDir();
    final wallet = await createTestWallet(dir);
    await wallet.close();

    await expectLater(
      GrinWalletClient.open(config: testConfig(dir), password: 'nope'),
      throwsA(isA<GrinInvalidPasswordException>()),
    );
    final reopened = await GrinWalletClient.open(
      config: testConfig(dir),
      password: 'pw',
    );
    addTearDown(reopened.close);
  });

  test('open without a wallet throws WalletNotFound', () async {
    final dir = await makeTempDir();
    await expectLater(
      GrinWalletClient.open(config: testConfig(dir), password: 'pw'),
      throwsA(isA<GrinWalletNotFoundException>()),
    );
  });

  test('create over an existing wallet throws WalletExists', () async {
    final dir = await makeTempDir();
    final wallet = await createTestWallet(dir);
    await wallet.close();
    await expectLater(
      GrinWalletClient.create(config: testConfig(dir), password: 'pw'),
      throwsA(isA<GrinWalletExistsException>()),
    );
  });

  test(
    'delete after close removes wallet_data and keeps the directory',
    () async {
      final dir = await makeTempDir();
      final wallet = await createTestWallet(dir);
      final walletData = Directory(p.join(dir.path, 'wallet_data'));
      expect(walletData.existsSync(), isTrue);
      await wallet.close();

      await GrinWalletClient.delete(walletDir: dir.path);
      expect(walletData.existsSync(), isFalse);
      expect(dir.existsSync(), isTrue);

      // Deleting again counts as success.
      await GrinWalletClient.delete(walletDir: dir.path);
    },
  );

  test('the mnemonic round trips and the password can change', () async {
    final dir = await makeTempDir();
    final wallet = await createTestWallet(dir, mnemonic: knownMnemonic);
    expect(await wallet.getMnemonic('pw'), knownMnemonic);
    await expectLater(
      wallet.getMnemonic('wrong'),
      throwsA(isA<GrinInvalidPasswordException>()),
    );
    await expectLater(
      wallet.changePassword('wrong', 'new'),
      throwsA(isA<GrinInvalidPasswordException>()),
    );
    await wallet.changePassword('pw', 'new');
    await wallet.close();

    await expectLater(
      GrinWalletClient.open(config: testConfig(dir), password: 'pw'),
      throwsA(isA<GrinInvalidPasswordException>()),
    );
    final reopened = await GrinWalletClient.open(
      config: testConfig(dir),
      password: 'new',
    );
    addTearDown(reopened.close);
    expect(await reopened.getMnemonic('new'), knownMnemonic);
  });

  test('a restored wallet has the address of the phrase', () async {
    final a = await makeTempDir();
    final first = await createTestWallet(a, mnemonic: knownMnemonic);
    final address = await first.getAddress();
    await first.close();

    final b = await makeTempDir();
    final second = await createTestWallet(b, mnemonic: knownMnemonic);
    expect(await second.getAddress(), address);
  });

  test('a node that cannot be reached is reported, not hidden', () async {
    final dir = await makeTempDir();
    final wallet = await createTestWallet(dir);
    await expectLater(
      wallet.getChainHeight(),
      throwsA(isA<GrinNodeException>()),
    );
    final balance = await wallet.getBalance();
    expect(balance.refreshedFromNode, isFalse);
    await wallet.updateNetwork(
      nodeUrl: 'http://127.0.0.1:2',
      apiSecret: null,
      socksProxy: null,
      networkEnabled: true,
    );
    await expectLater(
      wallet.updateNetwork(
        nodeUrl: 'not a url',
        apiSecret: null,
        socksProxy: null,
        networkEnabled: true,
      ),
      throwsA(isA<GrinException>()),
    );
  });

  test('with the network off nothing connects', () async {
    final node = await _CountingServer.start();
    final peer = await _CountingServer.start();
    final dir = await makeTempDir();
    final config = GrinWalletConfig(
      walletDir: dir.path,
      network: GrinNetwork.automatedTesting,
      nodeUrl: node.url,
      requestTimeout: const Duration(seconds: 2),
      networkEnabled: false,
    );
    final wallet = await GrinWalletClient.create(
      config: config,
      mnemonic: knownMnemonic,
      isNewWallet: true,
      password: 'pw',
    );
    addTearDown(wallet.close);

    final offline = throwsA(
      isA<GrinNodeException>().having(
        (e) => e.details?['reason'],
        'reason',
        'network_disabled',
      ),
    );
    await expectLater(wallet.getChainHeight(), offline);
    await expectLater(wallet.getBalance(), offline);
    await expectLater(wallet.getTransactions(), offline);
    await expectLater(wallet.scan(), offline);
    await expectLater(wallet.estimateFee(amount: BigInt.one), offline);
    await expectLater(
      wallet.initSend(GrinSendArgs(amount: BigInt.one)),
      offline,
    );
    await expectLater(
      wallet.sendHttp(
        GrinHttpSendArgs(amount: BigInt.one, recipientUrlOrAddress: peer.url),
      ),
      offline,
    );
    await expectLater(
      wallet.issueInvoice(GrinInvoiceArgs(amount: BigInt.one)),
      offline,
    );
    await expectLater(wallet.cancelTransaction(txId: 0), offline);

    // What needs no node still works.
    final balance = await wallet.getBalance(refreshFromNode: false);
    expect(balance.refreshedFromNode, isFalse);
    expect(await wallet.lastScannedHeight(), 0);
    expect((await wallet.getAddress()).slatepackAddress, startsWith('tgrin1'));
    await Future<void>.delayed(const Duration(milliseconds: 200));
    expect(node.connections, 0);
    expect(peer.connections, 0);

    // The stateless check is not affected and shows that the server counts.
    await expectLater(
      GrinWalletClient.testNodeConnection(nodeUrl: node.url),
      throwsA(isA<GrinNodeException>()),
    );
    expect(await node.waitAbove(0), isTrue);

    final before = node.connections;
    await wallet.updateNetwork(
      nodeUrl: node.url,
      apiSecret: null,
      socksProxy: null,
      networkEnabled: true,
    );
    await expectLater(
      wallet.getChainHeight(),
      throwsA(
        isA<GrinNodeException>().having((e) => e.details, 'details', isNull),
      ),
    );
    expect(await node.waitAbove(before), isTrue);

    await wallet.updateNetwork(
      nodeUrl: node.url,
      apiSecret: null,
      socksProxy: null,
      networkEnabled: false,
    );
    final after = node.connections;
    await expectLater(wallet.getChainHeight(), offline);
    await Future<void>.delayed(const Duration(milliseconds: 200));
    expect(node.connections, after);
    expect(peer.connections, 0);
  });

  test('testNodeConnection reports an unreachable node', () async {
    await expectLater(
      GrinWalletClient.testNodeConnection(nodeUrl: 'http://127.0.0.1:1'),
      throwsA(isA<GrinNodeException>()),
    );
    await expectLater(
      GrinWalletClient.testNodeConnection(nodeUrl: 'nope'),
      throwsA(isA<GrinInvalidArgumentException>()),
    );
  });

  test('the events stream ends with GrinWalletClosed', () async {
    final dir = await makeTempDir();
    final wallet = await createTestWallet(dir);
    final events = <GrinWalletEvent>[];
    final done = Completer<void>();
    wallet.events.listen(events.add, onDone: done.complete);
    await wallet.close();
    await done.future;
    expect(events.last, isA<GrinWalletClosed>());
  });

  test('a cancel with no scan running is a no-op', () async {
    final dir = await makeTempDir();
    final wallet = await createTestWallet(dir);
    wallet.cancelScan();
    expect(wallet.scanProgress().cancelRequested, isFalse);
    // The next scan runs normally (no node is reachable here, so it fails
    // with a node error rather than Cancelled).
    await expectLater(
      wallet.scan(),
      throwsA(isNot(isA<GrinCancelledException>())),
    );
    expect(wallet.scanProgress().cancelRequested, isFalse);
  });

  test('a scan queued before close does not start', () async {
    final dir = await makeTempDir();
    final wallet = await createTestWallet(dir);
    final blocker = wallet.getBalance(refreshFromNode: false);
    final queuedScan = wallet.scan();
    final closing = wallet.close();
    await blocker;
    await expectLater(queuedScan, throwsA(isA<StateError>()));
    await closing;
  });

  test('close completes while an events listener is paused', () async {
    final dir = await makeTempDir();
    final wallet = await createTestWallet(dir);
    final seen = <GrinWalletEvent>[];
    final done = Completer<void>();
    final subscription = wallet.events.listen(seen.add, onDone: done.complete);
    addTearDown(subscription.cancel);
    subscription.pause();

    // A paused listener gets no done event, and close() must not wait for it.
    await wallet.close().timeout(const Duration(seconds: 10));
    expect(wallet.isClosed, isTrue);
    expect(done.isCompleted, isFalse);

    subscription.resume();
    await done.future.timeout(const Duration(seconds: 10));
    expect(seen.last, isA<GrinWalletClosed>());
  });

  test('close can be awaited from inside an await for body', () async {
    final dir = await makeTempDir();
    final wallet = await createTestWallet(dir);
    final scan = wallet.scan().then<Object?>(
      (r) => r,
      onError: (Object e) => e,
    );
    final closed = Completer<void>();
    final loop = () async {
      await for (final event in wallet.events) {
        if (event is GrinScanFinished) {
          await wallet.close();
          closed.complete();
        }
      }
    }();
    await scan;
    await closed.future.timeout(const Duration(seconds: 10));
    await loop.timeout(const Duration(seconds: 10));
  });

  test('queued operations run in order and an error stays with its '
      'caller', () async {
    final dir = await makeTempDir();
    final wallet = await createTestWallet(dir);
    final gate = Completer<void>();
    final order = <String>[];

    final first = wallet.runQueued(() async {
      await gate.future;
      order.add('first');
      return 1;
    });
    final second = wallet.runQueued<int>(() async {
      order.add('second');
      throw StateError('second failed');
    });
    final third = wallet.runQueued(() async {
      order.add('third');
      return 3;
    });
    final secondCheck = expectLater(second, throwsStateError);

    // Nothing behind the first operation may start before it finishes.
    await Future<void>.delayed(const Duration(milliseconds: 50));
    expect(order, isEmpty);

    gate.complete();
    expect(await first, 1);
    await secondCheck;
    expect(await third, 3);
    expect(order, ['first', 'second', 'third']);

    // The queue is still usable after the failure.
    expect(await wallet.runQueued(() async => 4), 4);
  });
}

/// A local TCP server that counts connections and closes each one at once.
final class _CountingServer {
  _CountingServer._(this._socket) {
    _socket.listen((client) {
      connections++;
      client.destroy();
    });
    addTearDown(_socket.close);
  }

  static Future<_CountingServer> start() async => _CountingServer._(
    await ServerSocket.bind(InternetAddress.loopbackIPv4, 0),
  );

  final ServerSocket _socket;
  int connections = 0;

  String get url => 'http://127.0.0.1:${_socket.port}';

  /// Waits up to five seconds for more than [count] connections.
  Future<bool> waitAbove(int count) async {
    final deadline = DateTime.now().add(const Duration(seconds: 5));
    while (DateTime.now().isBefore(deadline)) {
      if (connections > count) return true;
      await Future<void>.delayed(const Duration(milliseconds: 10));
    }
    return false;
  }
}
