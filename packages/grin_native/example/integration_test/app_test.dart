import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:grin_native_example/main.dart';
import 'package:integration_test/integration_test.dart';

String _textOf(WidgetTester tester, Key key) =>
    tester.widget<SelectableText>(find.byKey(key)).data ?? '';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('creates a wallet through the native library', (tester) async {
    await tester.pumpWidget(const ExampleApp());

    // Native calls run in other isolates, so poll real time, not fake time.
    final deadline = DateTime.now().add(const Duration(seconds: 60));
    while (!_textOf(tester, const ValueKey('address')).startsWith('tgrin1')) {
      expect(
        DateTime.now().isBefore(deadline),
        isTrue,
        reason: 'no address after 60 s; error: ${_errorText(tester)}',
      );
      await tester.pump(const Duration(milliseconds: 250));
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 250)),
      );
    }

    expect(_textOf(tester, const ValueKey('mnemonic')), contains('24 words'));

    await tester.tap(find.byKey(const ValueKey('balance-button')));
    for (var i = 0; i < 240; i++) {
      await tester.pump(const Duration(milliseconds: 250));
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 250)),
      );
      if (find.textContaining('Spendable:').evaluate().isNotEmpty) break;
    }
    expect(find.textContaining('Spendable:'), findsOneWidget);
  });
}

String _errorText(WidgetTester tester) {
  final finder = find.byKey(const ValueKey('error'));
  return finder.evaluate().isEmpty
      ? 'none'
      : tester.widget<Text>(finder).data ?? 'none';
}
