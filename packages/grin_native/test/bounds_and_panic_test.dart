import 'package:grin_native/grin_native.dart';
import 'package:grin_native/testing.dart';
import 'package:test/test.dart';

import 'support.dart';

void main() {
  group('panic', () {
    test('a native panic comes back as GrinInternalPanicException', () {
      expect(
        GrinTestChain.testPanic,
        throwsA(
          isA<GrinInternalPanicException>()
              .having((e) => e.code, 'code', 16)
              .having((e) => e.message, 'message', contains('grin_test_panic')),
        ),
      );
    });

    test('the library still works after a panic', () async {
      expect(GrinTestChain.testPanic, throwsA(isA<GrinException>()));
      final mnemonic = await GrinWalletClient.generateMnemonic(wordCount: 12);
      expect(mnemonic.split(' '), hasLength(12));
    });
  });

  group('bounds', () {
    test('an empty walletDir is an invalid argument', () {
      expect(
        GrinWalletClient.create(
          config: const GrinWalletConfig(
            walletDir: '',
            network: GrinNetwork.automatedTesting,
            nodeUrl: 'http://127.0.0.1:1',
          ),
          password: 'pw',
        ),
        throwsA(isA<GrinInvalidArgumentException>()),
      );
    });

    test('an unsupported node URL is an invalid argument', () async {
      final dir = await makeTempDir();
      final config = GrinWalletConfig(
        walletDir: dir.path,
        network: GrinNetwork.automatedTesting,
        nodeUrl: 'ftp://example.com',
      );
      expect(
        GrinWalletClient.create(config: config, password: 'pw'),
        throwsA(isA<GrinInvalidArgumentException>()),
      );
    });

    test('initLogs may be repeated and rejects an unknown level', () {
      GrinWalletClient.initLogs();
      GrinWalletClient.initLogs(level: 'warn');
      GrinWalletClient.initLogs(level: 'ERROR');
      expect(
        () => GrinWalletClient.initLogs(level: 'loud'),
        throwsA(isA<GrinInvalidArgumentException>()),
      );
    });

    test(
      'a closed client throws StateError and never reaches the handle',
      () async {
        final dir = await makeTempDir();
        final wallet = await createTestWallet(dir);
        await wallet.close();
        expect(wallet.isClosed, isTrue);
        expect(wallet.getAddress, throwsStateError);
        expect(wallet.getChainHeight, throwsStateError);
        expect(wallet.getBalance, throwsStateError);
        expect(wallet.scan, throwsStateError);
        expect(wallet.scanProgress, throwsStateError);
        expect(wallet.cancelScan, throwsStateError);
        expect(() => wallet.getMnemonic('pw'), throwsStateError);
        expect(() => wallet.changePassword('pw', 'x'), throwsStateError);
        expect(
          () => wallet.updateNetwork(
            nodeUrl: 'http://127.0.0.1:2',
            apiSecret: null,
            socksProxy: null,
            networkEnabled: true,
          ),
          throwsStateError,
        );
      },
    );

    test('a bad handle is reported by the native library', () {
      // Bypasses the client on purpose: handle 1 << 40 was never issued.
      final ghost = GrinWalletClient.fromHandle(
        handle: 1 << 40,
        walletDir: '/nonexistent',
        network: GrinNetwork.mainnet,
      );
      expect(ghost.scanProgress, throwsA(isA<GrinInvalidHandleException>()));
      // The finalizer ignores unknown handles, so there is nothing to undo.
    });
  });

  group('GrinException.fromJson', () {
    test('InsufficientFunds exposes BigInt amounts', () {
      final e = GrinException.fromJson(
        10,
        '{"code":10,"message":"not enough","details":{"required":5000000000,"available":1234}}',
      );
      expect(e, isA<GrinInsufficientFundsException>());
      e as GrinInsufficientFundsException;
      expect(e.required, BigInt.from(5000000000));
      expect(e.available, BigInt.from(1234));
      expect(e.code, 10);
      expect(e.message, 'not enough');
    });

    test('InsufficientFunds without details has null amounts', () {
      final e = GrinException.fromJson(
        10,
        '{"code":10,"message":"m","details":null}',
      ) as GrinInsufficientFundsException;
      expect(e.required, isNull);
      expect(e.available, isNull);
    });

    test('InsufficientFunds reads integer strings', () {
      final e = GrinException.fromJson(
        10,
        '{"code":10,"message":"m","details":'
        '{"required":"18446744073709551615","available":"7"}}',
      ) as GrinInsufficientFundsException;
      expect(e.required, BigInt.parse('18446744073709551615'));
      expect(e.available, BigInt.from(7));
    });

    test('InsufficientFunds with malformed amounts gives null', () {
      for (final bad in ['"abc"', '1.5', '-1', '"-5"', 'true', '[1]', '{}']) {
        final e = GrinException.fromJson(
          10,
          '{"code":10,"message":"m","details":'
          '{"required":$bad,"available":$bad}}',
        ) as GrinInsufficientFundsException;
        expect(() => e.required, returnsNormally, reason: bad);
        expect(e.required, isNull, reason: bad);
        expect(e.available, isNull, reason: bad);
      }
      // One malformed value does not affect the other.
      final e = GrinException.fromJson(
        10,
        '{"code":10,"message":"m","details":{"required":"x","available":3}}',
      ) as GrinInsufficientFundsException;
      expect(e.required, isNull);
      expect(e.available, BigInt.from(3));
    });

    test('scannedHeight is read from the details of any scan error', () {
      final cancelled = GrinException.fromJson(
        14,
        '{"code":14,"message":"m","details":'
        '{"scanned_height":4,"tip_height":12}}',
      );
      expect(cancelled.scannedHeight, 4);
      final failed = GrinException.fromJson(
        9,
        '{"code":9,"message":"m","details":'
        '{"scanned_height":0,"tip_height":12}}',
      );
      expect(failed, isA<GrinNodeException>());
      expect(failed.scannedHeight, 0);
      for (final details in ['null', '{}', '{"scanned_height":"4"}']) {
        final e = GrinException.fromJson(
          14,
          '{"code":14,"message":"m","details":$details}',
        );
        expect(e.scannedHeight, isNull, reason: details);
      }
    });

    test('finalized and released are read from the details', () {
      final refused = GrinException.fromJson(
        9,
        '{"code":9,"message":"m","details":'
        '{"slate_id":"2b1c7f2e-9d4a-4b8e-a3c5-0f6d1e2a3b4c","finalized":true}}',
      );
      expect(refused.finalized, isTrue);
      expect(refused.released, isFalse);
      expect(refused.slateId, '2b1c7f2e-9d4a-4b8e-a3c5-0f6d1e2a3b4c');

      final undone = GrinException.fromJson(
        11,
        '{"code":11,"message":"m","details":'
        '{"slate_id":"2b1c7f2e-9d4a-4b8e-a3c5-0f6d1e2a3b4c","released":true}}',
      );
      expect(undone.released, isTrue);
      expect(undone.finalized, isFalse);

      // Anything but the JSON value true is false.
      for (final details in [
        'null',
        '{}',
        '{"finalized":"true"}',
        '{"finalized":1,"released":"yes"}',
      ]) {
        final e = GrinException.fromJson(
          9,
          '{"code":9,"message":"m","details":$details}',
        );
        expect(e.finalized, isFalse, reason: details);
        expect(e.released, isFalse, reason: details);
      }
    });

    test('WalletBusy exposes the directory', () {
      final e = GrinException.fromJson(
        18,
        '{"code":18,"message":"busy","details":{"wallet_dir":"/data/w"}}',
      ) as GrinWalletBusyException;
      expect(e.walletDir, '/data/w');
      expect(e.isScanInProgress, isFalse);
    });

    test('WalletBusy for a running scan says so', () {
      final e = GrinException.fromJson(
        18,
        '{"code":18,"message":"busy","details":{"wallet_dir":"/data/w","reason":"scan_in_progress"}}',
      ) as GrinWalletBusyException;
      expect(e.isScanInProgress, isTrue);
    });

    test('WalletNotFound and WalletExists expose the directory', () {
      final notFound = GrinException.fromJson(
        4,
        '{"code":4,"message":"m","details":{"wallet_dir":"a"}}',
      ) as GrinWalletNotFoundException;
      final exists = GrinException.fromJson(
        6,
        '{"code":6,"message":"m","details":{"wallet_dir":"b"}}',
      ) as GrinWalletExistsException;
      expect(notFound.walletDir, 'a');
      expect(exists.walletDir, 'b');
    });

    test('every known code maps to its own class', () {
      const names = {
        1: 'GrinInvalidArgumentException',
        2: 'GrinInvalidUtf8Exception',
        3: 'GrinJsonException',
        4: 'GrinWalletNotFoundException',
        5: 'GrinInvalidPasswordException',
        6: 'GrinWalletExistsException',
        7: 'GrinInvalidHandleException',
        8: 'GrinLifecycleException',
        9: 'GrinNodeException',
        10: 'GrinInsufficientFundsException',
        11: 'GrinSlateException',
        12: 'GrinTransactionException',
        13: 'GrinNotFoundException',
        14: 'GrinCancelledException',
        15: 'GrinIoException',
        16: 'GrinInternalPanicException',
        17: 'GrinInternalException',
        18: 'GrinWalletBusyException',
        19: 'GrinHttpException',
        20: 'GrinNotSupportedException',
        21: 'GrinInvalidAddressException',
        22: 'GrinInvalidMnemonicException',
      };
      for (final entry in names.entries) {
        final e = GrinException.fromJson(
          entry.key,
          '{"code":${entry.key},"message":"m","details":null}',
        );
        expect(e.runtimeType.toString(), entry.value, reason: '${entry.key}');
        expect(e.code, entry.key);
        expect(e.details, isNull);
      }
    });

    test('an unknown code maps to GrinUnknownException', () {
      final e = GrinException.fromJson(
        999,
        '{"code":999,"message":"from the future","details":{"x":1}}',
      );
      expect(e, isA<GrinUnknownException>());
      expect(e.code, 999);
      expect(e.message, 'from the future');
      expect(e.details, {'x': 1});
    });

    test('text that is not JSON becomes the message', () {
      final e = GrinException.fromJson(17, 'plain text');
      expect(e, isA<GrinInternalException>());
      expect(e.message, 'plain text');
      expect(e.details, isNull);
    });

    test('the sealed hierarchy can be switched over', () {
      String describe(GrinException e) => switch (e) {
        GrinWalletBusyException() => 'busy',
        GrinInvalidPasswordException() => 'password',
        _ => 'other',
      };
      expect(describe(GrinException.fromJson(18, '{}')), 'busy');
      expect(describe(GrinException.fromJson(5, '{}')), 'password');
      expect(describe(GrinException.fromJson(1, '{}')), 'other');
    });
  });
}
