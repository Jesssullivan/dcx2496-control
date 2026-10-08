# PR #45 native qualification receipt

Authority: dec-autonomous-muted-bench-20261007 / R-HOOK-CONVERGENCE-20261004.
Host petting-zoo-mini, source 9a35ff70c7ea7614e2c0c34dd35e21454a7adad1 (Swift
inputs identical to 88ba484, where the native recall/binding targets executed
fresh), source tree and `DCX_BAZEL_OUTPUT_USER_ROOT` on `/Volumes/LegalabCache`,
under the pzm-bench lockf, 2026-10-08T00:28Z-00:47Z. Boot Data volume stayed at
14 GiB free throughout; no boot-disk floor controller was used.

- `//apple:logic_recall`: 15 methods, 0 failures.
- `//apple:response_binding`, `//apple:mutation_recovery` (14),
  `//apple:process_runner` (22), `//apple:child_failure_diagnostics` (10): pass.
- `just apple-package-check`: 56 methods, 0 failures, 22 skipped (the
  TEST_TMPDIR-gated suites executed by the Bazel targets above).
- `just apple-bundle-check` (unsigned helper + AUv3 + embedded dcxctl): BUILD
  SUCCEEDED.
- `just check` (Cargo fmt/clippy/tests, fixtures, lockfiles, frontdoor) and
  `just bazel-check` (8 of 8 Bazel tests, live-control build): pass.

No signing, installation, serial, Logic or audio operation. Installed helper/AU
identity and device behavior remain separately qualified work.
