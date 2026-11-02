// Runs every gate of docs/architecture.md sections 7 and 8 in one command.
//
//   dart run tool/ci.dart                 # everything that runs on this host
//   dart run tool/ci.dart --list          # print the stages
//   dart run tool/ci.dart --only rust-test,dart-test
//   dart run tool/ci.dart --skip nix,android,flutter-build
//   dart run tool/ci.dart --local-inputs  # Nix with the unpushed kit and fork
//
// Stages, in order:
//   rust-dups        cargo tree -d shows no duplicated grin or secp crates
//   rust-fmt         cargo fmt --check
//   rust-clippy      clippy with and without test-utils, -D warnings
//   rust-test        cargo test --features test-utils (includes the C11/C++11
//                    header test and the end-to-end chain tests)
//   rust-release     host release build; nm: 35 grin_ symbols, 0 grin_test_
//   android          release builds for aarch64, armv7 and x86_64 Android with
//                    NDK 28.2 through cargo-ndk; nm on each
//   bindings         tool/regen_bindings.dart --check (header and ffigen output
//                    are up to date)
//   dart-analyze     dart analyze --fatal-infos in packages/grin
//   dart-format      dart format --set-exit-if-changed on the Dart sources
//   dart-test        dart test in packages/grin (test_utils on, cargo mode)
//   flutter-analyze  flutter analyze in packages/grin_flutter and its example
//   flutter-test     flutter test in packages/grin_flutter
//   flutter-build    flutter build linux and apk (arm64) of the example
//   nix              the flake's release artifacts for every target this host
//                    can build, the nm gate on each, and a candidate
//                    prebuilt_hashes.json written under build/nix
//
// The nm gate reads the dynamic symbol table (llvm-nm -D) for ELF files,
// because the reference build strips the static table, and the COFF export
// table (llvm-readobj --coff-exports) for a DLL.
import 'dart:convert';
import 'dart:io';

const _ndkVersion = '28.2.13676358';
const _ndkHome = '/home/user/Android/Sdk/ndk/$_ndkVersion';
const _standardSymbolCount = 35;

// Local wiring until the kit and the fork are pushed (docs/architecture.md
// D2 and README). --local-inputs passes these to nix.
const _localKit =
    'git+file:///home/user/src/stack_wallet/wt/native-build-kit'
    '?ref=feat/native-build-kit&rev=85f7eb097604e5b99db3f90bccbcada3336836b8';
const _localFork =
    'git+file:///home/user/src/grin-wallet'
    '?ref=optional-tor-rustls&rev=64d361f2e49fa6d544cf0236417da48a99444b2f'
    '&submodules=1';

const _androidTargets = {
  'aarch64-linux-android': 'arm64-v8a',
  'armv7-linux-androideabi': 'armeabi-v7a',
  'x86_64-linux-android': 'x86_64',
};

// What the kit's mkCargoLibrary builds on an x86_64-linux host as pure
// derivations (Apple targets need a macOS host and Xcode).
const _nixTargets = [
  'x86_64-unknown-linux-gnu',
  'aarch64-unknown-linux-gnu',
  'aarch64-linux-android',
  'armv7-linux-androideabi',
  'x86_64-linux-android',
  'x86_64-pc-windows-msvc',
];

late final String root;
String get rust => '$root/packages/grin/rust';
String get grin => '$root/packages/grin';
String get grinFlutter => '$root/packages/grin_flutter';
String get example => '$root/packages/grin_flutter/example';

final List<String> _failures = [];
final List<String> _passed = [];

Future<void> main(List<String> args) async {
  root = _repoRoot();
  final only = _listArg(args, '--only');
  final skip = _listArg(args, '--skip');
  final localInputs = args.contains('--local-inputs');

  final stages = <String, Future<void> Function()>{
    'rust-dups': rustDups,
    'rust-fmt': rustFmt,
    'rust-clippy': rustClippy,
    'rust-test': rustTest,
    'rust-release': rustRelease,
    'android': android,
    'bindings': bindings,
    'dart-analyze': dartAnalyze,
    'dart-format': dartFormat,
    'dart-test': dartTest,
    'flutter-analyze': flutterAnalyze,
    'flutter-test': flutterTest,
    'flutter-build': flutterBuild,
    'nix': () => nix(localInputs: localInputs),
  };
  if (args.contains('--list')) {
    stages.keys.forEach(stdout.writeln);
    return;
  }
  for (final name in [...only, ...skip]) {
    if (!stages.containsKey(name)) {
      stderr.writeln('unknown stage: $name');
      exit(2);
    }
  }
  for (final entry in stages.entries) {
    if (only.isNotEmpty && !only.contains(entry.key)) continue;
    if (skip.contains(entry.key)) continue;
    stdout.writeln('\n==> ${entry.key}');
    try {
      await entry.value();
      _passed.add(entry.key);
      stdout.writeln('==> ${entry.key}: ok');
    } on _GateFailure catch (e) {
      _failures.add('${entry.key}: ${e.message}');
      stdout.writeln('==> ${entry.key}: FAILED (${e.message})');
    }
  }
  stdout.writeln('\nSummary');
  for (final p in _passed) {
    stdout.writeln('  ok      $p');
  }
  for (final f in _failures) {
    stdout.writeln('  FAILED  $f');
  }
  exit(_failures.isEmpty ? 0 : 1);
}

