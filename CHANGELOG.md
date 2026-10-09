# Changelog

## Unreleased

- Admit the MVP routing set under `dec-autonomous-muted-bench-20261007`: the
  O4/O5/O6 mutes, the O4 and O3 sources, and the setup input sum (off or A+B
  only), with a closed `dcx.desired-routing/v1` profile, `dcxctl routing
  desired-profile|inspect`, fixed apply order (mutes, input sum, sources) and
  reverse rollback. Input C, Mute Outs, crossover, the O1/O2/O5/O6 sources,
  the O1-O3 mutes, and every other setup address still fail closed.
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
