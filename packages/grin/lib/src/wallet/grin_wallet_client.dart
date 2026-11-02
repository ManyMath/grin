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
import '../models/grin_fee_estimate.dart';
import '../models/grin_finalize_result.dart';
import '../models/grin_invoice_args.dart';
import '../models/grin_mnemonic_validation.dart';
import '../models/grin_network.dart';
import '../models/grin_node_info.dart';
import '../models/grin_payment_proof.dart';
import '../models/grin_scan_progress.dart';
import '../models/grin_scan_result.dart';
import '../models/grin_send_args.dart';
import '../models/grin_send_result.dart';
import '../models/grin_slate_info.dart';
import '../models/grin_slatepack.dart';
import '../models/grin_tx_list.dart';
import '../models/grin_tx_log_entry.dart';
import '../models/grin_wallet_config.dart';
import 'grin_wallet_event.dart';

/// A typed client for one open Grin wallet.
///
/// The native library is synchronous, so every wallet operation runs in a
/// short-lived isolate. Operations of one client run one after another in
/// the order they were called; [scanProgress] and [cancelScan] are the two
/// exceptions, because they take no wallet lock and must answer while a scan
/// runs. Amounts are `BigInt` nanogrin; see `formatGrin` and `parseGrin`.
///
/// Errors reach the caller in three ways:
/// - A bad argument ([ArgumentError]: a negative or out of range number, an
///   amount that does not fit the native argument, a string with U+0000) and
///   use after [close] ([StateError]) are thrown synchronously, before a
///   future exists. They are programming errors, so they do not reach a
///   `catchError` on the returned future.
/// - Everything the native library reports arrives as a [GrinException]
///   through the returned future.
/// - Native output that does not have the documented shape surfaces as a
///   [FormatException], also through the returned future. It means a
///   mismatch between this package and the library, not a wallet problem.
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
  /// Throws [GrinInvalidArgumentException] for any other count, and
  /// [ArgumentError] for a count that is not between 0 and 4294967295.
  static Future<String> generateMnemonic({int wordCount = 24}) {
    final nativeWordCount = requireU32(wordCount, 'wordCount');
    return Isolate.run(() {
      final json = decodeJsonObject(
        unwrapFfiResult(bindings.grin_mnemonic_generate(nativeWordCount)),
      );
      return readString(json['mnemonic'], 'mnemonic');
    });
  }

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
    // The callbacks below call a method of this client, so the pending
    // future chain refers to it. That keeps the Finalizable reachable while
    // an operation is queued or running, so the native finalizer cannot
    // close the handle under a call the caller still waits for. The queued
    // closures themselves hold only the handle, so without this a client
    // that was dropped with work queued could be collected.
    _tail = result.then<void>(
      (_) => _keepAlive(),
      onError: (Object _) => _keepAlive(),
    );
    return result;
  }

  /// Does nothing. It only gives the callbacks in [_enqueue] a reference to
  /// this client.
  void _keepAlive() {}

  /// The address at derivation [index].
  ///
  /// Throws [ArgumentError] unless [index] is between 0 and 4294967295.
  Future<GrinAddressInfo> getAddress({int index = 0}) {
    _ensureOpen();
    final nativeIndex = requireU32(index, 'index');
    final handle = _handle;
    return _enqueue(() => _getAddress(handle, nativeIndex));
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
  /// is the depth an output needs to count as spendable, and must not be
  /// negative ([ArgumentError]).
  Future<GrinBalance> getBalance({
    bool refreshFromNode = true,
    int minimumConfirmations = 10,
  }) {
    _ensureOpen();
    final nativeConfirmations = requireU64(
      minimumConfirmations,
      'minimumConfirmations',
    );
    final handle = _handle;
    return _enqueue(
      () => _getBalance(handle, refreshFromNode, nativeConfirmations),
    );
  }

  /// Scans the chain from [startHeight] (0 means from the first block) and
  /// repairs the wallet's outputs. Transaction history is kept.
  ///
  /// It can take long. Poll [scanProgress] meanwhile. A scan that has
  /// started cannot be stopped; see [cancelScan]. Emits [GrinScanStarted]
  /// and [GrinScanFinished]. Throws [ArgumentError] for a negative
  /// [startHeight].
  Future<GrinScanResult> scan({
    int startHeight = 0,
    bool deleteUnconfirmed = false,
  }) {
    _ensureOpen();
    final nativeStartHeight = requireU64(startHeight, 'startHeight');
    final handle = _handle;
    return _enqueue(() async {
      // A cancel only reaches a scan that is running, so a scan that was
      // still queued when close() ran must not start now.
      if (_closed) {
        throw StateError('GrinWalletClient was closed before the scan ran');
      }
      _emit(GrinScanStarted(startHeight: nativeStartHeight));
      try {
        final result = await _scan(
          handle,
          nativeStartHeight,
          deleteUnconfirmed,
        );
        _emit(GrinScanFinished(result: result));
        return result;
      } catch (error, stackTrace) {
        _emit(GrinScanFinished(error: error, stackTrace: stackTrace));
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

  /// Asks a running scan to stop. A direct call that is not queued.
  ///
  /// The native scan checks for a cancel between its batches of 10,001
  /// blocks, so a running [scan] stops at the next batch boundary and its
  /// future completes with [GrinCancelledException], whose `details` carry
  /// the last saved `scanned_height`. With no scan running this does
  /// nothing; the next scan runs normally.
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
  ///
  /// Like every operation it runs in the queue, so it starts only after the
  /// operations before it. To apply it during a long scan, call [cancelScan]
  /// first; the scan then stops at its next batch boundary. See section 8 of
  /// docs/ffi-json.md.
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

  /// Computes the fee and the inputs a send of [amount] would use, without
  /// saving or locking anything.
  ///
  /// Refreshes the wallet's outputs from the node first, then selects inputs
  /// exactly as `initSend` would. With [amountIncludesFee] the fee is taken
  /// out of [amount], otherwise it is added on top. [selectionStrategyIsAll]
  /// spends every spendable output (up to 500) instead of only as many of
  /// the smallest as needed.
  ///
  /// Throws [ArgumentError] when [amount] is negative or does not fit the
  /// native 64-bit argument (values from 2^63 up are refused, which no real
  /// balance reaches), or when [minimumConfirmations] is negative. Throws
  /// [GrinInvalidArgumentException] for an amount of
  /// 0, [GrinInsufficientFundsException] when the funds do not cover it and
  /// [GrinTransactionException] when [amountIncludesFee] and the amount is
  /// below the fee.
  Future<GrinFeeEstimate> estimateFee({
    required BigInt amount,
    int minimumConfirmations = 10,
    bool selectionStrategyIsAll = false,
    bool amountIncludesFee = false,
  }) {
    _ensureOpen();
    final handle = _handle;
    final nativeAmount = amountToNative(amount);
    final nativeConfirmations = requireU64(
      minimumConfirmations,
      'minimumConfirmations',
    );
    return _enqueue(
      () => _estimateFee(
        handle,
        nativeAmount,
        nativeConfirmations,
        selectionStrategyIsAll,
        amountIncludesFee,
      ),
    );
  }

  /// The wallet's transaction log, joined with the Slatepack and
  /// counterparty data the library keeps for each transaction.
  ///
  /// With [refreshFromNode] the outputs and transaction states are updated
  /// from the node first. If the node cannot be reached, the stored history
  /// comes back with `refreshedFromNode` false. Entries are in log order.
  Future<GrinTxList> getTransactions({bool refreshFromNode = true}) {
    _ensureOpen();
    final handle = _handle;
    return _enqueue(() => _getTransactions(handle, refreshFromNode));
  }

  /// Reads a Slatepack with this wallet's keys, without changing anything.
  ///
  /// An encrypted Slatepack is decrypted with the wallet's address key at
  /// [secretIndex], which must be between 0 and 4294967295 ([ArgumentError]
  /// otherwise). Leading and trailing whitespace is ignored. Throws
  /// [GrinSlateException] when the text cannot be parsed or decrypted. Use
  /// [decodeSlatepackWithoutWallet] when no wallet is at hand.
  Future<GrinSlateInfo> decodeSlatepack(String armored, {int secretIndex = 0}) {
    _ensureOpen();
    final nativeSecretIndex = requireU32(secretIndex, 'secretIndex');
    final handle = _handle;
    final networkValue = network.value;
    return _enqueue(
      () => _decodeSlatepack(handle, networkValue, armored, nativeSecretIndex),
    );
  }

  /// Reads an unencrypted Slatepack for [network] without a wallet.
  ///
  /// An encrypted Slatepack throws [GrinSlateException], because only a
  /// wallet can decrypt it.
  static Future<GrinSlateInfo> decodeSlatepackWithoutWallet(
    String armored,
    GrinNetwork network,
  ) => _decodeSlatepack(0, network.value, armored, 0);

  /// Starts a send and returns the S1 Slatepack to hand to the recipient.
  ///
  /// Expects no earlier state. Locks the inputs now, unless
  /// [GrinSendArgs.lateLock] is set, so the funds are unavailable until the
  /// send is finalized or cancelled; cancel one that will not be completed
  /// with [cancelTransaction]. With [GrinSendArgs.recipientAddress] the S1 is
  /// encrypted to that address and a payment proof is requested.
  ///
  /// Throws [ArgumentError] for an amount that does not fit the native
  /// argument, [GrinInvalidArgumentException], [GrinJsonException],
  /// [GrinInvalidAddressException], [GrinInsufficientFundsException],
  /// [GrinTransactionException] or [GrinSlateException].
  Future<GrinSlatepack> initSend(GrinSendArgs args) {
    _ensureOpen();
    final handle = _handle;
    final argsJson = jsonEncode(args.toJson());
    return _enqueue(() => _initSend(handle, argsJson));
  }

  /// Sends directly to the recipient's wallet over HTTP or Tor and finalizes
  /// the transaction in one call.
  ///
  /// Expects no earlier state. The inputs are locked only after the recipient
  /// has answered, so a failed exchange leaves nothing locked and nothing in
  /// the history. A target that resolves to `.onion` needs a SOCKS proxy in
  /// the wallet's network settings (see [updateNetwork]). With
  /// [GrinHttpSendArgs.post] the transaction is broadcast.
  ///
  /// Throws [GrinHttpException] when the other wallet cannot be reached or
  /// refuses the slate, and [GrinNodeException] (with `slate_id` in its
  /// details) when the transaction was finalized but the broadcast failed;
  /// use [repost] for that one. Other errors are as for [initSend].
  Future<GrinSendResult> sendHttp(GrinHttpSendArgs args) {
    _ensureOpen();
    final handle = _handle;
    final argsJson = jsonEncode(args.toJson());
    return _enqueue(() => _sendHttp(handle, argsJson));
  }

  /// Answers an S1 Slatepack and returns the S2 to hand back to the sender.
  ///
  /// Expects an S1, plain or encrypted to this wallet. Adds this wallet's
  /// output and signature, so a new received transaction appears in the log.
  /// When the S1 names its sender, the S2 is encrypted to it. Locks no
  /// inputs.
  ///
  /// Throws [GrinSlateException] when the text is not an S1 or cannot be
  /// decrypted, and [GrinTransactionException] when this wallet has already
  /// received it or it has expired.
  Future<GrinSlatepack> receiveSlatepack(String armoredS1) {
    _ensureOpen();
    final handle = _handle;
    return _enqueue(() => _receiveSlatepack(handle, armoredS1));
  }

  /// Finishes a send: takes the S2 and returns the final transaction.
  ///
  /// Expects the S2 of a send this wallet started with [initSend], whose
  /// inputs are already locked (or locks them now with
  /// [GrinSendArgs.lateLock]). With [broadcast] the transaction is posted to
  /// the node; [fluff] skips the Dandelion stem phase. Without it the
  /// transaction is stored and [repost] can broadcast it later.
  ///
  /// Throws [GrinSlateException] when the text is not an S2,
  /// [GrinNotFoundException] when this wallet has no open transaction for the
  /// slate (unknown, or already finalized), [GrinTransactionException] for a
  /// bad signature, a payment proof mismatch or an expired transaction, and
  /// [GrinNodeException] (with `slate_id` in its details) when the broadcast
  /// fails after the transaction was finalized.
  Future<GrinFinalizeResult> finalizeSlatepack(
    String armoredS2, {
    bool broadcast = true,
    bool fluff = false,
  }) {
    _ensureOpen();
    final handle = _handle;
    return _enqueue(
      () => _finalizeSlatepack(handle, armoredS2, broadcast, fluff),
    );
  }

  /// Requests a payment: returns the I1 Slatepack to hand to the payer.
  ///
  /// Expects no earlier state. Adds a received transaction to the log at
  /// once, which [cancelTransaction] can remove if the payer never answers.
  /// Locks no inputs. With [GrinInvoiceArgs.recipientAddress] the I1 is
  /// encrypted to the payer.
  ///
  /// Throws [ArgumentError] for an amount that does not fit the native
  /// argument, [GrinInvalidArgumentException], [GrinJsonException],
  /// [GrinInvalidAddressException] or [GrinTransactionException].
  Future<GrinSlatepack> issueInvoice(GrinInvoiceArgs args) {
    _ensureOpen();
    final handle = _handle;
    final argsJson = jsonEncode(args.toJson());
    return _enqueue(() => _issueInvoice(handle, argsJson));
  }

  /// Pays an invoice: returns the I2 Slatepack to hand back to the payee.
  ///
  /// Expects an I1. Selects inputs, signs, and locks the inputs in the same
  /// call, so the funds are unavailable until the invoice is finalized or the
  /// transaction is cancelled with [cancelTransaction]. The I2 is encrypted
  /// to the payee when the I1 named one.
  ///
  /// Throws [GrinSlateException] when the text is not an I1,
  /// [GrinInsufficientFundsException], and [GrinTransactionException] when
  /// the invoice was already paid or cancelled, or has expired.
  Future<GrinSlatepack> processInvoice(
    String armoredI1,
    GrinProcessInvoiceArgs args,
  ) {
    _ensureOpen();
    final handle = _handle;
    final argsJson = jsonEncode(args.toJson());
    return _enqueue(() => _processInvoice(handle, armoredI1, argsJson));
  }

  /// Finishes an invoice: takes the I2 and returns the final transaction.
  ///
  /// Expects the I2 of an invoice this wallet issued with [issueInvoice].
  /// Locks nothing new (the payer's inputs were locked by [processInvoice]).
  /// [broadcast] and [fluff] work as in [finalizeSlatepack], and so do the
  /// errors, with [GrinSlateException] for anything that is not an I2.
  Future<GrinFinalizeResult> finalizeInvoice(
    String armoredI2, {
    bool broadcast = true,
    bool fluff = false,
  }) {
    _ensureOpen();
    final handle = _handle;
    return _enqueue(
      () => _finalizeInvoice(handle, armoredI2, broadcast, fluff),
    );
  }

  /// Broadcasts the stored finalized transaction again.
  ///
  /// Name the transaction by [txId] (the `id` of a [GrinTxLogEntry]), by
  /// [slateId], or both; at least one is required, otherwise this throws
  /// [GrinInvalidArgumentException]. When both are given, [txId] is looked
  /// up first and [slateId] is the fallback. A negative [txId] throws
  /// [ArgumentError]; pass null to leave it out. The transaction is validated before it is posted.
  ///
  /// Only a transaction that is finalized and not yet confirmed can be
  /// reposted; never repost a confirmed one. Throws [GrinNotFoundException]
  /// when there is no such transaction, nothing is stored for it or the
  /// stored copy is not finalized, and [GrinNodeException] when the node
  /// refuses it. Changes no wallet state.
  Future<void> repost({int? txId, String? slateId, bool fluff = false}) {
    _ensureOpen();
    final handle = _handle;
    final nativeTxId = txIdToNative(txId);
    return _enqueue(() => _repost(handle, nativeTxId, slateId, fluff));
  }

  /// Cancels an unconfirmed sent, received or reverted transaction.
  ///
  /// Name the transaction by [txId], by [slateId], or both (both must then
  /// match the same transaction); at least one is required, otherwise this
  /// throws [GrinInvalidArgumentException]. A negative [txId] throws
  /// [ArgumentError]; pass null to leave it out. Inputs the transaction locked become spendable again and its
  /// new outputs are dropped; the log entry becomes
  /// [GrinTxType.txSentCancelled] or [GrinTxType.txReceivedCancelled].
  ///
  /// The wallet refreshes from the node first and refuses to cancel without
  /// it, so an unreachable node throws [GrinNodeException]. Throws
  /// [GrinNotFoundException] for an unknown transaction and
  /// [GrinTransactionException] when it is confirmed, a coinbase or already
  /// cancelled.
  Future<void> cancelTransaction({int? txId, String? slateId}) {
    _ensureOpen();
    final handle = _handle;
    final nativeTxId = txIdToNative(txId);
    return _enqueue(() => _cancelTransaction(handle, nativeTxId, slateId));
  }

  /// Exports the payment proof of a finalized send that asked for one.
  ///
  /// Name the transaction by [txId], by [slateId], or both (both must then
  /// match). Uses the stored history and does not contact the node. Only the
  /// sender has a proof, and only when it passed a recipient address to
  /// [initSend]. Throws [GrinNotFoundException] when there is no such
  /// transaction, no proof, or the transaction is not finalized yet,
  /// [GrinInvalidArgumentException] when neither identifier is given and
  /// [ArgumentError] for a negative [txId].
  Future<GrinPaymentProof> exportPaymentProof({int? txId, String? slateId}) {
    _ensureOpen();
    final handle = _handle;
    final nativeTxId = txIdToNative(txId);
    return _enqueue(() => _exportPaymentProof(handle, nativeTxId, slateId));
  }

  /// Checks a payment proof and says whether this wallet is its sender or
  /// recipient.
  ///
  /// Looks up the kernel on the chain through the wallet's node and checks
  /// both signatures, then compares the addresses with this wallet's index 0
  /// address. Both flags false means the proof is valid but belongs to two
  /// other wallets. The addresses must be for the wallet's network.
  ///
  /// Throws [GrinTransactionException] when a signature does not verify or
  /// the kernel is not on the chain, [GrinJsonException] when the proof is
  /// malformed and [GrinNodeException] when the node cannot be reached.
  Future<GrinPaymentProofVerification> verifyPaymentProof(
    GrinPaymentProof proof,
  ) {
    _ensureOpen();
    final handle = _handle;
    final proofJson = proof.toJsonString();
    return _enqueue(() => _verifyPaymentProof(handle, proofJson));
  }

  /// Closes the wallet.
  ///
  /// Safe to call more than once: every call returns the same future. In
  /// order, it
  /// 1. marks the client closed, so new calls throw [StateError],
  /// 2. calls [cancelScan], so a running scan stops at its next batch
  ///    boundary and a scan still waiting in the queue is skipped,
  /// 3. waits for the operations already queued, ignoring their errors,
  /// 4. detaches the native finalizer,
  /// 5. closes the native handle and then the [events] stream.
  ///
  /// Step 3 can still take up to one scan batch (10,001 blocks and their
  /// node requests). See section 8 of docs/ffi-json.md.
  ///
  /// When the returned future completes, the wallet directory is free to be
  /// opened again. The [events] stream is closed without waiting for its
  /// listeners, so a paused listener cannot stall this future.
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
      // Not awaited: a broadcast controller finishes closing only when
      // every subscription has received the done event, and a paused one
      // (an `await for` body that calls close(), a StreamQueue) never does.
      unawaited(_events.close());
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

  static Future<GrinFeeEstimate> _estimateFee(
    int handle,
    int amount,
    int minConfirms,
    bool selectionStrategyIsAll,
    bool amountIncludesFee,
  ) => Isolate.run(() {
    final json = decodeJsonObject(
      unwrapFfiResult(
        bindings.grin_tx_estimate_fee(
          handle,
          amount,
          minConfirms,
          selectionStrategyIsAll,
          amountIncludesFee,
        ),
      ),
    );
    return GrinFeeEstimate.fromJson(json);
  });

  static Future<GrinTxList> _getTransactions(
    int handle,
    bool refreshFromNode,
  ) => Isolate.run(() {
    final json = decodeJsonObject(
      unwrapFfiResult(bindings.grin_txs_get(handle, refreshFromNode)),
    );
    return GrinTxList.fromJson(json);
  });

  static Future<GrinSlateInfo> _decodeSlatepack(
    int handle,
    int networkValue,
    String armored,
    int secretIndex,
  ) => Isolate.run(() {
    return usingSecretArena((arena) {
      final json = decodeJsonObject(
        unwrapFfiResult(
          bindings.grin_slatepack_decode(
            handle,
            networkValue,
            toCString(armored, arena),
            secretIndex,
          ),
        ),
      );
      return GrinSlateInfo.fromJson(json);
    });
  });

  static Future<GrinSlatepack> _initSend(int handle, String argsJson) =>
      Isolate.run(() {
        return usingSecretArena((arena) {
          final json = decodeJsonObject(
            unwrapFfiResult(
              bindings.grin_tx_init_send(handle, toCString(argsJson, arena)),
            ),
          );
          return GrinSlatepack.fromJson(json);
        });
      });

  static Future<GrinSendResult> _sendHttp(int handle, String argsJson) =>
      Isolate.run(() {
        return usingSecretArena((arena) {
          final json = decodeJsonObject(
            unwrapFfiResult(
              bindings.grin_tx_send_http(handle, toCString(argsJson, arena)),
            ),
          );
          return GrinSendResult.fromJson(json);
        });
      });

  static Future<GrinSlatepack> _receiveSlatepack(
    int handle,
    String armoredS1,
  ) => Isolate.run(() {
    return usingSecretArena((arena) {
      final json = decodeJsonObject(
        unwrapFfiResult(
          bindings.grin_tx_receive_slatepack(
            handle,
            toCString(armoredS1, arena),
            nullptr,
          ),
        ),
      );
      return GrinSlatepack.fromJson(json);
    });
  });

  static Future<GrinFinalizeResult> _finalizeSlatepack(
    int handle,
    String armoredS2,
    bool broadcast,
    bool fluff,
  ) => Isolate.run(() {
    return usingSecretArena((arena) {
      final json = decodeJsonObject(
        unwrapFfiResult(
          bindings.grin_tx_finalize_slatepack(
            handle,
            toCString(armoredS2, arena),
            broadcast,
            fluff,
          ),
        ),
      );
      return GrinFinalizeResult.fromJson(json);
    });
  });

  static Future<GrinSlatepack> _issueInvoice(int handle, String argsJson) =>
      Isolate.run(() {
        return usingSecretArena((arena) {
          final json = decodeJsonObject(
            unwrapFfiResult(
              bindings.grin_tx_issue_invoice(
                handle,
                toCString(argsJson, arena),
              ),
            ),
          );
          return GrinSlatepack.fromJson(json);
        });
      });

  static Future<GrinSlatepack> _processInvoice(
    int handle,
    String armoredI1,
    String argsJson,
  ) => Isolate.run(() {
    return usingSecretArena((arena) {
      final json = decodeJsonObject(
        unwrapFfiResult(
          bindings.grin_tx_process_invoice(
            handle,
            toCString(armoredI1, arena),
            toCString(argsJson, arena),
          ),
        ),
      );
      return GrinSlatepack.fromJson(json);
    });
  });

  static Future<GrinFinalizeResult> _finalizeInvoice(
    int handle,
    String armoredI2,
    bool broadcast,
    bool fluff,
  ) => Isolate.run(() {
    return usingSecretArena((arena) {
      final json = decodeJsonObject(
        unwrapFfiResult(
          bindings.grin_tx_finalize_invoice(
            handle,
            toCString(armoredI2, arena),
            broadcast,
            fluff,
          ),
        ),
      );
      return GrinFinalizeResult.fromJson(json);
    });
  });

  static Future<void> _repost(
    int handle,
    int txId,
    String? slateId,
    bool fluff,
  ) => Isolate.run(() {
    usingSecretArena((arena) {
      unwrapFfiResult(
        bindings.grin_tx_repost(
          handle,
          txId,
          toCStringOrNull(slateId, arena),
          fluff,
        ),
      );
    });
  });

  static Future<void> _cancelTransaction(
    int handle,
    int txId,
    String? slateId,
  ) => Isolate.run(() {
    usingSecretArena((arena) {
      unwrapFfiResult(
        bindings.grin_tx_cancel(handle, txId, toCStringOrNull(slateId, arena)),
      );
    });
  });

  static Future<GrinPaymentProof> _exportPaymentProof(
    int handle,
    int txId,
    String? slateId,
  ) => Isolate.run(() {
    return usingSecretArena((arena) {
      final json = decodeJsonObject(
        unwrapFfiResult(
          bindings.grin_payment_proof_export(
            handle,
            txId,
            toCStringOrNull(slateId, arena),
          ),
        ),
      );
      return GrinPaymentProof.fromJson(json);
    });
  });

  static Future<GrinPaymentProofVerification> _verifyPaymentProof(
    int handle,
    String proofJson,
  ) => Isolate.run(() {
    return usingSecretArena((arena) {
      final json = decodeJsonObject(
        unwrapFfiResult(
          bindings.grin_payment_proof_verify(
            handle,
            toCString(proofJson, arena),
          ),
        ),
      );
      return GrinPaymentProofVerification.fromJson(json);
    });
  });

  static Future<void> _closeHandle(int handle) => Isolate.run(() {
    unwrapFfiResult(bindings.grin_wallet_close(handle));
  });
}
