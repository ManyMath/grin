import 'dart:convert';

/// Decodes [text] as a JSON object, or throws a [FormatException].
Map<String, Object?> decodeJsonObject(String? text) {
  if (text == null) {
    throw const FormatException('the native call returned no data');
  }
  final Object? decoded = jsonDecode(text);
  if (decoded is! Map<String, Object?>) {
    throw const FormatException('expected a JSON object');
  }
  return decoded;
}

/// Reads an unsigned integer amount (nanogrin) as a [BigInt].
///
/// Accepts a JSON integer, or a decimal string for values the JSON decoder
/// could not hold in a 64-bit integer.
BigInt readBigInt(Object? value, String key) {
  if (value is int) return BigInt.from(value);
  if (value is String) {
    final parsed = BigInt.tryParse(value);
    if (parsed != null) return parsed;
  }
  throw FormatException('"$key" is not an integer', value);
}

/// Reads a JSON integer that fits an [int].
int readInt(Object? value, String key) {
  if (value is int) return value;
  throw FormatException('"$key" is not an integer', value);
}

/// Reads a JSON boolean.
bool readBool(Object? value, String key) {
  if (value is bool) return value;
  throw FormatException('"$key" is not a boolean', value);
}

/// Reads a JSON string.
String readString(Object? value, String key) {
  if (value is String) return value;
  throw FormatException('"$key" is not a string', value);
}

/// Reads a JSON string that may be null.
String? readOptionalString(Object? value, String key) {
  if (value == null) return null;
  return readString(value, key);
}

/// Reads a JSON integer that may be null.
int? readOptionalInt(Object? value, String key) {
  if (value == null) return null;
  return readInt(value, key);
}

/// Reads an unsigned integer amount that may be null.
BigInt? readOptionalBigInt(Object? value, String key) {
  if (value == null) return null;
  return readBigInt(value, key);
}

/// Reads a JSON array of strings.
List<String> readStringList(Object? value, String key) {
  if (value is List<Object?>) {
    return List<String>.unmodifiable(
      value.map((Object? item) => readString(item, key)),
    );
  }
  throw FormatException('"$key" is not an array', value);
}

/// Reads a JSON object.
Map<String, Object?> readObject(Object? value, String key) {
  if (value is Map<String, Object?>) return value;
  throw FormatException('"$key" is not an object', value);
}

/// Reads an RFC 3339 timestamp as a UTC [DateTime].
DateTime readDateTime(Object? value, String key) {
  final text = readString(value, key);
  final parsed = DateTime.tryParse(text);
  if (parsed == null) {
    throw FormatException('"$key" is not an RFC 3339 timestamp', value);
  }
  return parsed.toUtc();
}

/// Reads an RFC 3339 timestamp that may be null.
DateTime? readOptionalDateTime(Object? value, String key) {
  if (value == null) return null;
  return readDateTime(value, key);
}

/// Whether two lists hold equal elements in the same order.
bool listsEqual<T>(List<T> a, List<T> b) {
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i] != b[i]) return false;
  }
  return true;
}

/// The largest amount the native calls take as an `int`.
///
/// Amounts are `u64` natively, but a Dart `int` is signed, so values from
/// 2^63 up would wrap. No real balance gets near it (about 9.2e18 nanogrin).
final BigInt _maxNativeAmount = BigInt.parse('9223372036854775807');

/// Checks that [amount] fits the native `u64` argument and returns it as an
/// `int`.
///
/// Throws [ArgumentError] for a negative value and for one at or above 2^63.
int amountToNative(BigInt amount, [String name = 'amount']) {
  if (amount.isNegative || amount > _maxNativeAmount) {
    throw ArgumentError.value(
      amount,
      name,
      'must be between 0 and 9223372036854775807 nanogrin',
    );
  }
  return amount.toUnsigned(64).toInt();
}
