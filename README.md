# dcx2496-control

Typed, fail-closed control for the Behringer ULTRADRIVE PRO DCX2496 in
Tinyland's Legacy Audio Lab.

The Rust core decodes bounded protocol frames, validates complete snapshots,
imports Room EQ Wizard Generic EQ text, maps one reviewed O1/PEQ9 change, and
builds immutable apply and rollback plans. `dcxctl` is offline by default. Its
macOS `live-control` feature holds one exact 38400 8N1 callout session and
exposes only typed discovery, remote-mode, Dump0/Dump1, and direct-parameter
operations. `live-discovery` remains a compatibility feature name.

There is no port enumeration, server, arbitrary-frame input, generic byte-write
surface, MIDI-to-device mapping, or automatic project-recall apply.

## Control flow

The implemented product path is:

```text
REW export -> typed O1/PEQ9 desired document -> snapshot/diff plans
           -> explicit apply -> complete readback -> explicit rollback

Logic AUv3 UI -> App Group socket -> foreground DCXLogicHelper
              -> exact bundled dcxctl -> RS-232 DCX2496
```

The AUv3 is a control-only MIDI FX with no audio input or audio DSP. It exposes
one stable stereo output bus required by the host's MIDI-processor lifecycle;
its render block passes MIDI through and performs no IPC, process launch,
filesystem access, or serial work. Project recall stages desired state only.
The foreground helper owns one single-flight child transaction and is the only
Apple process that can execute the bundled `dcxctl`. The separate CoreMIDI
Commands and Status endpoints have no device-control packet mapping in bridge
v1.

## CLI

Discovery uses one open descriptor to collect ten validated Search identities,
with five-second pacing and at most ten empty-timeout replays:

```sh
dcxctl discovery live-search \
  --tty /dev/cu.usbserial-EXACT_DEVICE \
  --expected-device 0
```

A complete raw `SnapshotV1` contains one validated identity plus exact 1015-byte
Dump0 and 911-byte Dump1 frames and their canonical digests:

```sh
dcxctl control snapshot \
  --tty /dev/cu.usbserial-EXACT_DEVICE \
  --expected-device 0 > snapshot.json
```

If an earlier interrupted session left the device transmitting unsolicited
direct-parameter traffic, recovery is a separate explicit state write. It
discards input without parsing it, writes exactly one typed ReceiveDirect
command, observes a bounded quiet window, restores the tty, and closes. Its
receipt proves write acceptance and observed quiet, not a device acknowledgement;
a normal complete snapshot must establish identity and state afterward.

```sh
dcxctl control recover-receive-direct \
  --tty /dev/cu.usbserial-EXACT_DEVICE \
  --expected-device 0
```

REW import and slot mapping are offline. The MVP writable mapping is exactly
physical output O1, PEQ9: channel 5 parameters `0x3b` through `0x3e`
(frequency, Q, gain, and filter kind). It deliberately excludes EQ enable,
editor selection, and shelf slope.

```sh
dcxctl rew plan-slot fixtures/rew/SYNTHETIC-cut-only.txt \
  --target-output 1 --filter-index 1 --peq-slot 9

dcxctl control diff --snapshot snapshot.json --profile desired-profile.json \
  > transaction-plans.json
```

`desired-profile.json` is the strict `dcx.desired-profile/v1` bridge document;
its `document` value is the unchanged `rew plan-slot` result. `control diff`
derives the exact desired snapshot and inverse values from the immutable
baseline and emits zero or one exact O1/PEQ9 semantic change plus strict raw
`apply_plan` and `rollback_plan` carriers. Profile identity and revision are
1–128 ASCII bytes, and every digest is `sha256/` plus 64 lowercase hexadecimal
digits. The document is exactly O1, channel 5, PEQ9 with four ordered actions:
frequency (`0x3b`), Q (`0x3c`), cut-only gain (`0x3d`), and peak kind (`0x3e`).

The diff output is one envelope. Extract its two immutable carriers before a
live command:

```sh
jq -e '.apply_plan' transaction-plans.json > apply-plan.json
jq -e '.rollback_plan' transaction-plans.json > rollback-plan.json
```

Live mutation accepts only those carriers. Apply first captures and compares a
fresh complete baseline on the same descriptor; a stale plan is rejected before
the direct write. Apply and rollback then attempt complete readback. If that
capture is unavailable, the Apple bridge returns an omitted or null `readback`
or `restored` document with false equality and an explicit unresolved/rollback
state; it never fabricates a successful snapshot.

