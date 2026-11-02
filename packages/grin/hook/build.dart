import 'dart:io';

import 'package:code_assets/code_assets.dart';
import 'package:native_prebuilt_hook/native_prebuilt_hook.dart';
import 'package:native_toolchain_rust/native_toolchain_rust.dart';

/// Obtains libgrin_ffi in the mode chosen by the `build` user-define: the
/// pinned prebuilt (default once a release exists), a verified Nix source
/// build, an unverified Nix dev build, or an unverified cargo build with the
/// host Rust toolchain. See "Native library" in the README.
Future<void> main(List<String> args) => runNativeBuildHook(
  args,
  const NativeLibrarySpec(
    packageName: 'grin',
    libraryBaseName: 'grin_ffi',
    // lib/src/ffi/bindings.g.dart binds its @Native functions to this asset.
    bindingsAssetName: 'src/ffi/bindings.g.dart',
    // Read by nix/build-apple.sh; the hook sets it, users do not.
    appleAnyXcodeVariable: 'GRIN_FFI_ANY_XCODE',
    // Rerun source and dev builds when what the Nix build reads changes.
    nativeSources: [
      'rust',
      'flake.nix',
      'flake.lock',
      'nix',
      'build-config.json',
    ],
    cargoBuild: _cargoBuild,
  ),
);

/// `build: cargo`, and the fallback where no prebuilt exists and Nix cannot
/// build the target (Windows hosts, Intel Macs, Windows on arm64).
///
/// The `test_utils` user-define (bool, default false) adds
/// `--features test-utils`, which exports the grin_test_* functions the Dart
/// tests drive an in-process chain with. Only this mode reads it: the
/// prebuilt, source and dev libraries are release builds by definition.
Future<void> _cargoBuild(BuildInput input, BuildOutputBuilder output) async {
  final config = input.config.code;
  final target = rustTarget(config);
  final variable = target.replaceAll('-', '_');
  final linker = 'CARGO_TARGET_${variable.toUpperCase()}_LINKER';
  final testUtils = parseBoolUserDefine(
    'test_utils',
    input.userDefines['test_utils'],
  );
  final environment = <String, String>{
    if (config.targetOS == OS.macOS)
      'MACOSX_DEPLOYMENT_TARGET': '${config.macOS.targetVersion}.0',
    if (config.targetOS == OS.iOS)
      'IPHONEOS_DEPLOYMENT_TARGET': '${config.iOS.targetVersion}.0',
  };
  final compiler = config.cCompiler;
  if (config.targetOS == OS.android && compiler != null) {
    // RustBuilder links with the NDK's newest API clang; link for the app's
    // minSdk instead, so no symbol newer than the floor is pulled in.
    final ndkTarget = target.replaceFirst('armv7-', 'armv7a-');
    final api = config.android.targetNdkApi;
    final suffix = Platform.isWindows ? '.cmd' : '';
    final cc = compiler.compiler
        .resolve('$ndkTarget$api-clang$suffix')
        .toFilePath();
    environment.addAll({
      'CC_$variable': cc,
      'CXX_$variable': compiler.compiler
          .resolve('$ndkTarget$api-clang++$suffix')
          .toFilePath(),
      linker: cc,
    });
  } else if (config.targetOS == OS.linux && compiler != null) {
    environment.addAll({
      'CC_$variable': compiler.compiler.toFilePath(),
      'AR_$variable': compiler.archiver.toFilePath(),
      linker: compiler.compiler.toFilePath(),
    });
  }

  await RustBuilder(
    assetName: 'src/ffi/bindings.g.dart',
    cratePath: 'rust',
    extraCargoBuildArgs: [
      '--locked',
      '--lib',
      if (testUtils) ...['--features', 'test-utils'],
    ],
    extraCargoEnvironmentVariables: environment,
  ).run(input: input, output: output);

  // Cargo's dep-info lists compiled sources only; also rerun when a
  // manifest, the lock file or a newly added file in the workspace changes.
  output.dependencies.addAll(
    nativeSourceDependencies(input.packageRoot, const ['rust']),
  );
}
