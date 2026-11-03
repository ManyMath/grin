import 'package:flutter/material.dart';
import 'package:grin_native/grin_native.dart';

import 'src/lifecycle_observer.dart';
import 'src/wallet_paths.dart';

void main() => runApp(const ExampleApp());

/// Creates or opens a testnet wallet and shows what the native library
/// returns. No node is needed: nothing here contacts one.
class ExampleApp extends StatelessWidget {
  const ExampleApp({super.key});

  @override
  Widget build(BuildContext context) => MaterialApp(
    title: 'Grin example',
    theme: ThemeData(colorSchemeSeed: Colors.amber),
    home: const WalletPage(),
  );
}

class WalletPage extends StatefulWidget {
  const WalletPage({super.key});

  @override
  State<WalletPage> createState() => _WalletPageState();
}

class _WalletPageState extends State<WalletPage> {
  static const _password = 'example';

  GrinWalletClient? _client;
  GrinLifecycleObserver? _observer;
  String _mnemonic = 'starting...';
  String _address = '';
  String _onion = '';
  String _balance = '';
  String? _error;

  @override
  void initState() {
    super.initState();
    _start();
  }

  Future<void> _start() async {
    try {
      GrinWalletClient.initLogs();
      final dir = await WalletPaths.defaultWalletDirectory('example');
      final config = GrinWalletConfig(
        walletDir: dir,
        network: GrinNetwork.testnet,
        nodeUrl: 'http://127.0.0.1:13413',
      );
      GrinWalletClient client;
      try {
        client = await GrinWalletClient.open(
          config: config,
          password: _password,
        );
      } on GrinWalletNotFoundException {
        client = await GrinWalletClient.create(
          config: config,
          mnemonic: await GrinWalletClient.generateMnemonic(wordCount: 24),
          password: _password,
        );
      }
      if (!mounted) {
        await client.close();
        return;
      }
      _client = client;
      _observer = GrinLifecycleObserver(
        wallet: client,
        onError: (e) => debugPrint('cancelScan failed: $e'),
      );
      final words = (await client.getMnemonic(_password)).split(' ');
      final address = await client.getAddress();
      if (!mounted) return;
      setState(() {
        _mnemonic = '${words.take(3).join(' ')} ... (${words.length} words)';
        _address = address.slatepackAddress;
        _onion = address.onionV3Url;
      });
    } catch (e) {
      if (mounted) setState(() => _error = 'Start failed: $e');
    }
  }

  Future<void> _showBalance() async {
    try {
      final balance = await _client!.getBalance(
        refreshFromNode: false,
        minimumConfirmations: 1,
      );
      if (mounted) {
        setState(() {
          _balance = 'Spendable: ${formatGrin(balance.spendable)} GRIN';
          _error = null;
        });
      }
    } catch (e) {
      if (mounted) setState(() => _error = 'Balance failed: $e');
    }
  }

  @override
  void dispose() {
    _observer?.dispose();
    _client?.close();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: const Text('Grin example')),
    body: ListView(
      padding: const EdgeInsets.all(16),
      children: [
        const Text('Mnemonic (first 3 words)'),
        SelectableText(_mnemonic, key: const ValueKey('mnemonic')),
        const SizedBox(height: 16),
        const Text('Slatepack address'),
        SelectableText(_address, key: const ValueKey('address')),
        const SizedBox(height: 16),
        const Text('Onion URL'),
        SelectableText(_onion, key: const ValueKey('onion')),
        const SizedBox(height: 16),
        Align(
          alignment: Alignment.centerLeft,
          child: FilledButton(
            key: const ValueKey('balance-button'),
            onPressed: _client == null ? null : _showBalance,
            child: const Text('Balance (local)'),
          ),
        ),
        Text(_balance, key: const ValueKey('balance')),
        if (_error != null)
          Text(
            _error!,
            key: const ValueKey('error'),
            style: TextStyle(color: Theme.of(context).colorScheme.error),
          ),
      ],
    ),
  );
}
