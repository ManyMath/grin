#!/usr/bin/env bash
# Release builds of grin-ffi for the Apple targets with the host toolchain
# (rustup and Xcode), and the symbol gate on each: 35 exported grin_
# functions and no grin_test_ ones. This is what the build hook does on a
# Mac. Run on macOS:
#
#   tool/verify-apple.sh
#
# With FLUTTER=1 the example app is also built for macOS through the hook.
# Fetching the grin-wallet fork needs ssh read access to git.cypherstack.com
# (or the fork's rev already in ~/.cargo/git/db).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd -P)
# The deployment floors the hook also passes. Without them rustc links for
# iOS 10 while Xcode's clang compiles the C code (secp256k1-zkp, lmdb) for
# the SDK's own version, and the iOS 10 runtime lacks ___chkstk_darwin.
export IPHONEOS_DEPLOYMENT_TARGET=13.0
export MACOSX_DEPLOYMENT_TARGET=11.0
export CARGO_NET_GIT_FETCH_WITH_CLI=true

echo "== host"; sw_vers 2>/dev/null || true; xcodebuild -version 2>/dev/null | head -2 || true; rustc --version; cargo --version

cd "$root/packages/grin/rust"
fail=0
for t in aarch64-apple-darwin x86_64-apple-darwin aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios; do
  echo "== cargo build --release --target $t"
  if ! cargo build -p grin-ffi --release --locked --target "$t"; then
    echo "BUILD_FAIL $t"; fail=1; continue
  fi
  lib="target/$t/release/libgrin_ffi.dylib"
  std=$(nm -gU "$lib" | awk '{print $3}' | grep -c '^_grin_' || true)
  test=$(nm -gU "$lib" | awk '{print $3}' | grep -c '^_grin_test_' || true)
  echo "nm $t: $std standard, $test test symbols"
  if [ "$std" != 35 ] || [ "$test" != 0 ]; then echo "NM_FAIL $t"; fail=1; fi
done

if [ "${FLUTTER:-0}" = 1 ] && command -v flutter >/dev/null; then
  echo "== flutter build macos (example)"
  (cd "$root" && dart pub get >/dev/null && cd packages/grin_flutter/example && flutter build macos) || { echo "FLUTTER_FAIL"; fail=1; }
fi

if [ "$fail" = 0 ]; then echo "VERIFY_APPLE_OK"; else echo "VERIFY_APPLE_FAIL"; exit 1; fi