// ---------------------------------------------------------------- stages

Future<void> rustDups() async {
  final out = await _capture('cargo', [
    'tree',
    '-d',
    '--edges',
    'normal',
  ], rust);
  final roots = out
      .split('\n')
      .where((l) => RegExp(r'^[A-Za-z0-9_-]+ v').hasMatch(l))
      .where((l) => RegExp('grin|secp', caseSensitive: false).hasMatch(l))
      .toList();
  if (roots.isNotEmpty) {
    throw _GateFailure('duplicated crates: ${roots.join(', ')}');
  }
  stdout.writeln('no duplicated grin or secp crates');
}

Future<void> rustFmt() =>
    _run('cargo', ['fmt', '-p', 'grin-ffi', '--', '--check'], rust);

Future<void> rustClippy() async {
  await _run('cargo', [
    'clippy',
    '-p',
    'grin-ffi',
    '--all-targets',
    '--features',
    'test-utils',
    '--',
    '-D',
    'warnings',
  ], rust);
  await _run('cargo', [
    'clippy',
    '-p',
    'grin-ffi',
    '--',
    '-D',
    'warnings',
  ], rust);
}

Future<void> rustTest() => _run('cargo', [
  'test',
  '-p',
  'grin-ffi',
  '--features',
  'test-utils',
  '--locked',
], rust);

Future<void> rustRelease() async {
  await _run('cargo', [
    'build',
    '-p',
    'grin-ffi',
    '--release',
    '--locked',
  ], rust);
  await _nmGate('$rust/target/release/libgrin_ffi.so');
}

Future<void> android() async {
  if (!Directory(_ndkHome).existsSync()) {
    throw _GateFailure('NDK $_ndkVersion not found at $_ndkHome');
  }
  for (final entry in _androidTargets.entries) {
    await _run(
      'cargo',
      [
        'ndk',
        '--target',
        entry.key,
        '--platform',
        '24',
        'build',
        '-p',
        'grin-ffi',
        '--release',
        '--locked',
      ],
      rust,
      environment: {'ANDROID_NDK_HOME': _ndkHome, 'ANDROID_NDK_ROOT': _ndkHome},
    );
    await _nmGate('$rust/target/${entry.key}/release/libgrin_ffi.so');
  }
}

Future<void> bindings() =>
    _run('dart', ['run', 'tool/regen_bindings.dart', '--check'], root);

Future<void> dartAnalyze() => _run('dart', ['analyze', '--fatal-infos'], grin);

Future<void> dartFormat() => _run('dart', [
  'format',
  '--set-exit-if-changed',
  'packages/grin',
  'packages/grin_flutter/lib',
  'packages/grin_flutter/test',
  'packages/grin_flutter/example/lib',
  'packages/grin_flutter/example/integration_test',
  'tool',
], root);

Future<void> dartTest() => _run('dart', ['test'], grin);

Future<void> flutterAnalyze() async {
  await _run('flutter', ['analyze'], grinFlutter);
  await _run('flutter', ['analyze'], example);
}

Future<void> flutterTest() => _run('flutter', ['test'], grinFlutter);

Future<void> flutterBuild() async {
  await _run('flutter', ['build', 'linux'], example);
  await _run('flutter', [
    'build',
    'apk',
    '--target-platform',
    'android-arm64',
  ], example);
}

