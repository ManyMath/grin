// Regenerates the C header and the Dart FFI bindings.
//
//   dart run tool/regen_bindings.dart          # regenerate
//   dart run tool/regen_bindings.dart --check  # fail if anything would change
//
// Steps: cargo build -p grin-ffi (build.rs runs cbindgen and refreshes
// packages/grin_native/rust/include/grin_ffi.h), then ffigen in
// packages/grin_native, then dart format on the generated file. --check runs the
// same and fails when git reports a diff in either generated file.
import 'dart:io';

import 'package:path/path.dart' as p;

const _header = 'packages/grin_native/rust/include/grin_ffi.h';
const _bindings = 'packages/grin_native/lib/src/ffi/bindings.g.dart';

Future<void> main(List<String> args) async {
  final check = args.contains('--check');
  final root = _repoRoot();
  final rust = p.join(root, 'packages', 'grin_native', 'rust');
  final grinNative = p.join(root, 'packages', 'grin_native');

  // build.rs only refreshes the committed header when asked, so a plain
  // cargo build never rewrites a checkout; tests/header_test.rs compares
  // the committed file with the freshly generated one.
  await _run(
    'cargo',
    ['build', '-p', 'grin-ffi', '--locked'],
    rust,
    environment: {'GRIN_FFI_WRITE_HEADER': '1'},
  );
  await _run('dart', ['run', 'ffigen', '--config', 'ffigen.yaml'], grinNative);
  await _run('dart', ['format', p.join(root, _bindings)], root);

  if (check) {
    final diff = await Process.run('git', [
      'status',
      '--porcelain',
      '--',
      _header,
      _bindings,
    ], workingDirectory: root);
    final out = (diff.stdout as String).trim();
    if (out.isNotEmpty) {
      stderr.writeln('regen_bindings --check: generated files differ:\n$out');
      exit(1);
    }
    stdout.writeln('regen_bindings --check: no diff');
  }
}

String _repoRoot() {
  var dir = Directory.current;
  while (!File(p.join(dir.path, 'pubspec.yaml')).existsSync() ||
      !Directory(p.join(dir.path, 'packages', 'grin')).existsSync()) {
    final parent = dir.parent;
    if (parent.path == dir.path) {
      stderr.writeln('run from inside the grin repository');
      exit(2);
    }
    dir = parent;
  }
  return dir.path;
}

Future<void> _run(
  String exe,
  List<String> args,
  String cwd, {
  Map<String, String> environment = const {},
}) async {
  stdout.writeln('\$ (cd $cwd && $exe ${args.join(' ')})');
  final process = await Process.start(
    exe,
    args,
    workingDirectory: cwd,
    environment: environment,
    mode: ProcessStartMode.inheritStdio,
  );
  final code = await process.exitCode;
  if (code != 0) {
    stderr.writeln('$exe exited with $code');
    exit(code);
  }
}
