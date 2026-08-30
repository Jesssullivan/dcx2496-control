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
exact request-prefix echo is removed inside a 34-byte wire bound and recorded
only as a byte count. Qualification always executes ten Searches on the same
descriptor, each after the first preceded by at least five seconds. Empty
timeouts are counted and do not suppress later trials; malformed, partial, or
transport failures remain terminal. Success still requires ten valid matching
identities. Snapshot and readback use the same ten-search identity sequence
followed by typed transmit remote mode, Dump0, and Dump1. Apply and rollback
accept only immutable plans produced from the exact O1/channel 5/PEQ9 desired
profile, verify a fresh baseline, issue one reviewed direct-parameter command
when needed, and attempt complete readback.

Queued input blocks a write. Each operation has an exact request type, response
ceiling, and deadline. The caller cannot choose frame bytes, remote-mode bytes,
baud, serial format, retry count, or timeout, and no generic write surface or
port enumeration exists. Raw callout paths and payloads stay out of diagnostic
output. Sanitized Search receipts distinguish total consumed wire bytes from
accepted response bytes and retain only overflow byte counts, never content.

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
