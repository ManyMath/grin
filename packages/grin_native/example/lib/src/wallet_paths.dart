import 'dart:io';

import 'package:path/path.dart' as p;
import 'package:path_provider/path_provider.dart';

/// Where this app keeps its wallets. Copy it into your app, or use your own
/// layout: grin_native takes any directory as `GrinWalletConfig.walletDir`.
abstract final class WalletPaths {
  /// Returns the directory that holds all Grin wallets of this app:
  /// `<application support>/grin`. It is created if it is missing.
  ///
  /// The application support directory is used on purpose. A wallet database
  /// is not a user document, and on iOS and macOS the documents directory can
  /// be synced to iCloud or included in backups.
  static Future<Directory> walletsRoot() async {
    final support = await getApplicationSupportDirectory();
    final root = Directory(p.join(support.path, 'grin'));
    await _createPrivate(root);
    return root;
  }

  /// Returns `<application support>/grin/<walletId>` and creates it if it is
  /// missing. Pass the result as `GrinWalletConfig.walletDir`.
  ///
  /// Store only [walletId] in your own data and call this on every open. Do
  /// not store the returned path. The absolute path of the app container can
  /// change, for example on iOS when the app is reinstalled or restored from
  /// a backup, so a saved path can point nowhere while the wallet files are
  /// still present under the new container.
  ///
  /// Throws [ArgumentError] if [walletId] is empty, contains a path separator
  /// or is `.` or `..`.
  static Future<String> defaultWalletDirectory(String walletId) async {
    _checkWalletId(walletId);
    final root = await walletsRoot();
    final dir = Directory(p.join(root.path, walletId));
    await _createPrivate(dir);
    return dir.path;
  }

  static void _checkWalletId(String walletId) {
    if (walletId.isEmpty) {
      throw ArgumentError.value(walletId, 'walletId', 'must not be empty');
    }
    if (walletId.contains('/') ||
        walletId.contains(r'\') ||
        walletId.contains('\u0000') ||
        walletId == '.' ||
        walletId.contains('..')) {
      throw ArgumentError.value(
        walletId,
        'walletId',
        'must not contain path separators or ".."',
      );
    }
  }

  /// Creates [dir] and, on platforms with POSIX permissions, restricts it to
  /// the current user (0700). Windows and the like keep the default ACL of
  /// the application data folder.
  static Future<void> _createPrivate(Directory dir) async {
    await dir.create(recursive: true);
    if (Platform.isWindows) return;
    try {
      await Process.run('chmod', ['700', dir.path]);
    } on ProcessException {
      // No chmod binary (Android and iOS sandboxes). The app container is
      // private to the app there already.
    }
  }
}
