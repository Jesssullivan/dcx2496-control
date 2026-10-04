#!/usr/bin/env bash
set -euo pipefail

# dec-native-studio-20261004 / dec-local-first-reapi-20261004 / R-N13:
# Fixed child executable is test-only, under owned TEST_TMPDIR. No dcxctl,
# serial device, App Group, CoreMIDI endpoint, or audio is invoked.
if [[ $# != 3 ]]; then
  printf 'usage: run_process_runner.sh PACKAGE_SWIFT PROCESS_TESTS FIXTURE_C\n' >&2
  exit 64
fi
if [[ $(/usr/bin/uname -s) != Darwin ]]; then
  printf 'real child fixtures require the admitted Darwin native lane\n' >&2
  exit 78
fi
for declared_input in "$@"; do
  if [[ ! -s "$declared_input" ]]; then
    printf 'missing declared real child fixture input: %s\n' "$declared_input" >&2
    exit 78
  fi
done
if [[ ${1##*/} != Package.swift || ${2##*/} != DCXCTLProcessRunnerTests.swift ||
  ${3##*/} != DCXChildFixture.c ]]; then
  printf 'real child fixtures require the exact declared inputs\n' >&2
  exit 78
fi
if [[ -z ${TEST_TMPDIR:-} || $TEST_TMPDIR != /* || ! -d $TEST_TMPDIR || ! -w $TEST_TMPDIR ]]; then
  printf 'real child fixtures require an absolute writable owned TEST_TMPDIR\n' >&2
  exit 78
fi
package_path="$(cd "$(dirname "$1")" && pwd -P)"
test_directory="$(cd "$(dirname "$2")" && pwd -P)"
fixture_directory="$(cd "$(dirname "$3")" && pwd -P)"
if [[ "$test_directory" != "$package_path/Tests/DCXLogicHelperCoreTests" ||
  "$fixture_directory" != "$test_directory/Fixtures" ]]; then
  printf 'real child fixture inputs must belong to the declared Swift package\n' >&2
  exit 78
fi
umask 077
owned_root="$(mktemp -d "$TEST_TMPDIR/dcx-real-child.XXXXXX")"
owned_root="$(cd "$owned_root" && pwd -P)"
fixture_binary="$owned_root/dcx-child-fixture"
xcode_developer_dir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p)"
swift_binary="$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift"
clang_binary="$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang"
if [[ ! -x "$swift_binary" || ! -x "$clang_binary" ]]; then
  printf 'real child fixtures require the selected Xcode Swift and Clang toolchains\n' >&2
  exit 78
fi
sdk_path="$(/usr/bin/env -u SDKROOT DEVELOPER_DIR="$xcode_developer_dir" \
  /usr/bin/xcrun --sdk macosx --show-sdk-path)"
/usr/bin/env -u SDKROOT DEVELOPER_DIR="$xcode_developer_dir" \
  "$clang_binary" -std=c11 -Wall -Wextra -Werror -isysroot "$sdk_path" "$3" -o "$fixture_binary"
chmod 700 "$fixture_binary"
exec /usr/bin/env -u SDKROOT DEVELOPER_DIR="$xcode_developer_dir" \
  TEST_TMPDIR="$owned_root" DCX_PROCESS_FIXTURE_EXECUTABLE="$fixture_binary" "$swift_binary" \
  test --package-path "$package_path" --scratch-path "$owned_root/swift-build" \
  --jobs 1 --filter 'DCXCTLProcessRunnerTests|MutationRecovery(Coordinator|LeaseStore)Tests'
