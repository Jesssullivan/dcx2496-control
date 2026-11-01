#!/usr/bin/env bash
set -euo pipefail

# TIN-6964: injected capture and receipt fixtures, never a serial device.
if [[ $# != 1 || ${1##*/} != Package.swift ]]; then
  printf 'usage: run_room_verification.sh PACKAGE_SWIFT\n' >&2
  exit 64
fi
if [[ $(/usr/bin/uname -s) != Darwin ]]; then
  printf 'room verification fixtures require the Darwin native lane\n' >&2
  exit 78
fi
if [[ -z ${TEST_TMPDIR:-} || $TEST_TMPDIR != /* || ! -d $TEST_TMPDIR || ! -w $TEST_TMPDIR ]]; then
  printf 'room verification requires an owned writable TEST_TMPDIR\n' >&2
  exit 78
fi
package_path="$(cd "$(dirname "$1")" && pwd -P)"
umask 077
owned_root="$(mktemp -d "$TEST_TMPDIR/dcx-room-verification.XXXXXX")"
xcode_developer_dir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p)"
swift_binary="$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift"
test -x "$swift_binary"
exec /usr/bin/env -u SDKROOT DEVELOPER_DIR="$xcode_developer_dir" \
  "$swift_binary" test --package-path "$package_path" --scratch-path "$owned_root/swift-build" \
  --jobs 1 --filter RoomVerificationTests
