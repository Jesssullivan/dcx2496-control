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

Exit zero means match. Exit one means drift and preserves the complete image. Failures returned by the bulk executor emit `status: failed`, null snapshot/comparison and null close verification, with measured attempts, elapsed duration and the error. Binding, open and admission failures return no ready envelope.

The fixed executor budget is twenty seconds. Source tests do not qualify this timing on the named device. This path does not alter proof expiry, approve a captured image or promise continuous readiness.

Lexical: XL-ACC-004

## Source validation

On Sting, `nix develop --command just check` passed all 300 Rust tests, formatting, Clippy, fixtures and front-door checks. No Rust tests were ignored. The six new tests cover strict approval admission and the shorter transport policy. Independent source review found no blocking issue.

The macOS-only CLI branch has not been compiled on Darwin or run against hardware. Those are separate qualification steps. No named-device latency result is claimed.
