#!/usr/bin/env bash
set -euo pipefail

# dec-local-first-reapi-20261004 / R-N13: exercise refusal before tool dispatch.
# Uses host Bash/GNU realpath; it establishes no build, REAPI or device receipt.
wrapper=${1:-scripts/run_bazelisk.sh}
wrapper=$(cd "$(dirname "$wrapper")" && pwd -P)/$(basename "$wrapper")
unset DCX_BAZELISK DCX_REAPI_EXECUTOR DCX_REAPI_CONFIG DCX_BAZEL_OUTPUT_USER_ROOT

reject() {
  local expected_status=$1 expected_message=$2 output status
  shift 2
  if output=$(bash "$wrapper" "$@" 2>&1); then
    printf 'unexpected successful dispatch: %s\n' "$*" >&2
    exit 1
  else
    status=$?
  fi
  if [[ "$status" != "$expected_status" || "$output" != *"$expected_message"* ]]; then
    printf 'unexpected refusal: status=%s output=%s\n' "$status" "$output" >&2
    exit 1
  fi
}

reject 64 'COMMAND must not begin with -' --batch build //:dcxctl
reject 64 'COMMAND must not begin with -' --ignore_all_rc_files build //:dcxctl
reject 78 'DCX_BAZELISK is unavailable' version
reject 78 'REAPI requires' --reapi test //:remote_eligible

# An output root off the boot disk must be an absolute writable directory.
export DCX_BAZEL_OUTPUT_USER_ROOT=relative-output-root
reject 78 'DCX_BAZEL_OUTPUT_USER_ROOT must name' version
export DCX_BAZEL_OUTPUT_USER_ROOT=/nonexistent/dcx-output-root
reject 78 'DCX_BAZEL_OUTPUT_USER_ROOT must name' version
unset DCX_BAZEL_OUTPUT_USER_ROOT

# Every synthetic external input below must refuse before a pinned tool runs.
export DCX_REAPI_CONFIG="$wrapper" DCX_REAPI_EXECUTOR=https://synthetic.invalid
reject 78 'must use grpc or grpcs' --reapi test //:remote_eligible
export DCX_REAPI_EXECUTOR=grpcs://identity@synthetic.invalid
reject 78 'must not contain credentials' --reapi test //:remote_eligible
export DCX_REAPI_CONFIG=relative-operator.rc DCX_REAPI_EXECUTOR=grpcs://synthetic.invalid
reject 78 'must name an absolute operator rc file' --reapi test //:remote_eligible

# An existing ordinary source file is never a Nix-store Bazelisk binding.
export DCX_BAZELISK="$wrapper"
reject 78 'outside the exact Nix development-shell binding' version
printf 'BAZEL_FRONTDOOR=refusal-cases-passed NO-BUILD NO-DEVICE\n'
