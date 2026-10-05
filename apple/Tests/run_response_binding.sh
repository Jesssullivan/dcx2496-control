#!/usr/bin/env bash
set -euo pipefail

# Pure request/reply correlation; no socket, helper, serial or device operation.
# dec-native-studio-20261004 / dec-local-first-reapi-20261004 / R-N13.
if [[ $# != 2 ]]; then
  printf 'usage: run_response_binding.sh PACKAGE_SWIFT BINDING_TESTS\n' >&2
  exit 64
fi
if [[ $(/usr/bin/uname -s) != Darwin ]]; then
  printf 'bridge correlation fixtures require the admitted Darwin native lane\n' >&2
  exit 78
fi
for declared_input in "$@"; do
  if [[ ! -s "$declared_input" ]]; then
    printf 'missing declared bridge correlation input: %s\n' "$declared_input" >&2
    exit 78
  fi
done
if [[ ${1##*/} != Package.swift || ${2##*/} != BridgeResponseBindingTests.swift ]]; then
  printf 'bridge correlation requires the exact declared package and suite\n' >&2
  exit 78
fi
if [[ -z ${TEST_TMPDIR:-} || $TEST_TMPDIR != /* || ! -d $TEST_TMPDIR || ! -w $TEST_TMPDIR ]]; then
  printf 'bridge correlation requires an absolute writable owned TEST_TMPDIR\n' >&2
  exit 78
fi
package_path="$(cd "$(dirname "$1")" && pwd -P)"
suite_directory="$(cd "$(dirname "$2")" && pwd -P)"
if [[ "$suite_directory" != "$package_path/Tests/DCXLogicBridgeTests" ]]; then
  printf 'declared correlation suite must belong to the declared Swift package\n' >&2
  exit 78
fi
umask 077
owned_root="$(mktemp -d "$TEST_TMPDIR/dcx-response-binding.XXXXXX")"
xcode_developer_dir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p)"
swift_binary="$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift"
if [[ ! -x "$swift_binary" ]]; then
  printf 'bridge correlation requires the selected Xcode Swift toolchain\n' >&2
  exit 78
fi
exec /usr/bin/env -u SDKROOT DEVELOPER_DIR="$xcode_developer_dir" \
  "$swift_binary" test --package-path "$package_path" --scratch-path "$owned_root/swift-build" \
  --jobs 1 --filter BridgeResponseBindingTests
