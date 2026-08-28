# dcx2496-control

Typed, fail-closed building blocks for restoring Behringer ULTRADRIVE PRO
DCX2496 control in Tinyland's Legacy Audio Lab.

`dcxctl` is offline by default. It decodes fixture bytes, constructs known
read-only query frames, validates and diffs complete profiles, plans discovery,
and quantizes Room EQ Wizard Generic EQ text. The macOS-only `live-discovery`
feature adds one hardware command: a typed Search followed by exactly nine
same-baud repeats.

There is no port enumeration, server, arbitrary-frame input, generic byte-write
surface, or configuration command.

## Live Search

The live command accepts one explicit FTDI callout node:

```sh
dcxctl discovery live-search \
  --tty /dev/cu.usbserial-EXACT_DEVICE \
  --expected-device 0
```

One invocation performs the complete discovery milestone:

1. Open the named callout exclusively and issue the fixed Search query at the
   MVP golden-path 38400 baud binding, 8N1.
2. Validate the exact 26-byte Behringer/DCX response and expected device
   address.
3. Issue exactly nine more Searches at the same baud, paced five seconds apart.
4. Print structured JSON with the parsed identity, selected baud, valid-response
   count, and carrier diagnostics.

The caller cannot choose a repeat baud, repeat count, request bytes, timeout, or
initial-baud policy. Legalab owns physical readiness and operator authorization
before this command is invoked; this repository does not mirror those records.

## Serial boundary

- The only outbound frame is the eight-byte Search request.
- Every attempt has a 500 ms total deadline and 26-byte input ceiling.
- Partial input, invalid identity, overflow, timeout, or transport failure stops
  immediately.
- The Darwin carrier opens nonblocking with `O_NOCTTY`, obtains `TIOCEXCL`, and
  performs one write syscall per attempt.
- Existing queued input blocks a write; it is never flushed or consumed as a
  response.
- Termios and modem control lines are snapshotted, restored, read back exactly,
  and the descriptor is closed on every opened path.
- Raw callout paths and response payloads are omitted from diagnostic output.

## Offline commands

```sh
just dcxctl query search
just dcxctl discovery plan --expected-device 0
just dcxctl discovery validate-response \
  fixtures/protocol/SYNTHETIC-search-response-26.hex --expected-device 0
just dcxctl decode --file fixtures/protocol/SYNTHETIC-direct-parameter.hex
just dcxctl profile validate fixtures/profiles/safe-muted-v1.json
just dcxctl profile diff \
  fixtures/profiles/safe-muted-v1.json \
  fixtures/profiles/SYNTHETIC-control-room-v1.json
just dcxctl rew import fixtures/rew/SYNTHETIC-cut-only.txt --target-output 3
```

## Build and validation

`just` is the operator front door and Bazel is the build graph:

```sh
nix develop
just check
just bazel-check
just live-package
```

The Bazel graph exposes the default `//:dcxctl`, the macOS-only
`//:dcxctl_live_discovery`, and the transport/carrier test suites. Tests use
injected transports and synthetic fixtures; they never open a device. The live
Nix package is available only for `aarch64-darwin`.

## Scope

A valid Search response establishes protocol identity and address only. The
remaining response payload stays opaque until observed behavior supports a
typed interpretation. Snapshot, semantic diff, apply/readback, and rollback are
the next product phases; no blind write is implemented here.

New work is licensed under either Apache-2.0 or MIT, at your option. See
`NOTICE`, `LICENSE`, and `LICENSE-APACHE`.
