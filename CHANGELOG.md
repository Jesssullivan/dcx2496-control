# Changelog

## Unreleased

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