```sh
dcxctl control apply \
  --tty /dev/cu.usbserial-EXACT_DEVICE --expected-device 0 \
  --plan apply-plan.json

dcxctl control readback \
  --tty /dev/cu.usbserial-EXACT_DEVICE --expected-device 0

dcxctl control rollback \
  --tty /dev/cu.usbserial-EXACT_DEVICE --expected-device 0 \
  --plan rollback-plan.json
```

Legalab owns the physical mute, route, authorization, and attended-stop
preconditions for every live invocation. This repository does not duplicate
those records.

## Serial boundary

- The caller supplies one exact `/dev/cu.usbserial-*` callout; the program never
  enumerates ports.
- The tty opens nonblocking with `O_NOCTTY` and `TIOCEXCL`, snapshots termios and
  modem lines, configures 38400 8N1 once, and reuses the descriptor.
- Every request is a closed typed value. Dump capture and readback send the
  named-device-qualified transmit-only remote mode before Dump0/Dump1, then
  make one source-documented receive-only transition. Once a transmit-capable
  mode is attempted, consuming finish performs that same bounded receive-only
  quiescence on every later failure before restoring the tty. Apply and rollback
  use receive-and-transmit only immediately before the reviewed direct command.
  No unobserved disable frame is invented.
- Queued input blocks a write. Reads have exact response ceilings and stop on
  timeout, malformed framing, wrong address/part, overflow, or EOF.
- The separate `recover-receive-direct` operation is the sole exception to
  queued-input refusal: it discards rather than accepts pending input and can
  write only the closed, named-device ReceiveDirect command at fixed 38400. It
  drains resumed input without parsing until one continuous bounded quiet
  window is observed; it never retries the typed write.
- Finish restores and reads back the original terminal and modem-line state,
  then closes. Raw paths and response payloads stay out of diagnostics.

## Build and validation

```sh
nix develop
just check
just bazel-check
just live-package
just apple-package-check
just apple-bundle-check
just apple-adhoc-bundle
just apple-team-signed-bundle
just product-check
```

Bazel exposes `//:dcxctl` and the macOS-only `//:dcxctl_live_control`; the old
`//:dcxctl_live_discovery` label aliases the latter. Injected Rust tests never
open a device. `//:apple_bundle_sources` and `//:dcx_logic_bridge_schema` make
the native Apple and bridge-schema inputs traversable without duplicating the
XcodeGen build in Bazel. XcodeGen is the checked-in Apple project source;
generated projects and build products are ignored. `just check` stays on the
portable Rust path; `just product-check` adds schema validation and the unsigned
arm64 Apple bundle build.

The Swift package and unsigned arm64 helper/AUv3 bundle compile locally. That is
build evidence only: signed PZM artifacts, `auval`, Logic discovery/insertion,
named-device snapshot, live apply/readback/rollback, and audio flow remain
separate runtime qualifications until their receipts exist.

`just apple-adhoc-bundle` builds the Release carrier unsigned, then signs the
bundled `dcxctl`, AUv3, and containing app from the inside out with ad-hoc
signatures. It verifies arm64 binaries, signatures, entitlement separation,
bundle/component identifiers, and the absence of provisioning profiles. This is
the bounded carrier for helper-absent AU discovery and `auval`; because an ad-hoc
signature has no team identifier, the carrier uses qualification-only app and AU
entitlements that omit the production App Group and serial access. It does not
qualify App Group access, helper IPC, installation, registration, Logic, or
device behavior.

`just apple-team-signed-bundle` builds Release unsigned and then manually signs
the bundled `dcxctl`, AUv3, and containing app from the inside out with one exact
Apple Development or Developer ID Application identity from team
`QP994XQKNH`. It uses the production entitlement split, including the
team-prefixed macOS App Group `QP994XQKNH.io.tinyland.dcx2496` and helper-only
serial access, while requiring that the app and extension contain no
provisioning profiles. Exactly one valid same-team identity must be visible by
default. Set `DCX_CODESIGN_IDENTITY` to one exact certificate SHA-1 or full
identity label to select among multiple valid identities; set
`DCX_CODESIGN_KEYCHAIN` to bind discovery and signing to one host-owned
temporary keychain. A noninteractive caller can additionally set
`DCX_CODESIGN_KEYCHAIN_PASSWORD_FILE` to a caller-owned password file; the
recipe uses it only to unlock that selected keychain before discovery and again
after the unsigned archive, immediately before signing. The recipe verifies
arm64 binaries, nested signatures, the exact team and selected leaf authority,
exact entitlements, and `aumi/DcxC/TnLd` metadata. It never imports the signing
credential and does not install or register the app, launch a GUI, or run
`auval`.

New work is licensed under either Apache-2.0 or MIT, at your option. See
`NOTICE`, `LICENSE`, and `LICENSE-APACHE`.
