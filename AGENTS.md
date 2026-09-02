# DCX2496 Control Agent Contract

This repository owns the typed, deterministic DCX2496 protocol and profile
runtime. Legalab owns the cross-repository studio ontology and PZM activation.

- Use `just` as the operator entrypoint. Bazel labels own the Rust build graph
  and index the Apple/schema product sources; SwiftPM and the checked-in
  XcodeGen specification own the native Apple build.
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
  readback -> rollback. The reviewed addresses are exactly O1/PEQ9 (channel 5,
  `0x3b` through `0x3e`) plus the O4 output mute (channel 8, `0x03`, Dump1
  byte 223); the mute address is fixture-derived and pending hardware
  confirmation by Legalab's WORD-FS-A silent mute-frame rehearsal. Every other
  output, slot, address, or opaque dump mapping fails closed. No server or
  automatic apply path belongs in the MVP.
- The desired profile is exactly O1/channel 5/PEQ9 with four ordered actions
  (`0x3b` through `0x3e`), and its semantic diff contains zero or one complete
  PEQ-slot change. Missing post-write capture remains explicit uncertainty:
  apply `readback` and rollback `restored` may be omitted or null, never
  synthesized.
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
