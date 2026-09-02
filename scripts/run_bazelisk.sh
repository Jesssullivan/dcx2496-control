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

exec "$resolved" "$@"
