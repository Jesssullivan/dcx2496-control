#!/usr/bin/env bash
set -euo pipefail

# TIN-6964: build library modules and typecheck the containing SwiftUI source.
# No Cargo embed, app launch, executable carrier, signature or install claim.
if [[ $# != 3 || ${1##*/} != Package.swift || ${2##*/} != DCXLogicHelperApp.swift || ${3##*/} != Info.plist ]]; then
  printf 'usage: run_helper_ui_typecheck.sh PACKAGE_SWIFT HELPER_APP_SWIFT HELPER_PLIST\n' >&2
  exit 64
fi
if [[ $(/usr/bin/uname -s) != Darwin || $(/usr/bin/uname -m) != arm64 ]]; then
  printf 'helper UI typecheck requires the admitted arm64 Darwin lane\n' >&2
  exit 78
fi
for declared_input in "$@"; do
  test -s "$declared_input"
done
package_path="$(cd "$(dirname "$1")" && pwd -P)"
app_source="$(cd "$(dirname "$2")" && pwd -P)/${2##*/}"
app_plist="$(cd "$(dirname "$3")" && pwd -P)/${3##*/}"
if [[ "$app_source" != "$package_path/DCXLogicHelper/DCXLogicHelperApp.swift" || "$app_plist" != "$package_path/DCXLogicHelper/Info.plist" ]]; then
  printf 'helper inputs must belong to the declared package\n' >&2
  exit 78
fi
scratch_parent="${TEST_TMPDIR:-${TMPDIR:-/tmp}}"
test -d "$scratch_parent"
test -w "$scratch_parent"
umask 077
owned_root="$(mktemp -d "$scratch_parent/dcx-helper-ui-typecheck.XXXXXX")"
trap 'rm -rf "$owned_root"' EXIT
xcode_developer_dir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p)"
toolchain="$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin"
test -x "$toolchain/swift"
test -x "$toolchain/swiftc"
sdk="$(/usr/bin/env -u SDKROOT DEVELOPER_DIR="$xcode_developer_dir" /usr/bin/xcrun --sdk macosx --show-sdk-path)"
/usr/bin/env -u SDKROOT DEVELOPER_DIR="$xcode_developer_dir" \
  "$toolchain/swift" build --package-path "$package_path" --scratch-path "$owned_root/swift-build" --jobs 1
bin_path="$(/usr/bin/env -u SDKROOT DEVELOPER_DIR="$xcode_developer_dir" \
  "$toolchain/swift" build --package-path "$package_path" --scratch-path "$owned_root/swift-build" --show-bin-path)"
/usr/bin/env -u SDKROOT DEVELOPER_DIR="$xcode_developer_dir" \
  "$toolchain/swiftc" -parse-as-library -typecheck -target arm64-apple-macosx14.0 \
  -sdk "$sdk" -I "$bin_path/Modules" "$app_source"
/usr/bin/plutil -lint "$app_plist"
printf 'DCX_HELPER_UI_TYPECHECK=pass NO-CARRIER NO-INSTALL NO-DEVICE\n'
