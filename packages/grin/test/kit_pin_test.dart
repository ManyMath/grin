// The kit comes in twice: flake.lock pins the Nix library, pubspec.yaml the
// hook package. Both must name the same commit, or the hook of one kit
// revision runs against the flake of another. The grin-wallet fork comes in
// twice as well: flake.lock pins the flake input, rust/Cargo.toml the cargo
// dependency.
library;

import 'dart:convert';
import 'dart:io';

import 'package:test/test.dart';

void main() {
  final lock = jsonDecode(File('flake.lock').readAsStringSync()) as Map;
  final nodes = lock['nodes'] as Map;

  test(
    'pubspec.yaml pins native_prebuilt_hook to the flake.lock kit commit',
    () {
      final kit = (nodes['kit'] as Map)['locked'] as Map;
      final pubspec = File('pubspec.yaml').readAsStringSync();
      final dep = RegExp(
        r'^  native_prebuilt_hook:\n    git:\n((?:      .*\n)+)',
        multiLine: true,
      ).firstMatch(pubspec);
      expect(
        dep,
        isNotNull,
        reason: 'native_prebuilt_hook must be a git dependency',
      );
      String field(String name) => RegExp(
        '^      $name: (.*)\$',
        multiLine: true,
      ).firstMatch(dep!.group(1)!)!.group(1)!.trim();

      expect(field('ref'), kit['rev']);
      expect(field('url'), kit['url']);
      expect(field('path'), 'dart/native_prebuilt_hook');
    },
  );

  test('rust/Cargo.toml pins grin-wallet to the flake.lock input commit', () {
    final fork = (nodes['grin-wallet'] as Map)['locked'] as Map;
    final cargo = File('rust/Cargo.toml').readAsStringSync();
    final revs = RegExp(
      r'git = "ssh://git@git\.cypherstack\.com:222/sneurlax/grin-wallet\.git", rev = "([0-9a-f]+)"',
    ).allMatches(cargo).map((m) => m.group(1)).toSet();
    expect(
      revs,
      hasLength(1),
      reason: 'every fork crate must use the same rev',
    );
    expect(revs.single, fork['rev']);
    expect(
      fork['url'],
      'ssh://git@git.cypherstack.com:222/sneurlax/grin-wallet.git',
    );
  });
}
