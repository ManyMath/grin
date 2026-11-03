import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:grin_native_example/src/lifecycle_observer.dart';
import 'package:grin_native_example/src/wallet_paths.dart';
import 'package:path/path.dart' as p;
import 'package:path_provider_platform_interface/path_provider_platform_interface.dart';
import 'package:plugin_platform_interface/plugin_platform_interface.dart';

class _FakePathProvider extends PathProviderPlatform
    with MockPlatformInterfaceMixin {
  _FakePathProvider(this.support);

  final String support;

  @override
  Future<String?> getApplicationSupportPath() async => support;
}

void main() {
  final binding = TestWidgetsFlutterBinding.ensureInitialized();

  // The binding keeps its state between tests, and an observer rejects
  // transitions the platform would never send, so every test starts from
  // resumed and walks the real sequence.
  void send(List<AppLifecycleState> states) {
    states.forEach(binding.handleAppLifecycleStateChanged);
  }

  void pause() => send([
    AppLifecycleState.inactive,
    AppLifecycleState.hidden,
    AppLifecycleState.paused,
  ]);

  group('GrinLifecycleObserver', () {
    setUp(() => send([AppLifecycleState.resumed]));

    test('cancels on paused and detached only', () {
      var calls = 0;
      final observer = GrinLifecycleObserver.custom(cancelScan: () => calls++);
      addTearDown(observer.dispose);

      send([AppLifecycleState.inactive, AppLifecycleState.resumed]);
      expect(calls, 0);
      expect(observer.lastState, AppLifecycleState.resumed);

      send([AppLifecycleState.inactive, AppLifecycleState.hidden]);
      expect(calls, 0);
      send([AppLifecycleState.paused]);
      expect(calls, 1);
      expect(observer.lastState, AppLifecycleState.paused);

      send([AppLifecycleState.detached]);
      expect(calls, 2);
      expect(observer.lastState, AppLifecycleState.detached);
    });

    test('honours the flags', () {
      var pauseOnly = 0;
      final a = GrinLifecycleObserver.custom(
        cancelScan: () => pauseOnly++,
        cancelScanOnDetach: false,
      );
      addTearDown(a.dispose);
      var detachOnly = 0;
      final b = GrinLifecycleObserver.custom(
        cancelScan: () => detachOnly++,
        cancelScanOnPause: false,
      );
      addTearDown(b.dispose);

      pause();
      expect([pauseOnly, detachOnly], [1, 0]);
      send([AppLifecycleState.detached]);
      expect([pauseOnly, detachOnly], [1, 1]);
    });

    test('does nothing after dispose', () {
      var calls = 0;
      final observer = GrinLifecycleObserver.custom(cancelScan: () => calls++);
      observer.dispose();
      observer.dispose();
      pause();
      expect(calls, 0);
      expect(observer.lastState, isNull);
      observer.start();
      send([AppLifecycleState.detached]);
      expect(calls, 0);
    });

    test('autoStart false waits for start', () {
      var calls = 0;
      final observer = GrinLifecycleObserver.custom(
        cancelScan: () => calls++,
        autoStart: false,
      );
      addTearDown(observer.dispose);
      pause();
      expect(calls, 0);
      send([AppLifecycleState.resumed]);
      observer.start();
      pause();
      expect(calls, 1);
    });

    test('ignores StateError and reports other errors', () {
      final errors = <Object>[];
      var mode = 0;
      final observer = GrinLifecycleObserver.custom(
        cancelScan: () {
          if (mode == 0) throw StateError('closed');
          throw const FormatException('boom');
        },
        onError: errors.add,
      );
      addTearDown(observer.dispose);

      pause();
      expect(errors, isEmpty);

      send([AppLifecycleState.resumed]);
      mode = 1;
      pause();
      expect(errors, hasLength(1));
      expect(errors.single, isA<FormatException>());
    });
  });

  group('WalletPaths.defaultWalletDirectory', () {
    late Directory tmp;
    late PathProviderPlatform previous;

    setUp(() {
      previous = PathProviderPlatform.instance;
      tmp = Directory.systemTemp.createTempSync('grin_example_test');
      PathProviderPlatform.instance = _FakePathProvider(tmp.path);
    });

    tearDown(() {
      PathProviderPlatform.instance = previous;
      tmp.deleteSync(recursive: true);
    });

    test('returns <support>/grin/<id> and creates it', () async {
      final dir = await WalletPaths.defaultWalletDirectory('abc123');
      expect(dir, p.join(tmp.path, 'grin', 'abc123'));
      expect(Directory(dir).existsSync(), isTrue);
      expect(await WalletPaths.defaultWalletDirectory('abc123'), dir);
      expect((await WalletPaths.walletsRoot()).path, p.join(tmp.path, 'grin'));
    });

    test('rejects bad ids', () async {
      for (final id in ['', '.', '..', 'a/b', r'a\b', '../x', 'a..b']) {
        await expectLater(
          WalletPaths.defaultWalletDirectory(id),
          throwsArgumentError,
          reason: 'id "$id"',
        );
      }
      expect(Directory(p.join(tmp.path, 'grin')).existsSync(), isFalse);
    });
  });
}
