import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';

import 'package:meta/meta.dart';

import '../errors/grin_exception.dart';
import '../ffi/bindings.g.dart' as bindings;
import '../ffi/memory.dart';
import '../internal/json_helpers.dart';
import '../models/grin_address_info.dart';
import '../models/grin_address_validation.dart';
import '../models/grin_balance.dart';
import '../models/grin_chain_tip.dart';
import '../models/grin_mnemonic_validation.dart';
import '../models/grin_network.dart';
import '../models/grin_node_info.dart';
import '../models/grin_scan_progress.dart';
import '../models/grin_scan_result.dart';
import '../models/grin_wallet_config.dart';
import 'grin_wallet_event.dart';

/// A typed client for one open Grin wallet.
///
/// The native library is synchronous, so every wallet operation runs in a
/// short-lived isolate. Operations of one client run one after another in
/// the order they were called; [scanProgress] and [cancelScan] are the two
/// exceptions, because they take no wallet lock and must answer while a scan
/// runs. Errors are [GrinException]s. Amounts are `BigInt` nanogrin; see
/// `formatGrin` and `parseGrin`.
///
/// Call [close] when you are done. A client that is dropped without it is
/// closed by a native finalizer when it is garbage collected, but the
/// directory may stay busy until any operation still running has returned.
class GrinWalletClient implements Finalizable {
  /// Wraps a native wallet [handle].
  ///
  /// Only for code that obtains a handle by other means, such as the test
  /// harness. Use [create] or [open] otherwise. The client takes ownership
  /// of the handle and closes it.
  @internal
  GrinWalletClient.fromHandle({
    required int handle,
    required this.walletDir,
    required this.network,
  }) : _handle = handle {
    _finalizer.attach(this, Pointer<Void>.fromAddress(handle), detach: this);
  }

  /// Calls `grin_wallet_close_finalizer` with the handle as the token. A
  /// handle is a non-zero 64-bit number, so it is stored in the pointer
  /// itself and nothing is allocated.
  static final NativeFinalizer _finalizer = NativeFinalizer(
    bindings.addresses.grin_wallet_close_finalizer,
  );

  // Static members ----------------------------------------------------------

  /// Sets up native logging to stderr and a panic hook that logs panics.
  ///
  /// [level] is one of `trace`, `debug`, `info`, `warn`, `error` and `off`.
  /// It is a direct call that may be repeated: later calls only change the
  /// level. Throws [GrinInvalidArgumentException] for an unknown level.
  static void initLogs({String level = 'info'}) {
    usingSecretArena((arena) {
      unwrapFfiResult(bindings.grin_init_logs(toCString(level, arena)));
    });
  }

  /// Generates a new BIP39 mnemonic of [wordCount] words (12, 15, 18, 21 or
  /// 24).
  ///
  /// Throws [GrinInvalidArgumentException] for any other count.
  static Future<String> generateMnemonic({int wordCount = 24}) =>
      Isolate.run(() {
        final json = decodeJsonObject(
          unwrapFfiResult(bindings.grin_mnemonic_generate(wordCount)),
        );
        return readString(json['mnemonic'], 'mnemonic');
      });

  /// Checks a mnemonic. A bad phrase is a result with `valid` false, not an
  /// error.
  static Future<GrinMnemonicValidation> validateMnemonic(String mnemonic) =>
      Isolate.run(() {
        return usingSecretArena((arena) {
          final json = decodeJsonObject(
            unwrapFfiResult(
              bindings.grin_mnemonic_validate(toCStringSecret(mnemonic, arena)),
            ),
          );
          return GrinMnemonicValidation.fromJson(json);
        });
      });

  /// Checks [address] for [network].
  ///
  /// Accepts a Slatepack address, a v3 onion address (with or without
  /// scheme, `.onion` and `/v2/foreign`) and an `http(s)` URL. Throws
  /// [GrinInvalidAddressException] for anything else, including an address
  /// of the other network.
  static Future<GrinAddressValidation> validateAddress(
    String address,
    GrinNetwork network,
  ) {
    final networkValue = network.value;
    return Isolate.run(() {
      return usingSecretArena((arena) {
        final json = decodeJsonObject(
          unwrapFfiResult(
            bindings.grin_address_validate(
              toCString(address, arena),
              networkValue,
            ),
          ),
        );
        return GrinAddressValidation.fromJson(json);
      });
    });
  }

