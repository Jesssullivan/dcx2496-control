# Feedback-plan hardening after the A4 O4 round trips

Authority: dec-autonomous-muted-bench-20261007 / R-HOOK-CONVERGENCE-20261004
(R-N11, R-N13). Branch `fix/feedback-plan-hardening-20261007` off `47471c0`.
No serial or device write was made in this lane.

## Bench binary provenance (A4)

The unsandboxed `live-control` `dcxctl` that drove the 2026-10-08 A4 Status,
Search, Snapshot and both O4 PEQ round trips was built on petting-zoo-mini from
source `44adf4eb04fe5fcb4828c10acf0c2cc8d8ec60d8` (tree `0fccbff6...`), output
`/Volumes/LegalabCache/legalab/dcx-a4-20261007/src/target/release/dcxctl`:

- SHA-256 `e38dcdfb97b0ed9bf53454f74dbfbdde0cc84583ceeb40a800651d357abbe250`,
  `dcxctl 0.1.0`, built 2026-10-08T01:17Z.
- Recorded as `unsandboxedLiveDcxctlSHA256` in the private team-signed receipt
  for 44adf4e and as `STATUS dcxctl_sha256` in the A4 chain log; re-hashed on
  the host for this note (2026-10-08). The bundled, signed helper `dcxctl`
  is a different binary (`811e2f2e...19725`); see
  `2026-10-08-a4-signed-install-o4-peq-probe.md`.

## What changed

1. **Stored boosts never become active.** `ApplyPlanV1::new`, the single choke
   point of every v1/v2 `control diff` and every reparsed apply carrier, refuses
   a desired snapshot that makes a band active (PEQ on and band <= count) when
   that band holds a gain code above 150 and was inactive in the baseline
   (`StoredBoostActivation`). Only outputs whose active prefix grows are read.
   The O4 planner refuses the same case early (`WouldEnableStoredBoost`) even
   with `--allow-enable-operator-bands`; the flag still admits enabling the
   operator's cuts. An already active stored boost is left alone.
2. **v2 is O4-only.** `validate_bank_document` admits only O4/channel 8, the
   planner's sole target (`DESIRED_PROFILE_V2_OUTPUT`). AGENTS, README and
   `docs/feedback-suppression.md` say so.
3. **Containment gate.** `ApplyPlanV1::projected_offsets` and
   `uncontained_changes`, surfaced as offline `dcxctl control
   verify-containment`, which exits 1 when a readback changed any byte outside
   the projected location bytes (low, 7-of-8 carrier, high) plus the trailer of
   each touched dump (Dump1 byte 909 for O4). The A4 bench harness is now
   versioned as `scripts/serial-probe.sh` (helpers `serial_probe_route.py`,
   `serial_probe_v2.py`); it runs the gate after apply, always rolls back, and
   exits non-zero on an uncontained byte, a missing or unverified apply
   readback, or a final readback that differs from the original snapshot.
4. **Bundle version** `0.1.2 (3)` in `apple/project.yml`, so the next install
   is distinguishable from the A4 `0.1.1 (2)` install by version, not only hash.

## Tests

`enable_peq_over_stored_boost_is_refused`,
`raise_band_count_over_stored_boost_is_refused`, a seeded property test
(400 random baselines and action sets against an independent decode oracle, and
200 permissive planner runs that must either stage or refuse with
`WouldEnableStoredBoost`), a containment unit test, v2 O4-only refusals, and
`tests/feedback_pipeline.sh` coverage of `verify-containment` exit codes.

Validation receipts are in the pull request.
