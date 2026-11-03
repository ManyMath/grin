import 'dart:ffi';

import 'package:ffi/ffi.dart';
import 'package:grin_native/grin_native.dart';
import 'package:grin_native/src/ffi/bindings.g.dart' as bindings;
import 'package:grin_native/src/ffi/memory.dart';
import 'package:test/test.dart';

/// Hands out malloc memory and records what each block held when it was
/// freed.
class _RecordingAllocator implements Allocator {
  final List<List<int>> freedContents = [];

  @override
  Pointer<T> allocate<T extends NativeType>(int byteCount, {int? alignment}) =>
      malloc.allocate<T>(byteCount, alignment: alignment);

  @override
  void free(Pointer<NativeType> pointer) {
    // The allocator under test passes the block on after zeroing it; the
    // size is not known here, so look at the first 64 bytes of a block that
    // the test made large enough.
    freedContents.add(List<int>.of(pointer.cast<Uint8>().asTypedList(64)));
    malloc.free(pointer);
  }
}

void main() {
  test('ZeroingAllocator clears a block before it is freed', () {
    final recorder = _RecordingAllocator();
    final allocator = ZeroingAllocator(recorder);
    final pointer = allocator.allocate<Uint8>(64);
    pointer.asTypedList(64).fillRange(0, 64, 0xAB);
    expect(allocator.liveBlocks, 1);
    allocator.free(pointer);
    expect(allocator.liveBlocks, 0);
    expect(recorder.freedContents.single, everyElement(0));
  });

  test('usingSecretArena returns the value and frees on a throw', () {
    expect(
      usingSecretArena(
        (arena) => toCStringSecret('abc', arena).cast<Utf8>().toDartString(),
      ),
      'abc',
    );
    expect(
      () => usingSecretArena<void>((arena) {
        toCStringSecret('abc', arena);
        throw StateError('boom');
      }),
      throwsStateError,
    );
  });

  test('toCStringSecret copies multi-byte text with a terminator', () {
    usingSecretArena((arena) {
      final pointer = toCStringSecret('pässwörd', arena);
      expect(pointer.cast<Utf8>().toDartString(), 'pässwörd');
      expect(toCStringSecretOrNull(null, arena), nullptr);
    });
  });

  test('unwrapFfiResult returns data and frees the result', () {
    final text = usingSecretArena((arena) {
      return unwrapFfiResult(
        bindings.grin_mnemonic_validate(toCStringSecret('one two', arena)),
      );
    });
    expect(text, contains('"valid":false'));
  });

  test('unwrapFfiResult throws the mapped exception', () {
    expect(
      () => unwrapFfiResult(bindings.grin_mnemonic_generate(7)),
      throwsA(isA<GrinInvalidArgumentException>()),
    );
  });

  test('unwrapFfiResult returns null when there is nothing to return', () {
    final result = usingSecretArena((arena) {
      return unwrapFfiResult(bindings.grin_init_logs(toCString('info', arena)));
    });
    expect(result, isNull);
  });

  group('a string with U+0000', () {
    test('is refused by toCString and toCStringOrNull', () {
      final arena = Arena();
      addTearDown(arena.releaseAll);
      expect(() => toCString('a\u0000b', arena), throwsArgumentError);
      expect(() => toCStringOrNull('\u0000', arena), throwsArgumentError);
      expect(toCStringOrNull(null, arena), nullptr);
      expect(toCString('ab', arena).cast<Utf8>().toDartString(), 'ab');
    });

    test('is refused by the secret variants without leaking the value', () {
      usingSecretArena((arena) {
        expect(
          () => toCStringSecret('hunter2\u0000x', arena),
          throwsA(
            isA<ArgumentError>().having(
              (e) => '${e.message}',
              'message',
              isNot(contains('hunter2')),
            ),
          ),
        );
        expect(
          () => toCStringSecretOrNull('hunter2\u0000x', arena),
          throwsArgumentError,
        );
        expect(toCStringSecretOrNull(null, arena), nullptr);
      });
    });

    test('is refused by the public API before it reaches the library', () {
      expect(
        GrinWalletClient.validateMnemonic('abandon\u0000about'),
        throwsArgumentError,
      );
    });
  });
}
