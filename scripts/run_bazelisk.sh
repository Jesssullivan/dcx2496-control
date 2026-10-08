#!/usr/bin/env bash
set -euo pipefail

# dec-local-first-reapi-20261004: the pinned binding grants no host-placement
# authority. Host admission and native/device proof remain separate.
lane=local
if [[ "${1:-}" == --reapi ]]; then
  lane=reapi
  shift
fi
command="${1:-}"
if [[ -z "$command" ]]; then
  printf 'usage: run_bazelisk.sh [--reapi] COMMAND [ARGS...]\n' >&2
  exit 64
fi
if [[ "$command" == -* ]]; then
  printf 'Bazel COMMAND must not begin with -\n' >&2
  exit 64
fi
shift

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
startup=(--nosystem_rc --nohome_rc --noworkspace_rc "--bazelrc=$repo_root/.bazelrc")
# Bazel's default output root lives on the boot disk (/private/var/tmp), and the
# native Apple tests put SwiftPM scratch under TEST_TMPDIR inside it. On a host
# whose boot disk is near full, that write pressure ended the 2026-10-06 PZM
# //apple:logic_recall run. An operator may move the whole output root (output
# bases, repository cache, TEST_TMPDIR, SwiftPM scratch) onto another volume.
if [[ -n "${DCX_BAZEL_OUTPUT_USER_ROOT:-}" ]]; then
  if [[ "$DCX_BAZEL_OUTPUT_USER_ROOT" != /* ||
    "$DCX_BAZEL_OUTPUT_USER_ROOT" == *[[:space:]]* ||
    ! -d "$DCX_BAZEL_OUTPUT_USER_ROOT" || ! -w "$DCX_BAZEL_OUTPUT_USER_ROOT" ]]; then
    printf 'DCX_BAZEL_OUTPUT_USER_ROOT must name an absolute writable directory without spaces\n' >&2
    exit 78
  fi
  startup+=("--output_user_root=$DCX_BAZEL_OUTPUT_USER_ROOT")
fi
execution=()
if [[ "$lane" == reapi ]]; then
  case "$command" in
    build | test | coverage) ;;
    *) printf 'REAPI supports only build, test and coverage\n' >&2; exit 64 ;;
  esac
  if [[ -z "${DCX_REAPI_EXECUTOR:-}" || -z "${DCX_REAPI_CONFIG:-}" ]]; then
    printf 'REAPI requires DCX_REAPI_EXECUTOR and an external DCX_REAPI_CONFIG rc file\n' >&2
    exit 78
  fi
  if [[ "$DCX_REAPI_CONFIG" != /* || ! -f "$DCX_REAPI_CONFIG" ]]; then
    printf 'DCX_REAPI_CONFIG must name an absolute operator rc file\n' >&2
    exit 78
  fi
  case "$DCX_REAPI_EXECUTOR" in
    grpc://?* | grpcs://?*) ;;
    *) printf 'DCX_REAPI_EXECUTOR must use grpc or grpcs\n' >&2; exit 78 ;;
  esac
  if [[ "$DCX_REAPI_EXECUTOR" == *'@'* ||
    "$DCX_REAPI_EXECUTOR" == *'?'* ||
    "$DCX_REAPI_EXECUTOR" == *'#'* ||
    "$DCX_REAPI_EXECUTOR" == *[[:space:]]* ]]; then
    printf 'REAPI endpoint must not contain credentials, query or fragment material\n' >&2
    exit 78
  fi
  startup+=("--bazelrc=$DCX_REAPI_CONFIG")
  execution+=(--config=reapi "--remote_executor=$DCX_REAPI_EXECUTOR"
    --spawn_strategy=remote '--strategy_regexp=.*=remote'
    --strategy=TestRunner=remote --remote_local_fallback=false --jobs=2)
  if [[ "$command" == test || "$command" == coverage ]]; then
    execution+=(--test_strategy=standalone
      '--test_tag_filters=-no-remote,-no-remote-exec,-local,-exclusive')
  fi
else
  case "$command" in
    build | test | coverage | run)
      execution+=(--config=local --remote_executor= --remote_cache=
        --spawn_strategy=local '--strategy_regexp=.*=local'
        --strategy=TestRunner=local --jobs=2)
      if [[ "$command" == test || "$command" == coverage ]]; then
        execution+=(--test_strategy=standalone --local_test_jobs=2)
      fi
      ;;
  esac
fi

candidate="${DCX_BAZELISK:-}"
if [[ -z "$candidate" ]]; then
  printf 'DCX_BAZELISK is unavailable; enter the pinned Nix development shell\n' >&2
  exit 78
fi

# Resolving the binding needs a resolver, and a missing one says nothing about
# whether the binding is the pinned one. Report it as its own failure so the two
# conditions can never be read as each other. The pinned shell supplies GNU
# realpath; `-e` makes a path that does not exist a resolution failure too.
if ! command -v realpath >/dev/null 2>&1; then
  printf 'realpath is unavailable to resolve DCX_BAZELISK; enter the pinned Nix development shell\n' >&2
  exit 78
fi

# A path that will not resolve is not the pinned binding, so it joins the
# rejection below rather than earning a third diagnostic.
if ! resolved="$(realpath -e -- "$candidate" 2>/dev/null)"; then
  resolved=""
fi

# Anchored on purpose. A `case` glob's `*` spans `/`, so `/nix/store/*-bazelisk-*`
# would also accept `/nix/store/a/b-bazelisk-1/bin/bazelisk`. A store path is
# exactly one component under /nix/store, and the only suffix a binding carries
# is its version.
binding_pattern='^/nix/store/[^/]+-bazelisk(-[0-9][^/]*)?/bin/bazelisk$'
if [[ ! "$resolved" =~ $binding_pattern ]]; then
  printf 'refusing bazelisk outside the exact Nix development-shell binding\n' >&2
  exit 78
fi

# toolchains_llvm resolves the macOS sysroot once, at fetch time, by running
# `/usr/bin/xcrun --show-sdk-path --sdk macosx`, and bakes the absolute answer
# into @llvm_toolchain. Inside this shell DEVELOPER_DIR and SDKROOT point at
# nixpkgs' apple-sdk, whose SDK ships no `libc++.tbd`, so every later Darwin
# link fails. Scrub both here, at the one place that owns which Bazel runs, so
# the probe reads the Xcode SDK the linker needs -- the same `unset
# DEVELOPER_DIR` the apple recipes in just/repo.just already depend on.
#
# Two limits, said out loud: this governs the first fetch into an output base,
# because these repo rules never declare the environment they read and so Bazel
# does not refetch when it changes -- a base already fetched with the nixpkgs
# SDK baked in still needs `clean --expunge`. And it is a scrub, not hermeticity:
# the durable fix stays a sysroot Bazel can fetch and hash. See MODULE.bazel,
# TIN-4050.
unset DEVELOPER_DIR SDKROOT BAZELRC

# Lane flags stay with Bazel; run's program arguments stay after its separator.
options=()
program_args=()
after_separator=false
for argument in "$@"; do
  if [[ "$command" == run && "$argument" == -- ]]; then
    after_separator=true
  fi
  if [[ "$after_separator" == true ]]; then
    program_args+=("$argument")
  else
    options+=("$argument")
  fi
done

# The selected external rc may hold authentication configuration. Do not print it.
exec "$resolved" "${startup[@]}" "$command" \
  ${options[@]+"${options[@]}"} ${execution[@]+"${execution[@]}"} \
  --announce_rc=false ${program_args[@]+"${program_args[@]}"}
