#!/usr/bin/env bash
# TIN-6964: host Python tests only, no compiler, Mach-O tool or device access.
set -euo pipefail
exec python3 -B -m unittest discover -s tests -p 'test_apple_*.py'
