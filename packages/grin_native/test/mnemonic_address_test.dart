import 'package:grin_native/grin_native.dart';
import 'package:test/test.dart';

import 'support.dart';

void main() {
  group('mnemonic', () {
    for (final count in [12, 24]) {
      test('generates $count valid words', () async {
        final mnemonic = await GrinWalletClient.generateMnemonic(
          wordCount: count,
        );
        expect(mnemonic.split(' '), hasLength(count));
        final check = await GrinWalletClient.validateMnemonic(mnemonic);
        expect(check.valid, isTrue);
        expect(check.wordCount, count);
      });
    }

    test('defaults to 24 words', () async {
      final mnemonic = await GrinWalletClient.generateMnemonic();
      expect(mnemonic.split(' '), hasLength(24));
    });

    test('rejects an unsupported word count', () {
      expect(
        GrinWalletClient.generateMnemonic(wordCount: 13),
        throwsA(isA<GrinInvalidArgumentException>()),
      );
    });

    test('validates a known phrase', () async {
      final check = await GrinWalletClient.validateMnemonic(knownMnemonic);
      expect(check, const GrinMnemonicValidation(valid: true, wordCount: 12));
    });

    test('a bad checksum is a result, not an error', () async {
      final check = await GrinWalletClient.validateMnemonic(
        List.filled(12, 'abandon').join(' '),
      );
      expect(check.valid, isFalse);
      expect(check.wordCount, 12);
    });

    test('a word outside the list is invalid', () async {
      final check = await GrinWalletClient.validateMnemonic('not a phrase');
      expect(check.valid, isFalse);
      expect(check.wordCount, 3);
    });

    test('creating a wallet from a bad phrase fails', () async {
      final dir = await makeTempDir();
      expect(
        GrinWalletClient.create(
          config: testConfig(dir),
          mnemonic: List.filled(12, 'abandon').join(' '),
          password: 'pw',
        ),
        throwsA(isA<GrinInvalidMnemonicException>()),
      );
    });
  });

  group('address', () {
    late String address;
    late String onionUrl;

    setUp(() async {
      final dir = await makeTempDir();
      final wallet = await createTestWallet(dir);
      final info = await wallet.getAddress();
      address = info.slatepackAddress;
      onionUrl = info.onionV3Url;
    });

    test('a wallet on a test network has a tgrin1 address', () {
      expect(address, startsWith('tgrin1'));
      expect(onionUrl, matches(RegExp(r'^http://[a-z2-7]{56}\.onion$')));
    });

    test('accepts the slatepack address on a test network', () async {
      final result = await GrinWalletClient.validateAddress(
        address,
        GrinNetwork.automatedTesting,
      );
      expect(result.kind, GrinAddressKind.slatepack);
      expect(result.slatepackAddress, address);
      expect(result.onionV3Url, onionUrl);
      expect(result.url, onionUrl);
    });

    test('validateAddressSync gives the same answer on this isolate', () {
      final result = GrinWalletClient.validateAddressSync(
        address,
        GrinNetwork.automatedTesting,
      );
      expect(result.kind, GrinAddressKind.slatepack);
      expect(result.slatepackAddress, address);
      expect(result.url, onionUrl);
      expect(
        () =>
            GrinWalletClient.validateAddressSync(address, GrinNetwork.mainnet),
        throwsA(isA<GrinInvalidAddressException>()),
      );
      expect(
        () => GrinWalletClient.validateAddressSync('nope', GrinNetwork.mainnet),
        throwsA(isA<GrinInvalidAddressException>()),
      );
    });

    test('accepts the upper case form', () async {
      final result = await GrinWalletClient.validateAddress(
        address.toUpperCase(),
        GrinNetwork.testnet,
      );
      expect(result.slatepackAddress, address);
    });

    test('rejects a tgrin1 address on mainnet', () {
      expect(
        GrinWalletClient.validateAddress(address, GrinNetwork.mainnet),
        throwsA(isA<GrinInvalidAddressException>()),
      );
    });

    test('rejects a grin1 address on a test network', () async {
      final dir = await makeTempDir();
      final wallet = await createTestWallet(dir, network: GrinNetwork.mainnet);
      final mainnetAddress = (await wallet.getAddress()).slatepackAddress;
      expect(mainnetAddress, startsWith('grin1'));
      final ok = await GrinWalletClient.validateAddress(
        mainnetAddress,
        GrinNetwork.mainnet,
      );
      expect(ok.kind, GrinAddressKind.slatepack);
      expect(
        GrinWalletClient.validateAddress(mainnetAddress, GrinNetwork.testnet),
        throwsA(isA<GrinInvalidAddressException>()),
      );
    });

    test('accepts the onion forms and derives the slatepack address', () async {
      final label = onionUrl.substring('http://'.length).split('.').first;
      for (final form in [
        label,
        '$label.onion',
        'http://$label.onion',
        'http://$label.onion/',
        'https://$label.onion',
        'http://$label.onion/v2/foreign',
      ]) {
        final result = await GrinWalletClient.validateAddress(
          form,
          GrinNetwork.automatedTesting,
        );
        expect(result.kind, GrinAddressKind.onion, reason: form);
        expect(result.slatepackAddress, address, reason: form);
        expect(result.onionV3Url, onionUrl, reason: form);
        expect(result.url, endsWith('$label.onion'), reason: form);
      }
    });

    test('accepts an http URL', () async {
      final result = await GrinWalletClient.validateAddress(
        'https://wallet.example.com:3415/',
        GrinNetwork.mainnet,
      );
      expect(result.kind, GrinAddressKind.http);
      expect(result.url, 'https://wallet.example.com:3415');
      expect(result.slatepackAddress, isNull);
      expect(result.onionV3Url, isNull);
    });

    test('rejects text that is no address', () {
      for (final bad in ['', 'hello', 'grin1abc', 'ftp://example.com']) {
        expect(
          GrinWalletClient.validateAddress(bad, GrinNetwork.mainnet),
          throwsA(isA<GrinInvalidAddressException>()),
          reason: bad,
        );
      }
    });

    test('rejects a URL with credentials or a query', () {
      for (final bad in [
        'http://user:pw@example.com',
        'http://example.com/?a=b',
        'http://example.com/#frag',
      ]) {
        expect(
          GrinWalletClient.validateAddress(bad, GrinNetwork.mainnet),
          throwsA(isA<GrinInvalidAddressException>()),
          reason: bad,
        );
      }
    });
  });
}
