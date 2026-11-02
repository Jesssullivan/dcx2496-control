# Approved room refresh

Linear: TIN-6964

The foreground helper may use this explicit read path after independently approved room configuration is bound to its exact target and callout:

```text
dcxctl profile room-refresh --expectation FILE --expectation-digest sha256/HEX --tty EXACT_CALLOUT --expected-device ADDRESS
```

`live-control` on macOS is required. Strict expectation validation and exact byte-digest equality precede tty open. The expectation authenticates no tty or caller. The helper must retain its saved binding checks.

One exact Search identity is required in at most three attempts. Only empty timeouts replay at the unchanged five-second gap. The existing Transmit, full Dump0/Dump1, ReceiveDirect and verified-close path follows. No parameter write occurs. Generic snapshot and mutation qualification remain ten identities.

Successful JSON has schema `dcx.approved-room-refresh/v1`:

| Field | Type and meaning |
|---|---|
| `status` | `match` or `drift` |
| `snapshot` | Complete existing `SnapshotV1`, including exact identity and both dump frames |
| `comparison` | Existing `dcx.room-comparison/v1` result against the pinned expectation |
| `captureCompletedAtUnixMilliseconds` | Wall time after verified close, before offline comparison |
| `captureElapsedMilliseconds` | Monotonic executor duration from before identity through close |
| `comparisonElapsedMilliseconds` | Actual offline comparison duration |
| `searchAttempts` | Actual Search exchanges submitted |
| `searchEmptyTimeouts` | Actual empty Search deadline results |
| `validSearchCount` | One validated same-address identity |
| `ignoredDump0SearchReplays` | Zero or one strict accepted Search replay consumed while awaiting Dump0 |
| `closeVerified` | `true` only after successful complete capture and verified close |
| `identityPolicy` | `approved-exact-target-one-identity-three-attempts` |

Exit zero means match. Exit one means drift and preserves the complete image. Failures returned by the bulk executor emit `status: failed` with null snapshot and comparison. Measured attempts and elapsed duration remain available. The `failure` object uses `dcx.approved-room-capture-failure/v1` with stable category, operation, response count and finish outcome. Its `sessionClosedVerified` and the outer `closeVerified` agree. Close failure uses a fixed carrier category. Human-readable error text grants no authority. Binding, open and admission failures return no ready envelope.

The fixed executor budget is twenty seconds. Source tests do not qualify this timing on the named device. This path does not alter proof expiry, approve a captured image or promise continuous readiness.

Lexical: XL-ACC-004

## Source validation

On Sting, `nix develop --command just check` passed all 300 Rust tests, formatting, Clippy, fixtures and front-door checks. No Rust tests were ignored. The six new tests cover strict approval admission and the shorter transport policy. Independent source review found no blocking issue.

The macOS-only CLI branch has not been compiled on Darwin or run against hardware. Those are separate qualification steps. No named-device latency result is claimed.


## Failed captures

The helper accepts typed failure metadata only from a completed child with exit one. It requires the exact schema, bounded metrics and consistent close fields. Failure data cannot pass successful snapshot validation or publish readiness. A legacy, malformed or conflicting failure retains unknown close. Status exposes an optional `captureFailure` within `roomFailureDiagnostic`, without private stderr.

An empty timeout count measures the response accumulator. The carrier may have consumed an exact request echo before that empty response. The count cannot establish that the device sent no bytes. Verified finish requires ReceiveDirect quiescence and checked serial cleanup. An empty response still invalidates readiness. The typed-envelope change alone adds no retry or device command.

Authority: user implementation direction and Root's separate typed-failure source scope, TIN-6964. Lexical: XL-ACC-004


## Periodic recovery

The opt-in foreground scheduler may retry completed empty Dump0 or Dump1 timeouts with verified session close. The typed schema, approved binding and current lifecycle must still match. Success publishes only the new complete matching capture. A failed attempt cannot extend or restore an expired proof.

Consecutive qualified failures wait 15, 30, 60, 120, 240 and then 300 seconds. Later failures retain the 300-second limit. Busy admission has a separate counter and cannot reset transport backoff. A real successful capture resets the read-failure counter and resumes the existing 30-second success delay. One active attempt prevents queued reads.

Malformed, unknown, nonempty, mismatched or unverified-close failures stay latched. Configuration, target or lifecycle changes invalidate the old attempt. Every new attempt repeats the existing approval, recovery, binding and exclusive-serial admission. This policy never retries a parameter write or publishes a synthetic match. Native and named-device recovery qualification remain separate.


## Late Search during Dump0

While awaiting Dump0, the Darwin carrier may consume one complete strict Search response matching this session's accepted identity byte for byte. The address must also match the pending Dump0. It then continues under the original absolute operation deadline, without another query, write or flush. A second replay, changed frame or partial response remains terminal. Dump1 and meter reads gain no replay allowance.

If no Dump0 follows that consumed replay, the result is a terminal carrier failure. It cannot become an empty Dump timeout eligible for periodic recovery. Complete dump validation, image comparison and consuming verified close remain required. Successful captures report `ignoredDump0SearchReplays` as zero or one. The helper accepts an absent legacy metric and rejects values above one.

This source rule addresses a possible late-response boundary. A retained Dump0 length failure alone does not identify the rejected frame or establish its cause. Synthetic tests do not qualify the rule on hardware.

Authority: Root's bounded implementation direction, TIN-6964. Lexical: XL-ACC-004

## Private Search diagnostics

A failed approved refresh may include top-level `privateReadContext`, separate from the public `failure` object. It records a post-write Search overflow only. Success envelopes, helper status and MIDI readiness never include the raw prefix. Ordinary carrier error Display and Debug redact it. The helper stores accepted context only in its bounded private failure record.

| Fields | Meaning |
| --- | --- |
| `schemaVersion` | Exact `dcx.search-read-context/v1` |
| `subphase`, `frameIndex` | `response` or `queued_replay`, with a zero-based frame index across the post-write read, including an echo |
| `prefix`, `received`, `queued` | At most 26 already-consumed bytes, consumed count and actual pending count. No pending byte is read for diagnostics |
| `delimiter`, `header` | Terminator presence and header shape. A header classification does not validate an identity |
| `observedMonotonicMicroseconds`, `deadlineMonotonicMicroseconds` | Actual observation time and the unchanged operation deadline. Neither grants freshness or retry authority |

Pre-write reconciliation context is not exported. Missing or malformed context does not change the original failure or its latch. Capture proof invalidates before private storage. Diagnostic storage failure cannot restore readiness.

Search limits apply to each frame, not the total queued input. A complete frame ends immediately at its terminator. A retained overflow count of 26 consumed bytes and six queued bytes establishes no packet identity. The historical bytes were not retained, so their type and the failure's cause remain unverified. This diagnostic change adds no read, request, replay, deadline extension or acceptance rule. Partial, changed and malformed input remains terminal under the existing verified-close policy.

Authority: Root's diagnostics-only implementation direction, TIN-6964. Lexical: XL-ACC-004
