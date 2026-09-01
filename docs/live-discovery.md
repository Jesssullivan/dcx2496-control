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

Interrupted operation recovery is a separate explicit write surface:

```sh
dcxctl control recover-receive-direct \
  --tty /dev/cu.usbserial-EXACT_DEVICE --expected-device 0
```

It configures the same fixed 38400 binding, discards pending input without
parsing it, accepts only one typed ReceiveDirect command, and discards input
again. If input resumes, recovery discards it without parsing and restarts the
quiet observation until one continuous 25 ms quiet window is observed inside
the fixed operation deadline. The sanitized receipt proves one complete kernel
write, the bounded discard count, and observed quiet, not device acknowledgement
or durable mode state. A normal complete snapshot must reacquire identity and
exact baseline state after recovery.

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
identity. The Search reader returns immediately when no replay has begun. If a
replay prefix is already queued after the accepted response, it finishes only
that already-started frame within the existing receive deadline and accepts it
only as a complete byte-for-byte duplicate. After each five-second device-cadence interval,
the persistent carrier also reconciles all queued complete frames against the
previous accepted Search response before the next write; it never counts those
replays as new identities. After the final valid Search and its queued-only
reconciliation, remote mode and Dump0 follow immediately. The carrier still
reconciles any already-queued exact replay at each write boundary.
Each frame remains bounded to 26 bytes and the whole operation remains bounded
by its existing deadline; a begun replay that does not finish, or any different
input, is terminal. Qualification
collects ten valid matching identities on the same descriptor and permits at
most ten empty-timeout replays, for a hard ceiling of twenty Search attempts and 120 seconds. Every
attempt after the first is preceded by at least five seconds. Empty timeouts are
counted; malformed, partial, or transport failures remain terminal. Snapshot
and readback use the same ten-search identity sequence followed by the
named-device-qualified transmit-only remote mode, Dump0, and Dump1, then switch
once to the source-documented receive-only mode before close. Apply and rollback
switch to receive-and-transmit only immediately before the typed direct-parameter command.
They accept only immutable plans produced from the exact O1/channel 5/PEQ9
desired profile, verify a fresh baseline, issue one reviewed direct-parameter
command when needed, and attempt complete readback. The full apply envelope is 150
seconds so the maximum qualified baseline path plus mutation readback remains
bounded with room for complete cleanup.

Unrecognized queued input blocks every normal operation write; the separate
recovery command discards it without parsing before its sole closed write. Once
a transmit-capable mode is attempted, consuming session finish retains the
bound device and performs one recovery-capable ReceiveDirect quiescence before
tty restoration on every later exit. A successful capture performs that same
transition immediately after Dump1, so finish does not duplicate it. Each
operation has an exact request type, response ceiling, and deadline. The caller
cannot choose frame bytes, remote-mode bytes,
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
just apple-team-signed-bundle
```

The live package uses the canonical `live-control` feature. Rust tests use
injected transports, the Apple product check is unsigned, and neither command
opens a serial device or launches a GUI.

The team-signed Apple carrier is also non-installing and device-free. It builds
Release unsigned, then signs the child, AUv3, and containing app inside-out with
one exact team `QP994XQKNH` Apple Development or Developer ID Application
identity. It retains the production App Group and helper serial entitlements,
requires no embedded provisioning profile, and accepts an optional host-owned
temporary keychain through `DCX_CODESIGN_KEYCHAIN`. A noninteractive caller may
provide its caller-owned unlock file through
`DCX_CODESIGN_KEYCHAIN_PASSWORD_FILE`; the recipe re-unlocks only that selected
keychain after the unsigned archive and before signing. The AU remains a
control-only MIDI FX: its single stable stereo output bus satisfies the host
lifecycle but performs no audio DSP and accepts no audio input.
