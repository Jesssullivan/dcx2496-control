#!/usr/bin/env bash
set -euo pipefail

package_path="$(dirname "$1")"
xcode_developer_dir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p)"
exec /usr/bin/env -u SDKROOT DEVELOPER_DIR="$xcode_developer_dir" \
  "$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift" \
  test --package-path "$package_path" --scratch-path "$TEST_TMPDIR/swift-build" \
  --jobs 1 --filter ChildFailureDiagnosticsTests
