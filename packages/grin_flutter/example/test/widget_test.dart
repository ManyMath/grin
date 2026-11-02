import 'package:flutter_test/flutter_test.dart';
import 'package:grin_flutter/grin_flutter.dart';

// The app itself needs the native library, which `flutter test` does not
// load. integration_test/app_test.dart covers it on a real device.
void main() {
  test('grin_flutter re-exports the grin API', () {
    expect(GrinNetwork.testnet.value, 1);
    expect(formatGrin(BigInt.from(1500000000)), '1.5');
  });
}