  /// Asks a node for its version and chain tip without a wallet.
  ///
  /// Throws [GrinNodeException] when it cannot be reached and
  /// [GrinInvalidArgumentException] for a bad URL or proxy.
  static Future<GrinNodeInfo> testNodeConnection({
    required String nodeUrl,
    String? apiSecret,
    String? socksProxy,
  }) => Isolate.run(() {
    return usingSecretArena((arena) {
      final json = decodeJsonObject(
        unwrapFfiResult(
          bindings.grin_node_test_connection(
            toCString(nodeUrl, arena),
            toCStringSecretOrNull(apiSecret, arena),
            toCStringOrNull(socksProxy, arena),
          ),
        ),
      );
      return GrinNodeInfo.fromJson(json);
    });
  });

  /// Creates a wallet in `<config.walletDir>/wallet_data` and opens it.
  ///
  /// With a null [mnemonic] a new 24-word seed is generated; otherwise the
  /// 12 to 24 word phrase is restored, and the wallet needs a [scan] to find
  /// its outputs. Nothing contacts the node.
  ///
  /// Throws [GrinWalletExistsException] if the directory already holds a
  /// wallet, [GrinInvalidMnemonicException] for a bad phrase and
  /// [GrinInvalidArgumentException] for a bad configuration.
  ///
  /// Throws [GrinWalletBusyException] when another wallet in this process
  /// holds the directory, or one that was closed is still finishing a call.
  /// The error is reported as it is and never retried. Close the other
  /// client, reuse it, or let the caller decide whether to wait.
  static Future<GrinWalletClient> create({
    required GrinWalletConfig config,
    String? mnemonic,
    required String password,
  }) async {
    final configJson = jsonEncode(config.toJson());
    final handle = await Isolate.run(() {
      return usingSecretArena((arena) {
        final json = decodeJsonObject(
          unwrapFfiResult(
            bindings.grin_wallet_create(
              toCStringSecret(configJson, arena),
              toCStringSecretOrNull(mnemonic, arena),
              toCStringSecret(password, arena),
            ),
          ),
        );
        return readInt(json['handle'], 'handle');
      });
    });
    return GrinWalletClient.fromHandle(
      handle: handle,
      walletDir: config.walletDir,
      network: config.network,
    );
  }

  /// Opens the wallet in `<config.walletDir>/wallet_data`. Nothing contacts
  /// the node.
  ///
  /// Throws [GrinWalletNotFoundException] if there is no wallet and
  /// [GrinInvalidPasswordException] for a wrong password.
  ///
  /// A directory can be held by one wallet per process. Throws
  /// [GrinWalletBusyException] (with `walletDir`) if it is open already, or
  /// if a wallet that was closed has a call that has not returned yet. After
  /// `await client.close()` the directory is always free again. The error is
  /// reported as it is and never retried.
  static Future<GrinWalletClient> open({
    required GrinWalletConfig config,
    required String password,
  }) async {
    final configJson = jsonEncode(config.toJson());
    final handle = await Isolate.run(() {
      return usingSecretArena((arena) {
        final json = decodeJsonObject(
          unwrapFfiResult(
            bindings.grin_wallet_open(
              toCStringSecret(configJson, arena),
              toCStringSecret(password, arena),
            ),
          ),
        );
        return readInt(json['handle'], 'handle');
      });
    });
    return GrinWalletClient.fromHandle(
      handle: handle,
      walletDir: config.walletDir,
      network: config.network,
    );
  }

  /// Deletes `<walletDir>/wallet_data` and everything in it. `walletDir`
  /// itself stays. A directory without a wallet counts as already deleted.
  ///
  /// This cannot delete a wallet that a client has open: close the client
  /// first. Throws [GrinWalletBusyException] if an open wallet in this
  /// process, or a closed one with a call still running, holds the
  /// directory. Nothing is deleted then, and the error is never retried.
  static Future<void> delete({required String walletDir}) => Isolate.run(() {
    usingSecretArena((arena) {
      unwrapFfiResult(
        bindings.grin_wallet_delete(toCString(walletDir, arena), 0),
      );
    });
  });

  // Instance members --------------------------------------------------------

  /// The directory the wallet was opened from, as it was given.
  final String walletDir;

  /// The chain the wallet is on.
  final GrinNetwork network;

  final int _handle;
  final StreamController<GrinWalletEvent> _events =
      StreamController<GrinWalletEvent>.broadcast();
  bool _closed = false;
  Future<void>? _closing;

  /// The end of the operation queue. It never completes with an error: each
  /// caller gets its own result, and the queue only needs to know when the
  /// operation has settled.
  Future<void> _tail = Future<void>.value();

  /// Whether [close] has been called.
  bool get isClosed => _closed;

