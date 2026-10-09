#!/usr/bin/env bash
set -euo pipefail

# Bridge v2 feedback-notch surface: v2 profile/diff/plan bindings, helper
# offline planning over exact dcxctl fixtures, AU staging and recall, and one
# end to end run of the declared dcxctl's offline feedback subcommands. No
# socket, tty, serial, audio, or device operation.
# dec-native-studio-20261004 / dec-local-first-reapi-20261004 / R-N13.
if [[ $# != 2 ]]; then
  printf 'usage: run_feedback_notch.sh PACKAGE_SWIFT DCXCTL\n' >&2
  exit 64
fi
if [[ $(/usr/bin/uname -s) != Darwin ]]; then
  printf 'feedback notch fixtures require the admitted Darwin native lane\n' >&2
  exit 78
fi
if [[ ${1##*/} != Package.swift || ! -s $1 ]]; then
  printf 'feedback notch suite requires the exact declared Swift package\n' >&2
  exit 78
fi
if [[ ! -x $2 ]]; then
  printf 'feedback notch suite requires the declared dcxctl executable\n' >&2
  exit 78
fi
if [[ -z ${TEST_TMPDIR:-} || $TEST_TMPDIR != /* || ! -d $TEST_TMPDIR || ! -w $TEST_TMPDIR ]]; then
  printf 'feedback notch suite requires an absolute writable owned TEST_TMPDIR\n' >&2
  exit 78
fi
package_path="$(cd "$(dirname "$1")" && pwd -P)"
dcxctl="$(cd "$(dirname "$2")" && pwd -P)/${2##*/}"
umask 077
owned_root="$(mktemp -d "$TEST_TMPDIR/dcx-feedback-notch.XXXXXX")"
xcode_developer_dir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p)"
swift_binary="$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift"
if [[ ! -x "$swift_binary" ]]; then
  printf 'feedback notch suite requires the selected Xcode Swift toolchain\n' >&2
  exit 78
fi
exec /usr/bin/env -u SDKROOT DEVELOPER_DIR="$xcode_developer_dir" DCX_TEST_DCXCTL="$dcxctl" \
  "$swift_binary" test --package-path "$package_path" --scratch-path "$owned_root/swift-build" \
  --jobs 1 --filter 'FeedbackNotch|DCXControlNotchState|BridgeResponseBinding'
