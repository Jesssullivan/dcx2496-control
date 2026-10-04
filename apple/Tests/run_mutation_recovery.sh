#!/usr/bin/env bash
set -euo pipefail

# dec-native-studio-20261004 / dec-local-first-reapi-20261004 / R-N13:
# The lab-owned Darwin lane supplies Xcode. Fixtures use owned lease stores
# beneath Bazel's TEST_TMPDIR and injected backends; no dcxctl is executed.
if [[ $# != 3 ]]; then
  printf 'usage: run_mutation_recovery.sh PACKAGE_SWIFT COORDINATOR_TESTS LEASE_STORE_TESTS\n' >&2
  exit 64
fi
if [[ $(/usr/bin/uname -s) != Darwin ]]; then
  printf 'mutation recovery fixtures require the admitted Darwin native lane\n' >&2
  exit 78
fi
for declared_input in "$@"; do
  if [[ ! -s "$declared_input" ]]; then
    printf 'missing declared mutation recovery input: %s\n' "$declared_input" >&2
    exit 78
  fi
done
if [[ ${1##*/} != Package.swift ||
  ${2##*/} != MutationRecoveryCoordinatorTests.swift ||
  ${3##*/} != MutationRecoveryLeaseStoreTests.swift ]]; then
  printf 'mutation recovery requires the exact declared package and suites\n' >&2
  exit 78
fi
if [[ -z ${TEST_TMPDIR:-} || $TEST_TMPDIR != /* ||
  ! -d $TEST_TMPDIR || ! -w $TEST_TMPDIR ]]; then
  printf 'mutation recovery requires an absolute writable owned TEST_TMPDIR\n' >&2
  exit 78
fi

package_path="$(cd "$(dirname "$1")" && pwd -P)"
for declared_suite in "$2" "$3"; do
  suite_directory="$(cd "$(dirname "$declared_suite")" && pwd -P)"
  if [[ "$suite_directory" != "$package_path/Tests/DCXLogicHelperCoreTests" ]]; then
    printf 'declared recovery suites must belong to the declared Swift package\n' >&2
    exit 78
  fi
done

umask 077
owned_root="$(mktemp -d "$TEST_TMPDIR/dcx-mutation-recovery.XXXXXX")"
fixture_directory="$owned_root/recovery-fixtures"
mkdir "$fixture_directory"
xcode_developer_dir="$(unset DEVELOPER_DIR; /usr/bin/xcode-select -p)"
swift_binary="$xcode_developer_dir/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift"
if [[ ! -x "$swift_binary" ]]; then
  printf 'mutation recovery requires the selected Xcode Swift toolchain\n' >&2
  exit 78
fi
exec /usr/bin/env -u SDKROOT DEVELOPER_DIR="$xcode_developer_dir" \
  TEST_TMPDIR="$fixture_directory" "$swift_binary" \
  test --package-path "$package_path" --scratch-path "$owned_root/swift-build" \
  --jobs 1 --filter 'MutationRecovery(Coordinator|LeaseStore)Tests'
