# dcx2496-control

Typed, fail-closed building blocks for restoring Behringer ULTRADRIVE PRO
DCX2496 control in Tinyland's Legacy Audio Lab.

This foundation is deliberately **offline only**. `dcxctl` can decode fixture
bytes, construct known read-only query frames, validate/diff complete profiles,
plan and validate query-only discovery, and quantize Room EQ Wizard Generic EQ
text. It has no serial dependency, no network server, and no command that can
write to a device.

## Safety boundary

- A valid frame is not proof of a connected or compatible device.
- Discovery accepts exactly one complete 26-byte search response with the
  expected device address. Partial, ambiguous, wrong-manufacturer, wrong-model,
  wrong-function, and wrong-address evidence fails closed; its 18-byte payload
  remains opaque.
- The pure discovery plan is exactly 115200 8N1 followed by one 38400 8N1
  fallback after an explicit timeout. It does not open or name a transport.
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

The later hardware milestone must add query-only serial probing behind a new,
reviewed I/O boundary. This slice does not enumerate ports, open devices, set
line state, send bytes, or retry. The 38400 fallback remains an explicitly
bounded compatibility hypothesis, not a claim that the vendor documents it for
RS-232. A future I/O boundary must not weaken these pure contracts.

## Entrypoints

Use the pinned Nix environment and Just recipes:

```sh
nix develop
just check
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

The Bazel graph exposes `//:dcxctl` and `//:check`. Cargo remains the source of
Rust dependency truth; Bzlmod's crate-universe derives its external graph from
the committed `Cargo.lock`. CI also requires committed `MODULE.bazel.lock` and
`flake.lock`; the supply-chain gate denies the malicious August 2026 crate
releases and typosquats by name.

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
