# dcx2496-control

Typed, fail-closed building blocks for restoring Behringer ULTRADRIVE PRO
DCX2496 control in Tinyland's Legacy Audio Lab.

`dcxctl` remains deliberately **offline by default**. It can decode fixture bytes,
construct known read-only query frames, validate/diff complete profiles, plan
and validate query-only discovery, and quantize Room EQ Wizard Generic EQ text.
The separate `dcx-transport` crate adds an injected Search-only boundary.
`dcx-darwin-tty` binds that boundary to one private, digest-checked macOS
callout node. An explicit macOS-only `live-discovery` build adds `discovery
prepare`, `live`, and `repeat`; the default binary contains none of them. There
is no port enumeration, network server, arbitrary-frame input, or generic
byte-write surface.

## Safety boundary

- A valid frame is not proof of a connected or compatible device.
- Discovery accepts exactly one complete 26-byte search response with the
  expected device address. Partial, ambiguous, wrong-manufacturer, wrong-model,
  wrong-function, and wrong-address evidence fails closed; its 18-byte payload
  remains opaque.
- The pure discovery plan is exactly 115200 8N1 followed by one 38400 8N1
  fallback after an explicit timeout. It does not open or name a transport.
- The injected executor supplies only the exact eight-byte Search request. Each
  attempt has a 500 ms total adapter deadline and a 26-byte input ceiling; the
  sole fallback is eligible only after an empty primary timeout. Partial input,
  malformed or wrong identity, and transport errors stop without fallback.
- The Darwin carrier reserves 25 ms of that monotonic budget for mandatory
  termios/control-line restoration and close. It opens nonblocking with
  `O_NOCTTY`, obtains `TIOCEXCL`, performs one write syscall, and uses
  `FIONREAD` to detect overflow without consuming byte 27. It never calls
  `tcflush` or toggles DTR/RTS.
- The carrier checks `FIONREAD` before configuration and again after raw 8N1
  configuration. Any queued input blocks the write; it is never flushed or
  consumed as if it were a Search response.
- Synthetic fixtures are named `SYNTHETIC-*` and are never hardware evidence.
- Profiles bind the exact Behringer DCX2496 identity, profile ID/revision, and
  all six physical outputs. Omitted or role-swapped outputs fail validation.
- Input C is line mode with Auto Align, Auto EQ, and +15 V disabled. O5/O6 are
  always unused, unrouted, at -15 dB, and muted.
- Exact subwoofer and Alto PA models remain unverified in the fixtures, so the
  explicit apply-readiness gate blocks them even though offline diffing works.
- An unmuted output requires an explicit source and enabled limiter.
- REW imports are strictly bounded and cut-only; there is no boost option.
- The pure state machine treats panic as unverified/faulted, not as confirmed
  mute. Exact device/plan/profile readback and distinct expiring apply and
  activation tokens are required; its privileged state cannot be cloned or
  deserialized.

The carrier is not permission to probe. The feature-gated CLI reads a strict,
16 KiB maximum envelope only from redirected stdin; interactive stdin is
rejected. The raw path exists only in a non-cloneable, redacted in-memory
binding and must match an independently observed `sha256/...` digest immediately
before open. Only `/dev/cu.usbserial-*` is in scope. The authorization packet
binds the current LocalHostName, hardware/OS, boot, executable digest, physical
declarations, evidence revisions, and fixed I/O limits. Sanitized receipts
contain digests, byte counts, deadlines, cleanup, and an independently
recomputable `receiptBodyDigest`; they never retain the path, WORD, or raw
response. A WORD is replayable until its maximum 15-minute expiry, so a fresh
packet remains required after reboot, binary/profile/source/physical drift, or
binding change. The 38400 fallback remains a bounded compatibility hypothesis,
not a vendor claim for direct RS-232.

`discovery repeat` accepts only the complete canonical packet and successful
receipt body from the first Search, verifies both digests and all bindings, and
then issues exactly nine Searches at the successful baud. Each trial is spaced
by at least 500 ms, the session is capped at ten seconds, and the first timeout,
identity, transport, pacing, or cleanup failure stops the run.

The exact private-envelope contract, prepare/live/repeat behavior, receipt
mapping, merged-main artifact layout, and Legalab-owned execution boundary are
documented in [`docs/live-discovery.md`](docs/live-discovery.md).

## Entrypoints

Use the pinned Nix environment and Just recipes:

```sh
nix develop
just check
just transport-check
just darwin-carrier-check
just live-discovery-check
just bazel-check
```

Offline examples:

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

The Bazel graph exposes feature-free `//:dcxctl`, macOS-only
`//:dcxctl_live_discovery`, `//:check`,
`//crates/dcx-transport:all_tests`, and
`//crates/dcx-darwin-tty:all_tests`. The carrier target runs only fake syscalls;
CI never opens a tty. Cargo remains the source of Rust dependency truth;
Bzlmod's crate-universe derives its external graph from the committed
`Cargo.lock`. CI also requires committed `MODULE.bazel.lock` and `flake.lock`.

## Protocol status

The envelope and parameter encodings are research hypotheses until checked
against a safely commissioned DCX. See `NOTICE` for sources and provenance.
Unsupported functions remain typed as unknown; the parser does not speculate
about payload meaning. Arbitrary parsed messages cannot be constructed or
encoded through the public API: only typed, read-only queries can emit bytes.

The repository was initialized as `dcx-server` with the goal of a complete
Linux/web/mobile stack. That historical README and its MIT license remain in
Git history. Legalab's current epoch narrows this repository to a portable,
testable control core without a network service.

## License

New work is licensed under either Apache-2.0 or MIT, at your option. The
original MIT text remains in `LICENSE`; Apache-2.0 is in `LICENSE-APACHE`.