Future<void> nix({required bool localInputs}) async {
  final overrides = localInputs
      ? [
          '--override-input',
          'kit',
          _localKit,
          '--override-input',
          'grin-wallet',
          _localFork,
          '--no-write-lock-file',
        ]
      : const <String>[];
  final flake = 'git+file://$root?dir=packages/grin';
  final outDir = Directory('$root/build/nix')..createSync(recursive: true);
  final files = <String, String>{};
  for (final target in _nixTargets) {
    final printed = await _capture(
      'nix',
      [
        'build',
        '$flake#$target',
        '--no-link',
        '--print-out-paths',
        ...overrides,
      ],
      root,
      environment: {
        'NIX_CONFIG': 'extra-experimental-features = nix-command flakes',
      },
    );
    final out = printed.trim().split('\n').first;
    final lib = Directory('$out/lib').listSync().whereType<File>().toList();
    if (lib.isEmpty) throw _GateFailure('$target: no library in $out/lib');
    for (final f in lib) {
      final name = f.uri.pathSegments.last;
      await _nmGate(f.path);
      final digest = await _sha256(f.path);
      files['$target/$name'] = digest;
      stdout.writeln('$target/$name $digest');
    }
  }
  // The same shape as hook/prebuilt_hashes.json, without a release to point
  // at. A release workflow fills nativeVersion and releaseUrl.
  final candidate = const JsonEncoder.withIndent('  ')
      .convert({'nativeVersion': null, 'releaseUrl': null, 'files': files});
  File('${outDir.path}/prebuilt_hashes.candidate.json')
      .writeAsStringSync('$candidate\n');
  stdout.writeln('wrote ${outDir.path}/prebuilt_hashes.candidate.json');
}

// ---------------------------------------------------------------- helpers

Future<void> _nmGate(String path) async {
  final file = File(path);
  if (!file.existsSync()) throw _GateFailure('missing $path');
  final List<String> symbols;
  if (path.endsWith('.dll')) {
    final out = await _capture('llvm-readobj', ['--coff-exports', path], root);
    symbols = RegExp(r'Name: (grin_\w+)')
        .allMatches(out)
        .map((m) => m.group(1)!)
        .toList();
  } else {
    final out = await _capture('llvm-nm', ['-D', '--defined-only', path], root);
    symbols = out
        .split('\n')
        .map((l) => l.trim().split(RegExp(r'\s+')))
        .where((p) => p.length >= 3 && p.last.startsWith('grin_'))
        .map((p) => p.last)
        .toList();
  }
  final test = symbols.where((s) => s.startsWith('grin_test_')).toList();
  final standard = symbols.where((s) => !s.startsWith('grin_test_')).toSet();
  stdout.writeln(
    'nm ${file.uri.pathSegments.last}: ${standard.length} standard, '
    '${test.length} test symbols',
  );
  if (test.isNotEmpty) {
    throw _GateFailure('$path exports test symbols: ${test.join(', ')}');
  }
  if (standard.length != _standardSymbolCount) {
    throw _GateFailure(
      '$path exports ${standard.length} grin_ symbols, expected '
      '$_standardSymbolCount',
    );
  }
}

Future<String> _sha256(String path) async {
  final out = await _capture('sha256sum', [path], root);
  return out.trim().split(RegExp(r'\s+')).first;
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
  if (code != 0) throw _GateFailure('$exe exited with $code');
}

Future<String> _capture(
  String exe,
  List<String> args,
  String cwd, {
  Map<String, String> environment = const {},
}) async {
  stdout.writeln('\$ (cd $cwd && $exe ${args.join(' ')})');
  final result = await Process.run(
    exe,
    args,
    workingDirectory: cwd,
    environment: environment,
  );
  if (result.exitCode != 0) {
    stderr.write(result.stderr);
    throw _GateFailure('$exe exited with ${result.exitCode}');
  }
  return result.stdout as String;
}

List<String> _listArg(List<String> args, String name) {
  final values = <String>[];
  for (var i = 0; i < args.length; i++) {
    if (args[i] == name && i + 1 < args.length) {
      values.addAll(args[i + 1].split(','));
    } else if (args[i].startsWith('$name=')) {
      values.addAll(args[i].substring(name.length + 1).split(','));
    }
  }
  return values.where((v) => v.isNotEmpty).toList();
}

String _repoRoot() {
  var dir = Directory.current;
  while (!Directory('${dir.path}/packages/grin').existsSync() ||
      !File('${dir.path}/pubspec.yaml').existsSync()) {
    final parent = dir.parent;
    if (parent.path == dir.path) {
      stderr.writeln('run from inside the grin repository');
      exit(2);
    }
    dir = parent;
  }
  return dir.path;
}

class _GateFailure implements Exception {
  _GateFailure(this.message);
  final String message;
}
