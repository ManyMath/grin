import '../models/grin_scan_result.dart';

/// Something that happened to a [GrinWalletClient]. Listen with
/// `GrinWalletClient.events`.
sealed class GrinWalletEvent {
  const GrinWalletEvent();
}

/// A scan began to run. Operations are queued, so this comes when the scan
/// reaches the front of the queue, not when `scan` was called.
final class GrinScanStarted extends GrinWalletEvent {
  /// Creates the event.
  const GrinScanStarted({required this.startHeight});

  /// The height the scan was asked to start from.
  final int startHeight;

  @override
  String toString() => 'GrinScanStarted(startHeight: $startHeight)';
}

/// A scan ended, successfully or not.
final class GrinScanFinished extends GrinWalletEvent {
  /// Creates the event.
  const GrinScanFinished({this.result, this.error});

  /// The result, or null when the scan failed.
  final GrinScanResult? result;

  /// The error, or null when the scan succeeded.
  final Object? error;

  @override
  String toString() => 'GrinScanFinished(result: $result, error: $error)';
}

/// The wallet was closed. It is the last event of the stream.
final class GrinWalletClosed extends GrinWalletEvent {
  /// Creates the event.
  const GrinWalletClosed();

  @override
  String toString() => 'GrinWalletClosed()';
}
