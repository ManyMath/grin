import 'package:grin/grin.dart';
import 'package:test/test.dart';

void main() {
  group('units', () {
    test('formatGrin is exact', () {
      expect(formatGrin(BigInt.zero), '0');
      expect(formatGrin(BigInt.one), '0.000000001');
      expect(formatGrin(BigInt.from(1500000000)), '1.5');
      expect(formatGrin(BigInt.from(2000000000)), '2');
      expect(
        formatGrin(BigInt.parse('18446744073709551615')),
        '18446744073.709551615',
      );
      expect(formatGrin(BigInt.from(-1500000000)), '-1.5');
    });

    test('parseGrin is exact and strict', () {
      expect(parseGrin('0'), BigInt.zero);
      expect(parseGrin('1'), GrinUnits.nanogrinPerGrin);
      expect(parseGrin(' 1.5 '), BigInt.from(1500000000));
      expect(parseGrin('0.000000001'), BigInt.one);
      expect(parseGrin('0.1'), BigInt.from(100000000));
      expect(
        parseGrin('18446744073.709551615'),
        BigInt.parse('18446744073709551615'),
      );
      for (final bad in [
        '',
        '.5',
        '1.',
        '-1',
        '1e3',
        '1,5',
        '0.0000000001',
        'abc',
      ]) {
        expect(() => parseGrin(bad), throwsFormatException, reason: bad);
      }
    });

    test('format and parse round trip', () {
      for (final n in [0, 1, 9, 10, 999999999, 1000000000, 123456789012345]) {
        final amount = BigInt.from(n);
        expect(parseGrin(formatGrin(amount)), amount);
      }
    });
  });

  group('models', () {
    test('GrinWalletConfig.toJson matches config_json', () {
      const config = GrinWalletConfig(
        walletDir: '/w',
        network: GrinNetwork.testnet,
        nodeUrl: 'https://node.example.com:3413',
        socksProxy: '127.0.0.1:9050',
        requestTimeout: Duration(milliseconds: 1500),
      );
      expect(config.toJson(), {
        'wallet_dir': '/w',
        'network': 1,
        'node_url': 'https://node.example.com:3413',
        'node_api_secret': null,
        'socks_proxy': '127.0.0.1:9050',
        'request_timeout_secs': 2,
      });
    });

    test('GrinWalletConfig.toString hides the secret', () {
      const config = GrinWalletConfig(
        walletDir: '/w',
        network: GrinNetwork.mainnet,
        nodeUrl: 'http://n',
        nodeApiSecret: 'hunter2',
      );
      expect(config.toString(), isNot(contains('hunter2')));
    });

    test('GrinNetwork values', () {
      expect(GrinNetwork.values.map((n) => n.value), [0, 1, 2, 3]);
      expect(GrinNetwork.fromValue(2), GrinNetwork.userTesting);
      expect(() => GrinNetwork.fromValue(9), throwsArgumentError);
    });

    test('GrinBalance.fromJson reads BigInt amounts', () {
      final balance = GrinBalance.fromJson({
        'last_confirmed_height': 12,
        'minimum_confirmations': 10,
        'total': 60000000000,
        'amount_awaiting_finalization': 1,
        'amount_awaiting_confirmation': 2,
        'amount_immature': 3,
        'amount_currently_spendable': 4,
        'amount_locked': 5,
        'amount_reverted': 6,
        'refreshed_from_node': true,
      });
      expect(balance.total, BigInt.from(60000000000));
      expect(balance.spendable, BigInt.from(4));
      expect(balance.lastConfirmedHeight, 12);
      expect(balance.refreshedFromNode, isTrue);
      expect(
        balance,
        isNot(
          equals(
            GrinBalance.fromJson({
              'last_confirmed_height': 13,
              'minimum_confirmations': 10,
              'total': 60000000000,
              'amount_awaiting_finalization': 1,
              'amount_awaiting_confirmation': 2,
              'amount_immature': 3,
              'amount_currently_spendable': 4,
              'amount_locked': 5,
              'amount_reverted': 6,
              'refreshed_from_node': true,
            }),
          ),
        ),
      );
    });

    test('a missing key is a FormatException', () {
      expect(
        () => GrinScanResult.fromJson({'scanned_height': 1}),
        throwsFormatException,
      );
    });

    test('GrinAddressValidation maps an unknown kind to unknown', () {
      expect(
        GrinAddressValidation.fromJson({'kind': 'carrier pigeon', 'url': 'x'})
            .kind,
        GrinAddressKind.unknown,
      );
      expect(
        () => GrinAddressValidation.fromJson({'kind': 5, 'url': 'x'}),
        throwsFormatException,
      );
    });
  });
}
