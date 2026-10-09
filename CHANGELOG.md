# Changelog

## Unreleased

- Give the rollback identity step the same paced Search qualification as
  snapshot and apply: a fresh rollback session now replays empty Search
  timeouts (attempt one immediate, five-second cadence, at most ten replays)
  before the inverse write, instead of failing on one unanswered Search. Partial
  or invalid responses stay terminal, and the rollback envelope is now 90 s.
- Admit the MVP routing set under `dec-autonomous-muted-bench-20261007`: the
  O4/O5/O6 mutes, the O4 and O3 sources, and the setup input sum (off or A+B
  only), with a closed `dcx.desired-routing/v1` profile, `dcxctl routing
  desired-profile|inspect`, fixed apply order (mutes, input sum, sources) and
  reverse rollback. Input C, Mute Outs, crossover, the O1/O2/O5/O6 sources,
  the O1-O3 mutes, and every other setup address still fail closed.
- Surface O4 feedback notch plans in Logic (bridge `dcx.logic-bridge/v2`):
  Swift `dcx.desired-profile/v2` and `dcx.semantic-diff/v2` validation that
  mirrors the Rust core, an offline helper `feedback.notch.plan` request that
  runs `feedback import|plan|desired-profile` against the helper's raw
  snapshot, and an AU notch list with explicit staging. Fixture parity with
  dcxctl is checked in `//:check`.
- Carry a selected notch measurement file byte for byte from the AU (a
  leading UTF-8 BOM was dropped, so `source_digest` no longer equalled the
  file's digest), and widen the read-only Pending Changes AU parameter from
  0-1 to 0-47 so a v2 bank diff stays inside its declared range.
- Refuse every apply plan whose desired state newly activates a PEQ band that
  holds a stored boost (gain code above 150), by turning PEQ on or raising the
  band count, under every policy including `--allow-enable-operator-bands`.
  The check runs after every ordered action and every rollback step, not only
  on the final state, and an active operator boost may not be moved or
  reshaped (`ActiveBoostReshaped`).
- Restrict `dcx.desired-profile/v2` to O4, the planner's only target.
- Add offline `dcxctl control verify-containment` and the
  `scripts/serial-probe.sh` bench harness, which now exits non-zero when any
  bit changes outside the projected addresses and touched dump trailer (a
  shared 7-of-8 carrier byte admits only the plan's own bits).
- Bump the Apple helper and AUv3 bundle version to 0.1.2 (3).
- Record the first named-device O4 PEQ round trips: the Dump1 trailer balance,
  band-count ordering, and O4 PEQ on/off, count and band 1 locations read back
  exactly and rolled back to the original snapshot.
- Add static feedback suppression for O4: `dcxctl feedback import|inspect|
  plan|desired-profile`, digest-bound ring-out/REW measurements, and a pure
  notch planner that writes only cut-only bells above the operator's bands.
- Generalize the reviewed writer to PEQ on/off, band count, and all nine PEQ
  bands of every output from the transcribed DuinoDCX layout, with domain
  checks, cut-only apply admission, reverse-order rollback, and the Dump1
  trailer balance as a named hypothesis. Add `dcx.desired-profile/v2`.
- Report Identity Search qualification counts from its exact Rust error format.
- Preserve snapshot failure stage and explicit cleanup failure through one
  transaction `Capture(...)` envelope, retaining bounded inspection and unknown
  classification for unsupported formats.
- Distinguish bundled child launch failures and output-limit failures from
  malformed responses without automatic retry or recovery-state completion.
