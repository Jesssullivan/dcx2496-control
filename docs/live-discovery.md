# Darwin live control

The macOS-only `live-control` feature adds explicit, bounded device operations.
`live-discovery` remains a compatibility feature name. Discovery is still the
smallest live command:

```sh
dcxctl discovery live-search --tty /dev/cu.usbserial-EXACT_DEVICE --expected-device 0
```

These are product operations, not authorization or evidence protocols. Legalab
establishes physical mute, routing, named-device identity, and attended-stop
preconditions before invocation. This repository owns typed requests, bounded
serial execution, protocol validation, and sanitized results.

## Persistent session

Every live invocation validates one explicit `/dev/cu.usbserial-*` callout,
opens it nonblocking and exclusively, snapshots terminal and modem-line state,
configures fixed 38400 8N1 once, and reuses that descriptor for the complete
operation. Consuming finish restores and verifies the original state before
close. Setup failure performs the same bounded cleanup; a failed restore is a
failed operation.

Search accepts either one exact 26-byte identity or one byte-for-byte copy of
the eight-byte Search immediately followed by that identity. The optional
exact request-prefix echo is removed only when it arrives as its own complete
frame and is recorded as a byte count. The reader stops at the first frame
terminator under the protocol hard bound, then still requires an exact 26-byte
identity. The Search reader settles through the existing bounded receive
deadline so at most one trailing frame—including one that becomes readable just
after the first terminator—is consumed, and only a byte-for-byte duplicate
response is tolerated. After each five-second device-cadence interval, the
persistent carrier also reconciles at most one still-later queued frame against
the previous accepted Search response before the next write; it never counts
that replay as a new identity. The final valid Search retains one cadence
interval before transmit mode so the same reconciliation covers the
Search-to-Dump boundary. Partial, different, already-replayed, or surplus input
remains terminal. Qualification collects ten valid
matching identities on the same descriptor and permits at most ten empty-timeout
replays, for a hard ceiling of twenty Search attempts and 120 seconds. Every
attempt after the first is preceded by at least five seconds. Empty timeouts are
counted; malformed, partial, or transport failures remain terminal. Snapshot
and readback use the same ten-search identity sequence
followed by typed transmit remote mode, Dump0, and Dump1. Apply and rollback
accept only immutable plans produced from the exact O1/channel 5/PEQ9 desired
profile, verify a fresh baseline, issue one reviewed direct-parameter command
when needed, and attempt complete readback. The full apply envelope is 150
seconds so the maximum qualified baseline path plus mutation readback cannot
exhaust the budget merely because both required replay-settle cadences ran.

Queued input blocks a write. Each operation has an exact request type, response
ceiling, and deadline. The caller cannot choose frame bytes, remote-mode bytes,
baud, serial format, retry count, or timeout, and no generic write surface or
port enumeration exists. Raw callout paths and payloads stay out of diagnostic
output. Sanitized Search receipts distinguish total consumed wire bytes from
accepted response bytes and retain only overflow byte counts, never content. An
open persistent carrier retains at most one fixed 26-byte accepted Search
response solely for byte-identical late-replay comparison; it is dropped at
session close and never logged, serialized, or exposed in a receipt.

Offline `control diff` is available without a live feature. It validates one
raw snapshot plus one strict desired profile and emits the immutable apply and
rollback carriers consumed by the live commands.

## Build

On the aarch64 Darwin builder:

```sh
just live-package
just product-check
```

The live package uses the canonical `live-control` feature. Rust tests use
injected transports, the Apple product check is unsigned, and neither command
opens a serial device or launches a GUI.