  /// Events of this client. A broadcast stream: listen before the action
  /// whose events you want. It ends after [GrinWalletClosed].
  Stream<GrinWalletEvent> get events => _events.stream;

  /// The native handle. For the test harness only.
  @internal
  int get nativeHandle {
    _ensureOpen();
    return _handle;
  }

  /// Runs [operation] after everything queued before it. For the test
  /// harness only.
  @internal
  Future<T> runQueued<T>(Future<T> Function() operation) {
    _ensureOpen();
    return _enqueue(operation);
  }

  void _ensureOpen() {
    if (_closed) throw StateError('GrinWalletClient is closed');
  }

  void _emit(GrinWalletEvent event) {
    if (!_events.isClosed) _events.add(event);
  }

  /// Chains [operation] behind the previous one. A failure goes to the
  /// caller of this operation only; the queue carries on.
  Future<T> _enqueue<T>(Future<T> Function() operation) {
    final result = _tail.then((_) => operation());
    _tail = result.then<void>((_) {}, onError: (Object _) {});
    return result;
  }

  /// The address at derivation [index].
  Future<GrinAddressInfo> getAddress({int index = 0}) {
    _ensureOpen();
    final handle = _handle;
    return _enqueue(() => _getAddress(handle, index));
  }

  /// The tip of the chain according to the wallet's node.
  Future<GrinChainTip> getChainHeight() {
    _ensureOpen();
    final handle = _handle;
    return _enqueue(() => _getChainHeight(handle));
  }

  /// The balance.
  ///
  /// With [refreshFromNode] the wallet first updates its outputs and
  /// transactions from the node. If the node cannot be reached, the stored
  /// state comes back with `refreshedFromNode` false. [minimumConfirmations]
  /// is the depth an output needs to count as spendable.
  Future<GrinBalance> getBalance({
    bool refreshFromNode = true,
    int minimumConfirmations = 10,
  }) {
    _ensureOpen();
    final handle = _handle;
    return _enqueue(
      () => _getBalance(handle, refreshFromNode, minimumConfirmations),
    );
  }

  /// Scans the chain from [startHeight] (0 means from the first block) and
  /// repairs the wallet's outputs. Transaction history is kept.
  ///
  /// It can take long. Poll [scanProgress] meanwhile. A scan that has
  /// started cannot be stopped; see [cancelScan]. Emits [GrinScanStarted]
  /// and [GrinScanFinished].
  Future<GrinScanResult> scan({
    int startHeight = 0,
    bool deleteUnconfirmed = false,
  }) {
    _ensureOpen();
    final handle = _handle;
    return _enqueue(() async {
      _emit(GrinScanStarted(startHeight: startHeight));
      try {
        final result = await _scan(handle, startHeight, deleteUnconfirmed);
        _emit(GrinScanFinished(result: result));
        return result;
      } catch (error) {
        _emit(GrinScanFinished(error: error));
        rethrow;
      }
    });
  }

  /// The progress of the current or most recent scan.
  ///
  /// A direct call that is not queued and takes no wallet lock, so it
  /// answers while a scan runs.
  GrinScanProgress scanProgress() {
    _ensureOpen();
    return GrinScanProgress.fromJson(
      decodeJsonObject(
        unwrapFfiResult(bindings.grin_wallet_scan_progress(_handle)),
      ),
    );
  }

  /// Asks for the scan to stop. A direct call that is not queued.
  ///
  /// The native scan has no hook between its 10,000-block batches, so a scan
  /// that is running cannot be interrupted and finishes normally. If none is
  /// running, the next [scan] throws [GrinCancelledException] without
  /// starting.
  void cancelScan() {
    _ensureOpen();
    unwrapFfiResult(bindings.grin_wallet_scan_cancel(_handle));
  }

  /// Re-encrypts the wallet seed with [newPassword].
  ///
  /// Throws [GrinInvalidPasswordException] if [oldPassword] is wrong.
  Future<void> changePassword(String oldPassword, String newPassword) {
    _ensureOpen();
    final handle = _handle;
    return _enqueue(() => _changePassword(handle, oldPassword, newPassword));
  }

  /// The recovery phrase, after checking [password].
  ///
  /// Throws [GrinInvalidPasswordException] if the password is wrong. The
  /// returned string cannot be wiped from memory, so keep it as briefly as
  /// you can.
  Future<String> getMnemonic(String password) {
    _ensureOpen();
    final handle = _handle;
    return _enqueue(() => _getMnemonic(handle, password));
  }

