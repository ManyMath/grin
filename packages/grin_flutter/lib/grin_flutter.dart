/// Flutter glue for package:grin.
///
/// This package re-exports `package:grin/grin.dart` and adds a default wallet
/// directory ([GrinFlutter]) and an app lifecycle observer
/// ([GrinLifecycleObserver]). The native library itself comes from the build
/// hook of package:grin.
library;

export 'package:grin/grin.dart';

export 'src/default_paths.dart';
export 'src/lifecycle_observer.dart';
