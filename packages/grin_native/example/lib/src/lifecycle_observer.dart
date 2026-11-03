import 'package:flutter/widgets.dart';
import 'package:grin_native/grin_native.dart';

/// Cancels a running scan when the app goes to the background.
///
/// It listens to [AppLifecycleListener] and calls [GrinWalletApi.cancelScan]
/// when the app becomes `paused` or `detached`, as configured. Call [dispose]
/// before closing the wallet.
///
/// The native scan cannot be interrupted between its batches of blocks, so
/// the cancel takes effect when the running batch ends. See
/// [GrinWalletApi.cancelScan].
class GrinLifecycleObserver {
  /// Watches [wallet]. [onError] receives every error from `cancelScan`
  /// except the [StateError] of a closed wallet, which is ignored.
  GrinLifecycleObserver({
    required GrinWalletApi wallet,
    bool cancelScanOnPause = true,
    bool cancelScanOnDetach = true,
    void Function(Object error)? onError,
    bool autoStart = true,
  }) : this.custom(
         cancelScan: wallet.cancelScan,
         cancelScanOnPause: cancelScanOnPause,
         cancelScanOnDetach: cancelScanOnDetach,
         onError: onError,
         autoStart: autoStart,
       );

  /// Same as the default constructor with a plain callback instead of a
  /// wallet. Tests use it to run without the native library.
  GrinLifecycleObserver.custom({
    required this._cancelScan,
    this._cancelScanOnPause = true,
    this._cancelScanOnDetach = true,
    this._onError,
    bool autoStart = true,
  }) {
    if (autoStart) start();
  }

  final void Function() _cancelScan;
  final bool _cancelScanOnPause;
  final bool _cancelScanOnDetach;
  final void Function(Object error)? _onError;

  AppLifecycleListener? _listener;
  AppLifecycleState? _lastState;
  bool _disposed = false;

  /// The most recent state this observer saw, or null before the first
  /// change.
  AppLifecycleState? get lastState => _lastState;

  /// Attaches to [WidgetsBinding.instance]. The constructors call this
  /// unless `autoStart` is false, so an observer can be built before the
  /// binding exists and started later. Does nothing if already started or
  /// disposed.
  void start() {
    if (_disposed || _listener != null) return;
    _listener = AppLifecycleListener(onStateChange: _onStateChange);
  }

  void _onStateChange(AppLifecycleState state) {
    _lastState = state;
    final cancel = switch (state) {
      AppLifecycleState.paused => _cancelScanOnPause,
      AppLifecycleState.detached => _cancelScanOnDetach,
      _ => false,
    };
    if (!cancel) return;
    try {
      _cancelScan();
    } on StateError {
      // The wallet is closed, so there is no scan to cancel.
    } catch (error) {
      _onError?.call(error);
    }
  }

  /// Stops listening. Safe to call more than once.
  void dispose() {
    _disposed = true;
    _listener?.dispose();
    _listener = null;
  }
}
