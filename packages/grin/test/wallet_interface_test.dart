import 'dart:async';

import 'package:grin/grin.dart';
import 'package:test/test.dart';

// App code written against the interfaces only, as a wallet integration
// would be: open the wallet, or create it when there is none, and report.
Future<String> openAndDescribe(
  GrinWalletBackend backend,
  GrinWalletConfig config,
) async {
  GrinWalletApi wallet;
  try {
    wallet = await backend.open(config: config, password: 'pw');
  } on GrinWalletNotFoundException {
    wallet = await backend.create(config: config, password: 'pw');
  }
  try {
    final address = await wallet.getAddress();
    final balance = await wallet.getBalance(refreshFromNode: false);
    return '${address.slatepackAddress} '
        '${formatGrin(balance.amountCurrentlySpendable)}';
  } finally {
    await wallet.close();
  }
}

final class _FakeBackend implements GrinWalletBackend {
  final Set<String> wallets = {};
  final List<String> calls = [];

  @override
  Future<GrinWalletApi> open({
    required GrinWalletConfig config,
    required String password,
  }) async {
    calls.add('open');
    if (!wallets.contains(config.walletDir)) {
      throw GrinException.fromJson(
        4,
        '{"code":4,"message":"no wallet","details":'
        '{"wallet_dir":"${config.walletDir}"}}',
      );
    }
    return _FakeWallet(config);
  }

  @override
  Future<GrinWalletApi> create({
    required GrinWalletConfig config,
    String? mnemonic,
    required String password,
  }) async {
    calls.add('create');
    wallets.add(config.walletDir);
    return _FakeWallet(config);
  }

  // The rest of the interface is not used by these tests.
  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

final class _FakeWallet implements GrinWalletApi {
  _FakeWallet(GrinWalletConfig config)
    : walletDir = config.walletDir,
      network = config.network;

  @override
  final String walletDir;

  @override
  final GrinNetwork network;

  final StreamController<GrinWalletEvent> _events =
      StreamController<GrinWalletEvent>.broadcast();
  bool _closed = false;

  @override
  bool get isClosed => _closed;

  @override
  Stream<GrinWalletEvent> get events => _events.stream;

  void _ensureOpen() {
    if (_closed) throw StateError('closed');
  }

  @override
  Future<GrinAddressInfo> getAddress({int index = 0}) async {
    _ensureOpen();
    return GrinAddressInfo.fromJson({
      'slatepack_address': 'tgrin1fake',
      'onion_v3_url': 'http://fake.onion',
      'index': index,
    });
  }

  @override
  Future<GrinBalance> getBalance({
    bool refreshFromNode = true,
    int minimumConfirmations = 10,
  }) async {
    _ensureOpen();
    return GrinBalance.fromJson({
      'last_confirmed_height': 7,
      'minimum_confirmations': minimumConfirmations,
      'total': 2500000000,
      'amount_awaiting_finalization': 0,
      'amount_awaiting_confirmation': 0,
      'amount_immature': 1000000000,
      'amount_currently_spendable': 1500000000,
      'amount_locked': 0,
      'amount_reverted': 0,
      'refreshed_from_node': refreshFromNode,
    });
  }

  @override
  Future<GrinScanResult> scan({
    int startHeight = 0,
    bool deleteUnconfirmed = false,
  }) async {
    _ensureOpen();
    _events.add(GrinScanStarted(startHeight: startHeight));
    final result = GrinScanResult.fromJson({
      'scanned_height': 7,
      'tip_height': 7,
    });
    _events.add(GrinScanFinished(result: result));
    return result;
  }

  @override
  Future<void> close() async {
    if (_closed) return;
    _closed = true;
    _events.add(const GrinWalletClosed());
    await _events.close();
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

void main() {
  const config = GrinWalletConfig(
    walletDir: '/wallets/a',
    network: GrinNetwork.testnet,
    nodeUrl: 'http://127.0.0.1:1',
  );

  test('app code creates a missing wallet, then opens it', () async {
    final backend = _FakeBackend();
    expect(await openAndDescribe(backend, config), 'tgrin1fake 1.5');
    expect(backend.calls, ['open', 'create']);

    expect(await openAndDescribe(backend, config), 'tgrin1fake 1.5');
    expect(backend.calls, ['open', 'create', 'open']);
  });

  test('the not-found error carries the directory', () async {
    final backend = _FakeBackend();
    await expectLater(
      backend.open(config: config, password: 'pw'),
      throwsA(
        isA<GrinWalletNotFoundException>()
            .having((e) => e.code, 'code', 4)
            .having((e) => e.walletDir, 'walletDir', '/wallets/a'),
      ),
    );
  });

  test('events and close through the interface', () async {
    final GrinWalletApi wallet = _FakeWallet(config);
    final events = <GrinWalletEvent>[];
    final done = wallet.events.listen(events.add).asFuture<void>();

    final result = await wallet.scan(startHeight: 3);
    expect(result.scannedHeight, 7);
    await wallet.close();
    await done;

    expect(wallet.isClosed, isTrue);
    expect(events, [
      isA<GrinScanStarted>().having((e) => e.startHeight, 'startHeight', 3),
      isA<GrinScanFinished>().having((e) => e.result, 'result', result),
      isA<GrinWalletClosed>(),
    ]);
    expect(wallet.getAddress, throwsStateError);
  });
}
