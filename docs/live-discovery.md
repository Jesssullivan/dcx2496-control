# Darwin live discovery

The `live-discovery` feature adds a single macOS command:

```sh
dcxctl discovery live-search --tty /dev/cu.usbserial-EXACT_DEVICE --expected-device 0
```

It is intentionally a product operation, not an authorization or evidence
protocol. Legalab establishes the external hardware preconditions before
invocation. This repository owns only the typed Search request, bounded serial
execution, protocol validation, and compact runtime result.

## Behavior

The command performs one uninterrupted Search-and-repeat transaction:

1. Validate the explicit `/dev/cu.usbserial-*` callout path and expected DCX
   address.
2. Search at 115200 baud, 8N1, with a 500 ms whole-attempt deadline.
3. Search once at 38400 only if the primary attempt times out with zero bytes.
4. Require one exact 26-byte Behringer/DCX Search response at the expected
   address.
5. Derive the successful baud from the validated attempt.
6. Wait at least 500 ms before each of exactly nine same-baud repeats, stopping
   on the first timeout, identity failure, carrier failure, or 10-second repeat
   budget overrun.

Success is JSON with `status: identified`, device address, selected baud,
`validResponses: 10`, and sanitized carrier attempts. Failure also emits JSON
before returning nonzero. Neither form includes the callout path or raw response
payload.

## Carrier guarantees

Each attempt opens the named callout nonblocking and exclusively, snapshots the
terminal and modem-line state, rejects pre-existing queued input, writes only
the fixed eight-byte Search frame, reads at most 26 bytes, restores and verifies
the original state, and closes the descriptor. A failed restoration is a failed
operation even if the response itself was valid.

The command has no options for request bytes, retry count, fallback rate,
timeout, serial format, or arbitrary output. The feature-free `dcxctl` binary
contains no serial-capable command.

## Build

On an aarch64 Darwin builder:

```sh
just live-package
```

The resulting `dcxctl` is built with the `live-discovery` feature. The ordinary
test suites use injected transports and never open a serial device.
