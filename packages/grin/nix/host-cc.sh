#!/bin/sh
# Compiler for the kit's fixed fake host on Apple.
#
# The kit's rustc wrapper reports x86_64-unknown-linux-musl as cargo's host
# so builds are independent of the build machine, and build scripts of host
# crates therefore see TARGET=x86_64-unknown-linux-musl. On Apple the host
# artifacts really are built for the Mac, and cc-rs would otherwise hand
# Xcode's clang a Linux target it has no headers for. Drop the target
# flags and compile natively with the system compiler; everything else is
# passed through. Selected through CC_x86_64_unknown_linux_musl (and the
# CXX variant) in flake.nix, for the Apple shell only.
case "$0" in
  *++*|*cxx*) cc=/usr/bin/c++ ;;
  *) cc=/usr/bin/cc ;;
esac
skip=0
set -- "$@" --end-of-args--
while [ "$1" != --end-of-args-- ]; do
  a=$1; shift
  if [ "$skip" = 1 ]; then skip=0; continue; fi
  case "$a" in
    --target=*) ;;
    -target|--target) skip=1 ;;
    *) set -- "$@" "$a" ;;
  esac
done
shift
exec "$cc" "$@"
