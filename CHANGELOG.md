# Changelog

## Unreleased

- Add output profile chains: `dcxctl profile plan|verify` (offline) and
  `profile apply` (`typed-write-live` only) move one output to a Legalab
  `DcxOutputProfileV1` (fixture `fixtures/profiles/LEGALAB-dcx-o4-vocal-1dc18140.json`)
  through an ordered chain of `write apply` transactions, PEQ, gain and
  crossover only (Legalab `dec-scene-dcx-apply-20261021`). Each step keeps
  the write policy (3 dB summed rise, 0 dB ceiling, cut-only, 5/3
  remote-mode writes), a cut band is retuned only from 0 dB, bands enter
  and leave the active set safely, and each step carries its predicted
  digest and exact rollback. Apply re-plans, requires the reviewed
  `chain_id`, and stops at the first step that does not verify.
- Refuse a PEQ band count above 8 on every apply carrier (core guard) and
  in `write plan`, and cap feedback notches at band 8: the device stored
  band 9 but refused the count twice on 2026-10-21 (`MAX_ACTIVE_PEQ_BANDS`).
- Document why there is no preset store, recall or directory command: none
  of the public sources read gives those frames (agent note 2026-10-21). The
  note records the sources and claims, the AUTO-STORE persistence path, and
  the design a future `dcxctl preset` would follow. It also gives an
  attended probe: a front-panel store checked by G1 snapshots, with no
  direct-parameter write, then a passively tapped DCX2496 -> PC sync by the
  vendor editor (its own decision).
- Repeat a failed post-apply readback only as a fresh read-only snapshot:
  `write apply` and `control apply` retry a readback that failed in
  transport or read (and closed verified) as one ordinary complete G1
  snapshot on a fresh session, 20 s later and only while it fits the
  existing 150 s apply envelope (usually one retry, two if the first fails early; ceiling three
  readbacks; fixed constants), never re-sending a write frame. Validation,
  mismatch, remote-mode, budget, close and open failures are not retried;
  if every readback fails the result is unchanged (`rollback_required`,
  `post_apply_state_uncertain`, `readback: null`). Apply output gains
  `readback_attempts`, sanitized `readback_failures` and
  `readback_retry_stop`.
- Model the observed linked O3/O4 crossover: an O4 high-pass frequency
  write, on a baseline holding O3 low-pass frequency at the same code, also
  moves O3 low-pass frequency to the written code (2026-10-20 PA leg). The
  desired snapshot and rollback expect it, `write plan` lists it under
  `linked_effects`, and `write verify` / `control verify-containment`
  report it under `linked_changes` instead of as uncontained; any other bit
  or code stays uncontained. The reverse direction is reported as
  `inferred_unverified`, never expected and never contained. A follower move
  on an active filter needs both protection consents, and a plan writing
  both fields of a linked pair is refused.
- Typed writes are no longer offline-only by ruling: Legalab
  `dec-dcx-attended-software-writes-20261019` admits attended software
  writes (at most 5 remote-mode writes per apply, 3 per rollback);
  `typed-write-live` stays off in unattended builds.
- Add typed parameter writes, offline planning only pending an operator
  ruling: `dcxctl write show|plan|verify` (offline), and `write
  apply|rollback` only behind the non-default `typed-write-live` feature,
  which no shipped binary enables (Legalab D5, front-panel entry, stands for
  device writes). `write apply` rebuilds its carrier from `--write-plan`
  under the same flags; both live commands exit non-zero unless the readback
  verifies. A strict `dcx.write-plan/v1` lists
  typed changes (PEQ band, PEQ on/off and band count, crossover, output gain,
  mute, polarity, delay, limiter, input A-C gain, and an O3/O4 source behind
  two consents). The dry-run `dcx.write-transaction/v1` prints the exact
  frame bytes, the dump offsets each value lands at, and the apply/rollback
  carriers. Policy refusals cover O5/O6 (never addressed; every plan refused
  while either reads unmuted), unmute, routing or loosening an active
  high-pass without consent, a summed level rise (gains, thresholds,
  effective PEQ band gains) past +3 dB, a level rise past 0 dB, limiter
  switch-off, EQ boosts, more than 16 actions, and carriers out of the safe
  in-frame order or (rollback) not its exact reverse. Moving or reshaping an
  active PEQ cut spends its released depth from the rise budget. `control
  apply|rollback` refuse typed-write-lane carriers (typed fields and O1-O3
  mutes); PEQ-only and O4-mute carriers stay under the core rules only.
  The reviewed allowlist gains the O1-O3 mutes, the O1-O4 gain, delay,
  crossover, limiter and polarity rows, and the input A-C gains, transcribed
  from the pinned DuinoDCX tables and pending named-device readback. Core apply
  carriers now refuse level rises past unity or +3 dB, limiter switch-off,
  and typed-write-lane actions while O5/O6 read unmuted, for every lane.
- Never unmute O5/O6 on any lane: every apply and rollback carrier refuses an
  O5/O6 mute value other than 1 (`ReservedUnmute`), so `control
  apply|rollback`, `write apply|rollback` and the Apple helper cannot send
  one, and a plan that mutes a live O5/O6 cannot be staged (its exact
  rollback would unmute). `routing desired-profile --o5-mute|--o6-mute`
  accept `on` only. The routing and write pipelines use the new
  `fixtures/snapshots/SYNTHETIC-reserve-muted-snapshot.json`. See
  `docs/typed-writes.md`.
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
