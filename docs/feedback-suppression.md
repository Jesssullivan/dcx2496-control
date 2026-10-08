# Static feedback suppression on O4

Status: offline pipeline, synthetic fixtures, and injected-transport tests.
No named-device readback of an O4 PEQ address has been recorded yet, and no
acoustic claim is made. Real feedback frequencies need an attended ring-out.

## Scope

The live path's feedback suppression is static. The vocal channel's DSP1124P
runs single-shot and then locks; this product adds measured notches to the DCX
O4 output PEQ, which feeds the PA. Adaptive feedback cancellation is roadmap,
not this path.

## Pipeline

```text
ring-out list | REW Generic EQ
  -> dcxctl feedback import       (dcx.feedback-measurement/v1, digest-bound)
  -> dcxctl control snapshot      (SnapshotV1 baseline, live)
  -> dcxctl feedback plan         (dcx.notch-plan/v1, pure function)
  -> dcxctl feedback desired-profile   (dcx.desired-profile/v2)
  -> dcxctl control diff          (apply_plan + rollback_plan)
  -> dcxctl control apply         (stale-baseline check, one typed frame,
                                   complete readback)
  -> dcxctl control rollback      (only if readback is not exact)
```

`dcxctl feedback inspect --snapshot S --target-output 4` decodes O4's PEQ
enable, band count, and all nine bands from a saved snapshot, read-only.

## Measurement formats

Frequency list, one peak per line, `#` comments and blank lines skipped, an
optional `frequency_hz` or `frequency_hz,level_db` header:

```text
frequency_hz,level_db
630,4.0
2500,9.0
2500,7.5
```

`level_db` is a relative severity, higher is worse. A repeated frequency is a
recurrence. REW Generic EQ exports go through the strict REW parser; every
enabled filter must be a peaking cut, and its cut becomes the requested cut.
REW's Q is not used. Values are canonicalized to thousandths so the JSON
carrier round-trips exactly.

## Planner policy (defaults)

| Rule | Default |
| --- | --- |
| Target | O4 only; O1-O3, O5, O6 are refused |
| Merge | peaks closer than 200 cents (1/6 octave) merge; the most severe member is the frequency |
| Notch count | at most 4, and never more than the free bands |
| Density | at most 2 notches in any one-octave span |
| Shape | bell (kind 1), Q code 40 (Q 10, the device's narrowest), slope code 0 |
| Depth | -6 dB, or the REW requested cut; -3 dB more per recurrence; capped at -12 dB (`--max-cut-db`, never past -15) |
| Gain | always a cut, gain code at most 149 |

Selection ranks merged peaks by severity, then recurrences, then frequency,
and drops the rest with a reason (`notch_cap`, `per_octave_limit`,
`duplicate_code`). Selected notches are placed in ascending frequency order.

## Band ownership

Bands 1 to n, where n is the current band count, belong to the operator and
are never written. Notches go into bands n+1 to n+k. Each band writes all five
fields, then the band count, then PEQ on. If PEQ is off while n > 0, planning
refuses unless `--allow-enable-operator-bands` is given, because turning PEQ
on would also enable the operator's bands.

`--prior-plan` names the receipt of the plan last applied to O4. The baseline
must still hold that plan's notches exactly directly above its operator count;
those bands are then reused, so replanning the same measurement is
idempotent and produces an empty diff. If the baseline drifted, planning
refuses rather than guessing ownership. Without a prior receipt, earlier
notches count as operator bands and new notches stack above them. To deepen a
recurring peak across ring-out rounds, append the new ring-out to the same list
and plan with `--prior-plan`.

## Device-state guarantees

The writer is the generalized reviewed-address projection in
`crates/dcx-core/src/layout.rs`: PEQ on/off (`0x06`), band count (`0x07`), and
the nine bands (`0x13`-`0x3f`) of every output, plus the O4 mute, transcribed
from the pinned MIT DuinoDCX `outputLocations` table. Every value is checked
against its device domain. The apply path admits only cuts. Crossover, dynamic
EQ, delay, limiter, gain, source, Input C mode, and every other mute fail
closed, so this feature cannot enable Auto Align, phantom power, or unmute
O5/O6.

Rollback actions are the exact baseline values in reverse order: PEQ enable
and band count are restored before the bands they had activated. Exact
apply-then-rollback byte equality, both trailers included, is property-tested
over seeded random baselines.

## Open hypotheses for the first hardware probe

1. **Dump1 trailer.** The modulo-128 trailer balance is observed for Dump0. The
   projection applies the same rule to Dump1. If the device does not maintain
   Dump1 that way, the first O4 write reads back inexactly and the bound
   rollback runs.
2. **Band count side effects.** The plan writes band values before raising the
   band count. If the device resets newly activated bands when the count rises,
   readback is inexact and rollback runs.
3. **O4 locations.** Only O1/PEQ9 has named-device readback. The first probe
   should change only an inactive O4 band frequency, read back, and roll back,
   before applying a synthetic two-notch plan with speakers muted.
