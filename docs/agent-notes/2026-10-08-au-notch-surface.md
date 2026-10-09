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
