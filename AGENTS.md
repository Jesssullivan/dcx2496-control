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
  empty-timeout replays (rollback's single pre-inverse identity shares that
  replay allowance), typed remote mode, exact Dump0/Dump1
  snapshot/readback, reviewed direct-parameter plans, and the read-only
  `discovery live-meter`: one Search identity (at most three attempts, only
  empty timeouts replay) then exactly ten paced fixed `Query::Ping` frames,
  each reply an exact 25-byte function-`0x04` frame, inside a 30 s budget,
  with no remote-mode or parameter write; its meter-byte layout is a lane
  hypothesis unverified on the named device.
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
  117) as off (0) or A+B (4) only; plus the typed-write lane
  (`dcx.write-plan/v1`, `docs/typed-writes.md`): the O1-O3 mutes (`0x03`) and,
  on O1-O4 only, output gain (`0x02`), delay switch, long and short delay
  (`0x04`, `0x05`, `0x4b`), crossover high/low-pass type and frequency
  (`0x42`-`0x45`), limiter switch, threshold and release (`0x46`-`0x48`) and
  polarity (`0x49`), and the input A-C gains (channels 1-3, `0x02`); the
  typed-write lane is offline planning only, pending an operator ruling
  (Legalab D5 front-panel entry stands for device writes; no `dec-*` ruling
  admits software writes to these addresses yet).
  Locations are transcribed from the pinned MIT DuinoDCX `outputLocations`,
  `inputLocations` and `setupLocations` tables; O1/PEQ9 and the O4 PEQ on/off,
  band count and band 1 fields have named-device readback, and every other
  address, including all routing and typed-write addresses, is pending its
  first exact readback (`dcxctl write verify`). Values must lie in their
  reviewed domain, every apply raises a level gain by at most +3 dB and never
  above unity, raises a limiter threshold by at most +3 dB and never switches
  an enabled limiter off, the apply path
  admits only PEQ cuts and never makes a band active (PEQ on and inside the
  band count) that holds a stored boost and was inactive in the baseline, nor
  moves or reshapes an active boost, at any ordered apply or rollback step,
  under any policy flag; it writes routing fields mutes first, then the input
  sum, then the O4 and O3 sources, and rollback restores the exact baseline in
  reverse order. No apply or rollback carrier on any lane writes an O5/O6
  mute value other than 1, so nothing ever unmutes O5 or O6 and a plan that
  mutes a live O5/O6 cannot be staged. Dump0 and Dump1 keep their baseline modulo-128 trailer
  balance. Every other output field (dynamic EQ, phase, name, PEQ editor
  index, every O5/O6 field but mute and PEQ), every other input field, the
  input SUM channel, every other setup field (Input C line/mic at Dump0 121,
  Mute Outs, links), the O1/O2/O5/O6 sources, and every opaque dump mapping
  fail closed. Typed writes refuse every plan while O5 or O6 reads unmuted,
  never address O5/O6, need explicit plan and command-line consent to unmute
  O1-O4 or change an O3/O4 source, and never combine the two; their rollback
  is the exact touched inverse, never a stored-preset recall. `control
  apply|rollback` refuse typed-write-lane carriers (the typed fields and the
  O1-O3 mutes; PEQ-only and O4-mute carriers stay under the core rules only);
  only `dcxctl write apply|rollback`, which rebuild the carrier from its write
  plan and enforce the safe in-frame order and exact-reverse rollback, send
  them, and those exist only behind the non-default `typed-write-live`
  feature, which `//:dcxctl_live_control` and `.#dcxctl-live` never enable
  until that ruling. Auto Align and
  +15 V have no
  address and must never gain one. No server or automatic apply path belongs
  in the MVP.
- Desired profiles are `dcx.desired-profile/v1`, exactly O1/channel 5/PEQ9
  with four ordered actions (`0x3b` through `0x3e`);
  `dcx.desired-profile/v2`, exactly O4/channel 8 with ordered cut-only PEQ
  on/off, band count, and band-field actions, matching the planner; or `dcx.desired-routing/v1`
  (`dcxctl routing desired-profile`), one to six routing actions in the fixed
  apply order above, with `--mvp` the ruling's routing (O5/O6 muted, input sum
  A+B, O4 from C, O3 from SUM). Static feedback notches (`dcxctl feedback`)
  target O4 only and never write the operator's active bands.
  `dcxctl control verify-containment` is the offline gate that fails when a
  readback changed any bit outside the plan's projected addresses (only its
  own bits of a shared carrier byte) and touched dump trailer. Missing
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
