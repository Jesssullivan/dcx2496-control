# MVP routing allowlist (O4/O5/O6 mute, O3/O4 source, input sum)

Authority: Jess's ruling `dec-autonomous-muted-bench-20261007`, recorded
verbatim in Linear TIN-5379 comment `d24dac45-715d-4f93-82be-aaad015e61bf`
with reply `40fefa34-edca-4435-acef-c3154be5795a` /
R-HOOK-CONVERGENCE-20261004 (R-N11, R-N13). This note covers code only. The
live probes and the final apply belong to the bench lane, which runs under the
pzm-bench lock.

## Scope admitted (and nothing else)

| Field | Direct address | Dump byte | Domain | Pinned source |
| --- | --- | --- | --- | --- |
| O4 mute | ch 8, `0x03` | Dump1 223 | 0/1 (1 = muted) | `outputLocations[3]` row `0x03` |
| O5 mute | ch 9, `0x03` | Dump1 392 | 0/1 | `outputLocations[4]` row `0x03` |
| O6 mute | ch 10, `0x03` | Dump1 561 | 0/1 | `outputLocations[5]` row `0x03` |
| Input sum type | ch 0, `0x02` | Dump0 117 | 0 off or 4 A+B only | `setupLocations[0]` |
| O4 source | ch 8, `0x41` | Dump1 365 | 0 A, 1 B, 2 C, 3 SUM | `outputLocations[3]` row `0x41` |
| O3 source | ch 7, `0x41` | Dump1 195 | 0..3 | `outputLocations[2]` row `0x41` |

The locations come from DuinoDCX `00b9d70` `Ultradrive.cpp` (MIT). The value
encodings follow UltradrivePi `protocol.md`: setup `02` sum type
off/A/B/C/A+B/A+C/B+C, output `41` source A/B/C/SUM, and mute 1 = muted. All
six locations are low-bits-only, so no 7-of-8 carrier is written. Dump0 121
(setup `0x04`, Input C gain) and its carrier (Dump0 124), plus Mute Outs (setup
`0x15`, Dump0 57), have unit tests showing that no reviewed address owns them.

On 2026-10-08 I re-read the stored private baseline
`sha256/531ca64a...2352` (`a4-20261007/baseline-0.json` and
`audit-20261008/snapshot.json` are identical). Values: Dump0[117] = 0;
Dump1[195] = 1 (O3 B); Dump1[365] = 1 (O4 B); Dump1[392] and [561] = 0; Dump0
trailer [1013] = 7; Dump1 trailer [909] = 78.

## Order and projection

- Apply order is fixed: the O4, O5 and O6 mutes, then the input sum, then the
  O4 source, then the O3 source. `routing_rank` enforces it in
  `ApplyPlanV1::new`, and `dcx.desired-routing/v1` enforces it as well.
  Rollback is the exact reverse: sources first, then the sum, then the mutes.
- The MVP projection on the baseline shape changes only these bytes: Dump0
  117 (0->4) and its trailer (7->3), and Dump1 195 (1->3), 365 (1->2), 392
  (0->1), 561 (0->1) and the trailer 909 (78->73). Each trailer keeps its
  modulo-128 balance.
- `dcxctl routing desired-profile` with single-field flags builds the
  one-at-a-time probe profiles. `--mvp` builds the ruling's final state, which
  leaves the O4 mute untouched.

## Validation

- macbook-neo at `45162bb`: `just check` passed (fmt, clippy pedantic, Cargo
  tests, fixtures including the new `tests/routing_pipeline.sh`, lockfiles,
  frontdoor).
- PZM Bazel and native suites: results are in the PR.

Everything here is a transcription. Nothing is a device observation until the
bench lane's exact readback confirms each byte.
