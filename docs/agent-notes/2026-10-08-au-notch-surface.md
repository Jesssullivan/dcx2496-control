# Issue #46 Logic notch surface (bridge v2)

Authority: dec-autonomous-muted-bench-20261007 (Linear TIN-5379 comment
d24dac45 and reply 40fefa34) / R-HOOK-CONVERGENCE-20261004 R-N13.
Branch `feat/au-notch-surface-20261007`, PR #51.

## Decisions

- The wire becomes `dcx.logic-bridge/v2`. The AU is embedded in the helper
  bundle, so both ends ship together. A v1-labelled frame is refused, and
  `schemas/v1` stays frozen as the prior wire.
- `DesiredProfile` and `SemanticDiff` are version-neutral. A v1 desired profile
  keeps its exact wire and `dcx.logic-project-state/v1` bytes, so existing
  Logic projects recall unchanged.
- The AU sends the measurement bytes and a snapshot reference by digest. The
  helper owns every child, raw snapshot and stored plan (`Plans/notch/`). A
  prior plan is named by digest only.
- No policy overrides cross the bridge. The planner runs at its defaults: 4
  notches, -6 dB initial cut, -3 dB per recurrence, -12 dB maximum, 200-cent
  merge. "Enable over operator bands" is never offered.
- A staged plan is bound to its planning snapshot. Previewing against a
  different capture is refused, so the operator must replan.

- v2 is O4-only in Swift ahead of #49, so Swift is never wider than Rust.
- Adversarial review (code-review, high) findings fixed in the second commit:
  - stale pending plan after recall or a newer capture;
  - a v2 diff that under-reports writes (now bound to the raw apply plan's
    actions);
  - planning-baseline preference;
  - request-side errors mapped to invalid_request;
  - equal-digest plan re-serialization;
  - the "1...0" display;
  - duplicated validators;
  - the diff decode round trip.
  The SemanticDiff enum refactor is deferred as cleanup only.
- Second review (medium) fixed in the third commit:
  - the planning baseline is now single-sourced from model state;
  - the pending plan is bound to the staged profile digest;
  - plan storage is first-write-wins;
  - v2 `before` values are bound to the rollback plan;
  - snapshot store errors are narrowed;
  - one shared JSON-integer rule;
  - the recurrence message is scoped to the request's document context.
- Third review (medium) fixed in the fourth commit:
  - the summary and the profile now come from the same plan bytes;
  - a damaged stored entry heals;
  - stored-baseline faults are reported as invalid_request;
  - the recurrence explanation comes only from a failed plan step;
  - state is re-checked when the file panel closes;
  - an explicit target-mismatch message;
  - the diff binding runs before any plan is persisted;
  - one state-view validity rule.
  The recurrence offer stays a digest heuristic, because a snapshot summary
  carries no band fields.

## Evidence

- Neo (iteration only, not evidence): `swift test` 79 methods, 0 failures.
  This included a real-dcxctl end-to-end run via `DCX_TEST_DCXCTL`.
  `just apple-bundle-check` BUILD SUCCEEDED. Swift-encoded v2 frames validated
  against the v2 schema with jsonschema 4.23.
- PZM native receipt: see the PR #51 comment.

## Open

- After a verified Apply, the helper lease stays pinned until the exact
  baseline is restored, so the AU cannot plan the next round while notches
  stay applied. The attended loop therefore uses dcxctl. An explicit "accept
  applied state" terminal is separate work.

- Rebase after the WF-3b (hardening) and WF-3c (MVP routing) PRs merge.
  Expected overlap: CHANGELOG/README lines only.
- The installed signed helper/AU and Logic UI behavior are not qualified here.
  That needs a signed bundle install on PZM, which is a separate lane.

## Post-merge adversarial review (2026-10-09)

Review of merged #51 on main after #49/#52/#53 (R-HOOK-CONVERGENCE-20261004
R-N13; dec-autonomous-muted-bench-20261007). Two defects, fixed in the
follow-up PR:

- The AU decoded the selected text file with `String(data:encoding: .utf8)`,
  which drops a leading UTF-8 BOM. dcxctl's frequency-list importer accepts a
  BOM and digests the bytes it is handed, so for a BOM file the reported
  `source_digest` was internally consistent but no longer the digest of the
  file legalab recorded. The AU now carries the exact bytes
  (`FeedbackMeasurementInputV1.file`), refusing invalid UTF-8; a real-dcxctl
  test pins source digest = file digest.
- The read-only Pending Changes AU parameter kept its v1 range 0-1 while a v2
  bank diff reports up to 47 changes. Now 0-47.

Checked and held: Swift v2 validation and digest still match Rust after #49
(O4-only, cut-only; the stored-boost refusal is planner/apply-side and stays
in dcxctl); plan action order (band fields, count, enable) matches
`plan_notches` and `verify`; peak bounds (20-20000 Hz, level within 200 dB)
match `validate_peak`; the render block is unchanged and does no locking or
I/O; offline children get no tty or device lock; the baseline load is
device-bound. Not addressed (design scope): the recovery-lease blocker above.
