#!/usr/bin/env bash
set -euo pipefail

candidate="${DCX_BAZELISK:-}"
if [[ -z "$candidate" ]]; then
  printf 'DCX_BAZELISK is unavailable; enter the pinned Nix development shell\n' >&2
  exit 78
fi

resolved="$({ python3 - "$candidate" <<'PY'
from pathlib import Path
import sys

print(Path(sys.argv[1]).resolve(strict=True))
PY
} 2>/dev/null || true)"

case "$resolved" in
  /nix/store/*-bazelisk-*/bin/bazelisk | /nix/store/*-bazelisk/bin/bazelisk)
    ;;
  *)
    printf 'refusing bazelisk outside the exact Nix development-shell binding\n' >&2
    exit 78
    ;;
esac

exec "$resolved" "$@"