  /// Points the wallet at another node, and optionally a SOCKS proxy,
  /// without reopening the database. Nothing contacts the node.
  Future<void> updateNetwork({
    required String nodeUrl,
    String? apiSecret,
    String? socksProxy,
  }) {
    _ensureOpen();
    final handle = _handle;
    return _enqueue(
      () => _updateNetwork(handle, nodeUrl, apiSecret, socksProxy),
    );
  }

  // Phase 1b, not part of this class yet:
  //   estimateFee, getTransactions
  //   decodeSlatepack, initSend, sendHttp, receiveSlatepack, finalizeSlatepack
  //   issueInvoice, processInvoice, finalizeInvoice
  //   repost, cancelTransaction
  //   exportPaymentProof, verifyPaymentProof

  /// Closes the wallet.
  ///
  /// Safe to call more than once: every call returns the same future. In
  /// order, it
  /// 1. marks the client closed, so new calls throw [StateError],
  /// 2. asks a running scan to stop,
  /// 3. waits for the operations already queued, ignoring their errors,
  /// 4. detaches the native finalizer,
  /// 5. closes the native handle and then the [events] stream.
  ///
  /// When the returned future completes, the wallet directory is free to be
  /// opened again.
  Future<void> close() => _closing ??= _close();

  Future<void> _close() async {
    _closed = true;
    final handle = _handle;
    try {
      unwrapFfiResult(bindings.grin_wallet_scan_cancel(handle));
    } on GrinException {
      // The wallet is going away either way.
    }
    await _tail;
    _finalizer.detach(this);
    try {
      await _closeHandle(handle);
    } finally {
      _emit(const GrinWalletClosed());
      await _events.close();
    }
  }

  // Isolate helpers ---------------------------------------------------------
  //
  // Each one is static, takes only numbers, booleans and strings, and builds
  // its Isolate.run closure from those alone. A closure made in an instance
  // method could drag `this` (a Finalizable with a StreamController) into
  // the message, and Isolate.run rejects that.

  static Future<GrinAddressInfo> _getAddress(int handle, int index) =>
      Isolate.run(() {
        final json = decodeJsonObject(
          unwrapFfiResult(bindings.grin_wallet_get_address(handle, index)),
        );
        return GrinAddressInfo.fromJson(json);
      });

  static Future<GrinChainTip> _getChainHeight(int handle) => Isolate.run(() {
    final json = decodeJsonObject(
      unwrapFfiResult(bindings.grin_wallet_get_chain_height(handle)),
    );
    return GrinChainTip.fromJson(json);
  });

  static Future<GrinBalance> _getBalance(
    int handle,
    bool refreshFromNode,
    int minConfirms,
  ) => Isolate.run(() {
    final json = decodeJsonObject(
      unwrapFfiResult(
        bindings.grin_wallet_get_balance(handle, refreshFromNode, minConfirms),
      ),
    );
    return GrinBalance.fromJson(json);
  });

  static Future<GrinScanResult> _scan(
    int handle,
    int startHeight,
    bool deleteUnconfirmed,
  ) => Isolate.run(() {
    final json = decodeJsonObject(
      unwrapFfiResult(
        bindings.grin_wallet_scan(handle, startHeight, deleteUnconfirmed),
      ),
    );
    return GrinScanResult.fromJson(json);
  });

  static Future<void> _changePassword(
    int handle,
    String oldPassword,
    String newPassword,
  ) => Isolate.run(() {
    usingSecretArena((arena) {
      unwrapFfiResult(
        bindings.grin_wallet_change_password(
          handle,
          toCStringSecret(oldPassword, arena),
          toCStringSecret(newPassword, arena),
        ),
      );
    });
  });

  static Future<String> _getMnemonic(int handle, String password) =>
      Isolate.run(() {
        return usingSecretArena((arena) {
          final json = decodeJsonObject(
            unwrapFfiResult(
              bindings.grin_wallet_get_mnemonic(
                handle,
                toCStringSecret(password, arena),
              ),
            ),
          );
          return readString(json['mnemonic'], 'mnemonic');
        });
      });

  static Future<void> _updateNetwork(
    int handle,
    String nodeUrl,
    String? apiSecret,
    String? socksProxy,
  ) => Isolate.run(() {
    usingSecretArena((arena) {
      unwrapFfiResult(
        bindings.grin_wallet_update_network(
          handle,
          toCString(nodeUrl, arena),
          toCStringSecretOrNull(apiSecret, arena),
          toCStringOrNull(socksProxy, arena),
        ),
      );
    });
  });

  static Future<void> _closeHandle(int handle) => Isolate.run(() {
    unwrapFfiResult(bindings.grin_wallet_close(handle));
  });
}
