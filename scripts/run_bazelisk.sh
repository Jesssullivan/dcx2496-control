#!/usr/bin/env bash
set -euo pipefail

candidate="${DCX_BAZELISK:-}"
if [[ -z "$candidate" ]]; then
  printf 'DCX_BAZELISK is unavailable; enter the pinned Nix development shell\n' >&2
  exit 78
fi

# Resolving the binding needs an interpreter, and a missing one says nothing
# about whether the binding is the pinned one. Report it as its own failure so
# the two conditions can never be read as each other.
if ! command -v python3 >/dev/null 2>&1; then
  printf 'python3 is unavailable to resolve DCX_BAZELISK; enter the pinned Nix development shell\n' >&2
  exit 78
fi

resolve_candidate() {
  python3 - "$1" 2>/dev/null <<'PY'
from pathlib import Path
import sys

print(Path(sys.argv[1]).resolve(strict=True))
PY
}

# A path that will not resolve is not the pinned binding, so it joins the case
# below rather than earning a third diagnostic.
if ! resolved="$(resolve_candidate "$candidate")"; then
  resolved=""
fi

case "$resolved" in
  /nix/store/*-bazelisk-*/bin/bazelisk | /nix/store/*-bazelisk/bin/bazelisk)
    ;;
  *)
    printf 'refusing bazelisk outside the exact Nix development-shell binding\n' >&2
    exit 78
    ;;
esac

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
unset DEVELOPER_DIR SDKROOT

exec "$resolved" "$@"
