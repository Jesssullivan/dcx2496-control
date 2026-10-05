#!/usr/bin/env bash
set -euo pipefail

# dec-native-studio-20261004 / dec-local-first-reapi-20261004 / R-N13:
# Pure AU state/admission and in-memory render fixtures. No App Group socket,
# CoreMIDI endpoint, child executable, serial, installation, or audio output.
if [[ $# != 3 ]]; then
  printf 'usage: run_logic_recall.sh PACKAGE_SWIFT ADMISSION_TESTS AU_TESTS\n' >&2
  exit 64
fi
if [[ $(/usr/bin/uname -s) != Darwin ]]; then
  printf 'Logic recall fixtures require the admitted Darwin native lane\n' >&2
  exit 78
fi
for declared_input in "$@"; do
  if [[ ! -s "$declared_input" ]]; then
    printf 'missing declared Logic recall input: %s\n' "$declared_input" >&2
    exit 78
  fi
done
if [[ ${1##*/} != Package.swift || ${2##*/} != DCXApplyRequestAdmissionTests.swift ||
  ${3##*/} != DCXControlAudioUnitTests.swift ]]; then
  printf 'Logic recall requires the exact declared package and suites\n' >&2
  exit 78
fi
if [[ -z ${TEST_TMPDIR:-} || $TEST_TMPDIR != /* || ! -d $TEST_TMPDIR || ! -w $TEST_TMPDIR ]]; then
  printf 'Logic recall requires an absolute writable owned TEST_TMPDIR\n' >&2
  exit 78
fi
package_path="$(cd "$(dirname "$1")" && pwd -P)"
for declared_suite in "$2" "$3"; do
  suite_directory="$(cd "$(dirname "$declared_suite")" && pwd -P)"
  if [[ "$suite_directory" != "$package_path/Tests/DCXControlAUCoreTests" ]]; then
    printf 'declared Logic recall suites must belong to the declared Swift package\n' >&2
    exit 78
  fi
done
umask 077
owned_root="$(mktemp -d "$TEST_TMPDIR/dcx-logic-recall.XXXXXX")"
xcode_developer_dir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p)"
swift_binary="$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift"
if [[ ! -x "$swift_binary" ]]; then
  printf 'Logic recall requires the selected Xcode Swift toolchain\n' >&2
  exit 78
fi
exec /usr/bin/env -u SDKROOT DEVELOPER_DIR="$xcode_developer_dir" \
  "$swift_binary" test --package-path "$package_path" --scratch-path "$owned_root/swift-build" \
  --jobs 1 --filter 'DCX(ApplyRequestAdmission|ControlAudioUnit)Tests'
