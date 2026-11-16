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
| `staleSearchRepliesDiscarded` | `dump0` and `dump1`: Search replies the carrier discarded ahead of each dump, each zero or one. Added for TIN-6964. Output from `dcxctl` before this change omits it, including the build 14 helper at `9ff6e73` |
| `validSearchCount` | One validated same-address identity |
| `closeVerified` | `true` only after successful complete capture and verified close |
| `identityPolicy` | `approved-exact-target-one-identity-three-attempts` |

A failed envelope carries `staleSearchRepliesDiscarded` too. The stderr text names the same counts as `stale_search_replies`.

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

## Failed window bytes

Linear: TIN-6964. Nothing here ran on a device.

When a Search, Dump0 or Dump1 exchange fails in transport, times out, hits its limit or fails validation, `dcxctl profile room-refresh` writes one JSON line to stderr after the failed envelope. Budget, pacing, Search qualification, remote-mode and close failures write none. A success writes none, and the executor does not copy bytes out of the carrier then. The carrier still holds the last window in memory until its next exchange or close. It cannot clear the window on a good read, because validation runs in the executor after the carrier returns, and a frame that fails validation needs its bytes.

| Field | Meaning |
|---|---|
| `schemaVersion` | `dcx.failure-window-bytes/v1` |
| `operation` | `search`, `dump0` or `dump1` |
| `sequence` | The Search attempt number, or null for a dump |
| `received` | Every byte read after that request was written, kept or not |
| `kept` | Bytes kept, at most 128, from the first byte read |
| `truncated` | Whether `received` exceeds `kept` |
| `hex` | Lowercase hex of the kept bytes |

The window holds every byte the carrier read after the write: a request echo, a discarded Search reply, the frame that failed and any trailing frame. The carrier records each byte as it reads it. It adds no read, no wait and no write, and it changes no deadline or retry.

The line goes to stderr only. The helper keeps child stderr in its private record under `RoomDiagnostics`, which has mode `0700` with records at `0600`. The stdout envelope, the room receipt, helper status and the unified log never carry the bytes. The error text on the last stderr line keeps its earlier form and names no byte.

Two caps bound how many records hold bytes. The store keeps at most eight records. Before it writes a record, it removes any record older than seven days. A record older than that stays only until the next failure is recorded.

Apply and rollback read their dumps through the same carrier. The carrier keeps the window in memory for those reads too, but neither path writes it anywhere. They are out of scope here.


## Periodic recovery

The opt-in foreground scheduler may retry completed empty Dump0 or Dump1 timeouts with verified session close. It may also retry a completed Dump0 or Dump1 timeout after some bytes with verified session close, at most twice in a row, as [Room verification](room-verification.md) describes. A completed Dump0 or Dump1 validation failure with verified session close and no byte count retries the same way, with its own count. A completed Search timeout after some bytes, or a Search transport failure, with verified session close also retries the same way, with its own count. A stop at any of these caps lifts after a pause of five minutes that doubles up to thirty. The typed schema, approved binding and current lifecycle must still match. Success publishes only the new complete matching capture. A failed attempt cannot extend or restore an expired proof.

Consecutive empty timeouts wait 15, 30, 60, 120, 240 and then 300 seconds. Later empty timeouts retain the 300-second limit. This backoff applies to empty timeouts only. A partial dump retries at the 30-second success delay and has its own counter. Busy admission has a separate counter and cannot reset transport backoff. A real successful capture resets the read-failure counter and resumes the existing 30-second success delay. One active attempt prevents queued reads.

Other malformed, unknown, nonempty, mismatched or unverified-close failures stay latched. Configuration, target or lifecycle changes invalidate the old attempt. Every new attempt repeats the existing approval, recovery, binding and exclusive-serial admission. This policy never retries a parameter write or publishes a synthetic match. Native and named-device recovery qualification remain separate.
