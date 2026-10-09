# DCX2496 Control Agent Contract

This repository owns the typed, deterministic DCX2496 protocol and profile
runtime. Legalab owns the cross-repository studio ontology and PZM activation.

- Use `just` as the operator entrypoint. Bazel labels own the Rust build graph
  and index the Apple/schema product sources; SwiftPM and the checked-in
  XcodeGen specification own the native Apple build.
- Source build/test through pinned Bazelisk is local by default, bounded to two
  jobs; explicit REAPI requires an operator-supplied executor and external rc
  for authentication/properties and refuses local fallback
  (`dec-local-first-reapi-20261004`). GF enrollment is deferred. Host placement
  still needs separate admission. Hosted CI provides static advisory feedback;
  name the actual host, source revision and executed/skipped targets in receipts.
  Cache hits do not prove new remote execution. Native Apple and named-device
  acceptance remain separate PZM/Legalab lanes (`dec-native-studio-20261004`).
- `dcxctl` is offline-only by default. The explicit macOS-only `live-control`
  feature (`live-discovery` remains a compatibility alias) accepts one exact
  `/dev/cu.usbserial-*` callout and holds one fixed 38400 8N1 session. Its
  closed operations collect ten Search identities with at most ten paced
  empty-timeout replays, typed remote mode, exact Dump0/Dump1
  snapshot/readback, and reviewed direct-parameter plans.
  It never enumerates ports or accepts caller-supplied frames, baud, retry,
  timeout, remote-mode bytes, or generic writes.
- Mutation is snapshot -> strict reviewed-address semantic diff -> immutable
  apply and inverse plans -> stale-baseline check -> typed apply -> complete
  readback -> rollback. The reviewed addresses are the closed allowlist in
  `crates/dcx-core/src/layout.rs`: PEQ on/off (`0x06`), PEQ band count
  (`0x07`), and the nine PEQ bands (`0x13` through `0x3f`) on output channels
  5 through 10, plus the O4 output mute (channel 8, `0x03`, Dump1 byte 223).
  Locations are transcribed from the pinned MIT DuinoDCX `outputLocations`
  table; only O1/PEQ9 has named-device readback, the O4 mute is pending
  Legalab's WORD-FS-A rehearsal, and every other PEQ address is pending its
  first exact readback. Values must lie in their device domain, the apply path
  admits only PEQ cuts and never makes a band active (PEQ on and inside the
  band count) that holds a stored boost and was inactive in the baseline, under
  any policy flag, and rollback restores the exact baseline in reverse
  order. Dump1 keeps its baseline modulo-128 trailer balance as a named
  hypothesis. Every other output, input, setup, crossover, dynamic-EQ, mute,
  or opaque dump mapping fails closed. No server or automatic apply path
  belongs in the MVP.
- Desired profiles are `dcx.desired-profile/v1`, exactly O1/channel 5/PEQ9
  with four ordered actions (`0x3b` through `0x3e`), or
  `dcx.desired-profile/v2`, exactly O4/channel 8 with ordered cut-only PEQ
  on/off, band count, and band-field actions, matching the planner. Static
  feedback notches (`dcxctl feedback`) target O4 only and never write the
  operator's active bands. `dcxctl control verify-containment` is the offline
  gate that fails when a readback changed any byte outside the plan's
  projected addresses and touched dump trailer. Missing
  post-write capture remains explicit uncertainty: apply `readback` and
  rollback `restored` may be omitted or null, never synthesized.
- The AUv3 MIDI FX owns staged desired state and MIDI pass-through only. It has
  no serial, filesystem, process, or socket work in its render path. The
  foreground helper is the only Apple process allowed to invoke its bundled
  exact `dcxctl`; incoming CoreMIDI never causes device activity.
- Legalab owns physical/operator readiness before invoking any live command. Do not
  duplicate Legalab decisions, evidence schemas, or authorization ceremonies in
  this device repository.
- Unknown identity, state, route, or value is an error. Never infer that an
  output is safe or muted.
- Tests use synthetic or explicitly sanitized fixtures. Never commit captures,
  device serials, credentials, proprietary software, or calibration files.
- Preserve source provenance and per-file licenses. Do not claim an observed
  protocol fact from a synthetic fixture.
- Use pull requests and run `just check` before claiming completion.
- Never add AI attribution or `Co-Authored-By` trailers.
