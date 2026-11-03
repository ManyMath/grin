import 'package:grin_native/grin_native.dart';
import 'package:test/test.dart';

import 'support.dart';

// GrinNativeBackend only delegates to GrinWalletClient's statics; these
// tests call every method once through the GrinWalletBackend type.
void main() {
  const GrinWalletBackend backend = GrinNativeBackend();

  test('mnemonic and address checks', () async {
    final phrase = await backend.generateMnemonic(wordCount: 12);
    expect(phrase.split(' '), hasLength(12));
    final check = await backend.validateMnemonic(phrase);
    expect(check.valid, isTrue);
    expect(check.wordCount, 12);
    expect((await backend.validateMnemonic('abandon')).valid, isFalse);

    final dir = await makeTempDir();
    final wallet = await createTestWallet(dir, network: GrinNetwork.testnet);
    final address = (await wallet.getAddress()).slatepackAddress;
    final validation = await backend.validateAddress(
      address,
      GrinNetwork.testnet,
    );
    expect(validation.slatepackAddress, address);
    expect(
      backend.validateAddressSync(address, GrinNetwork.testnet).kind,
      validation.kind,
    );
  });

  test('create, open and delete', () async {
    final dir = await makeTempDir();
    final config = testConfig(dir);

    final created = await backend.create(
      config: config,
      mnemonic: knownMnemonic,
      password: 'pw',
    );
    expect(created, isA<GrinWalletClient>());
    final address = await created.getAddress();
    await created.close();

    final opened = await backend.open(config: config, password: 'pw');
    expect(await opened.getAddress(), address);
    await opened.close();

    await backend.delete(walletDir: dir.path);
    await expectLater(
      backend.open(config: config, password: 'pw'),
      throwsA(isA<GrinWalletNotFoundException>()),
    );
  });

  test('wallet-less node and Slatepack calls', () async {
    await expectLater(
      backend.testNodeConnection(nodeUrl: 'http://127.0.0.1:1'),
      throwsA(isA<GrinNodeException>()),
    );
    await expectLater(
      backend.decodeSlatepackWithoutWallet(
        'not a slatepack',
        GrinNetwork.testnet,
      ),
      throwsA(isA<GrinSlateException>()),
    );
  });
}
