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
  5 through 10; plus the MVP routing set admitted by Jess's ruling
  `dec-autonomous-muted-bench-20261007` (Linear TIN-5379 comment `d24dac45`):
  the O4/O5/O6 output mutes (channels 8 through 10, `0x03`, Dump1 bytes
  223/392/561), the O4 and O3 output sources (channels 8 and 7, `0x41`, Dump1
  bytes 365/195), and the setup input sum type (channel 0, `0x02`, Dump0 byte
  117) as off (0) or A+B (4) only. Locations are transcribed from the pinned
  MIT DuinoDCX `outputLocations` and `setupLocations` tables; O1/PEQ9 and the
  O4 PEQ on/off, band count and band 1 fields have named-device readback, and
  every other address, including all routing addresses, is pending its first
  exact readback. Values must lie in their reviewed domain, the apply path
  admits only PEQ cuts and writes routing fields mutes first, then the input
  sum, then the O4 and O3 sources, and rollback restores the exact baseline in
  reverse order. Dump0 and Dump1 keep their baseline modulo-128 trailer
  balance. Every other output, input, setup (Input C gain/mode at Dump0 121,
  Mute Outs, links), crossover, dynamic-EQ, mute (O1-O3), source (O1, O2, O5,
  O6), or opaque dump mapping fails closed. Auto Align and +15 V have no
  address and must never gain one. No server or automatic apply path belongs
  in the MVP.
- Desired profiles are `dcx.desired-profile/v1`, exactly O1/channel 5/PEQ9
  with four ordered actions (`0x3b` through `0x3e`);
  `dcx.desired-profile/v2`, one output's ordered cut-only PEQ on/off, band
  count, and band-field actions; or `dcx.desired-routing/v1`
  (`dcxctl routing desired-profile`), one to six routing actions in the fixed
  apply order above, with `--mvp` the ruling's routing (O5/O6 muted, input sum
  A+B, O4 from C, O3 from SUM). Static feedback notches (`dcxctl feedback`)
  target O4 only and never write the operator's active bands. Missing
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
