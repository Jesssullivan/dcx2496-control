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

An empty timeout count measures the response accumulator. The carrier may have consumed an exact request echo before that empty response. The count cannot establish that the device sent no bytes. Verified finish requires ReceiveDirect quiescence and checked serial cleanup. An empty response still invalidates readiness and leaves periodic failure latched. This change adds no retry or device command.

Authority: user implementation direction and Root's separate typed-failure source scope, TIN-6964. Lexical: XL-ACC-004
