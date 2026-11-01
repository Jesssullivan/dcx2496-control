#!/usr/bin/env bash
# TIN-6964: injected artifact signing fixtures, no native tools or device access.
set -euo pipefail
if [[ $# != 2 ]]; then
  printf 'usage: run_sign_existing_bundle_contract.sh SIGNER TESTS\n' >&2
  exit 64
fi
export PYTHONDONTWRITEBYTECODE=1
exec python3 "$2" "$1"
