#!/usr/bin/env bash
# Release builds of grin-ffi for the Apple targets with the host toolchain
# (rustup and Xcode), and the symbol gate on each: 35 exported grin_
# functions and no grin_test_ ones. This is what the hook's cargo mode does
# on a Mac. Run on macOS:
#
#   tool/verify-apple.sh [path-to-grin-wallet-clone]
#
# The fork path is only needed while the fork is not pushed; it feeds
# tool/local-fork-env.sh. With FLUTTER=1 the example is also built for
# macOS. With KIT_LOCAL=<path to a native-build-kit clone> and Nix on PATH
# the flake's Apple shell builds the reference dylibs too (dev mode, any
# Xcode).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd -P)
fork=${1:-${GRIN_WALLET_LOCAL:-}}
if [ -n "$fork" ]; then
  # shellcheck source=local-fork-env.sh
  GRIN_WALLET_LOCAL=$fork source "$root/tool/local-fork-env.sh"
fi
export GIT_SSH_COMMAND=/bin/false

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

if [ -n "${KIT_LOCAL:-}" ] && command -v nix >/dev/null; then
  echo "== nix apple shell (dev mode, any Xcode)"
  out="$root/build/apple"
  mkdir -p "$out"
  (cd "$root" && GRIN_FFI_ANY_XCODE=1 nix develop "git+file://$root?dir=packages/grin#apple" \
      --override-input kit "git+file://$KIT_LOCAL?ref=feat/native-build-kit" \
      --override-input grin-wallet "git+file://$fork?ref=optional-tor-rustls&submodules=1" \
      --no-write-lock-file --command "$root/packages/grin/nix/build-apple.sh" "$out") || { echo "NIX_APPLE_FAIL"; fail=1; }
  for d in "$out"/*/; do
    [ -f "$d/libgrin_ffi.dylib" ] || continue
    std=$(nm -gU "$d/libgrin_ffi.dylib" | awk '{print $3}' | grep -c '^_grin_' || true)
    test=$(nm -gU "$d/libgrin_ffi.dylib" | awk '{print $3}' | grep -c '^_grin_test_' || true)
    echo "nm nix $(basename "$d"): $std standard, $test test symbols"
  done
fi

if [ "$fail" = 0 ]; then echo "VERIFY_APPLE_OK"; else echo "VERIFY_APPLE_FAIL"; exit 1; fi
