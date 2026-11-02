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
