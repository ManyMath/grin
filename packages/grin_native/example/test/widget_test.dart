import 'package:flutter_test/flutter_test.dart';
import 'package:grin_native/grin_native.dart';

// The app itself needs the native library, which `flutter test` does not
// load. integration_test/app_test.dart covers it on a real device.
void main() {
  test('grin_native re-exports the grin API', () {
    expect(GrinNetwork.testnet.value, 1);
    expect(formatGrin(BigInt.from(1500000000)), '1.5');
  });
}
