# Gated Darwin discovery

The live surface is present only in the `dcxctl-live` Nix package and the
Cargo/Bazel `live-discovery` feature. It can send only the fixed eight-byte
Search request. Keep envelopes, WORD values, native receipts, and the raw tty
path in an operator-private location outside Git.

## Build and transfer

```sh
live_store=$(nix build --no-link --print-out-paths .#dcxctl-live)
nix copy --to \
  'ssh-ng://pzm?remote-program=/nix/var/nix/profiles/default/bin/nix-daemon' \
  "$live_store"
printf 'Exact copied path: %s\n' "$live_store"
ssh -t pzm
```

This materializes and copies the exact immutable Nix store closure, verified by
its NAR hash; it does not activate a profile or open a tty. `just live-package`
performs the local build. After `ssh -t pzm`, set `live_store` inside that PZM
shell to the exact printed `/nix/store/...` path:

```sh
live_store=/nix/store/<exact-path-printed-on-the-build-mac>
```

Run every remaining command in this document inside that PZM shell. In
particular, prompt for the private callout path only on PZM; never place it in
an SSH command, argument, environment variable, local-build-host variable, or
file. Do not run the copied binary on the build Mac because LocalHostName is
part of the gate.

## First Search

Create a sanitized public-field template outside Git, omitting
`privateTtyPath` and `authorization`. Replace every synthetic value with fresh
current-boot evidence and Unix timestamps. Digests use exact lower-case
`sha256/<64 hex>` form; revisions and trees use 40 lower-case Git hex. The
following full object documents the strict shape, but never persist a version
containing the real raw path or WORD.

```json
{
  "schemaVersion": "dcx.live-discovery-envelope/v1",
  "action": "search",
  "privateTtyPath": "/dev/cu.usbserial-SYNTHETIC",
  "bindingDigest": "sha256/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "expectedHost": {
    "role": "petting-zoo-mini",
    "hardwareModel": "Mac16,10",
    "osBuild": "25G220"
  },
  "expectedDeviceId": 0,
  "physical": {
    "powered": true,
    "edition": "standard-non-le",
    "firmwareVersion": "1.17",
    "portMode": "RS-232",
    "rearRs232Connected": true,
    "adapterRs232ElectricalVerified": true,
    "speakersDisconnected": true
  },
  "evidence": {
    "passiveReceiptDigest": "sha256/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "passiveCapturedAtUnixSeconds": 1787520000,
    "adapterElectricalEvidenceDigest": "sha256/cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    "profileDigest": "sha256/177836a70709a1a12c9bbf52d9a359c691d91c6dcfabe729a1e16b546cf60b0d",
    "sourceRevision": "PLACEHOLDER-40-HEX-GIT-COMMIT",
    "sourceTree": "PLACEHOLDER-40-HEX-GIT-TREE"
  },
  "issuedAtUnixSeconds": 1787520000,
  "expiresAtUnixSeconds": 1787520900,
  "priorSearch": null,
  "authorization": null
}
```

`expectedDeviceId: 0` is the desired address declared by the
`tinyland-legalab-pzm-dcx2496` safe profile and bound by the exact current
`profileDigest` above; it is not presented as a front-panel observation. The
packet labels that basis `desired_profile_declared`. A successful Search proves
the address from the wire, and an unexpected response emits the sanitized
observed ID before stopping. Prepare performs host, boot, executable, binding, time,
declaration, and digest validation without opening the tty. Read the raw callout into a non-exported
shell variable, inject it through `jq` stdin, and pipe the completed envelope
directly to the remote process:

```sh
read -r -s -p 'Private tty callout: ' tty_path; printf '\n' >&2
prepared_search=$(printf '%s\n' "$tty_path" |
  jq -Rn --slurpfile base search-public.json \
    'input as $tty | $base[0] + {privateTtyPath: $tty, authorization: null}' |
  "$live_store/bin/dcxctl" discovery prepare)
unset tty_path
printf '%s\n' "$prepared_search" | jq 'del(.requiredWord)' > prepared-search-sanitized.json
```

Review the emitted packet and exact `requiredWord`. Put that exact WORD string
in the otherwise unchanged envelope's `authorization` field, then execute:

```sh
word=$(printf '%s\n' "$prepared_search" | jq -r .requiredWord)
read -r -s -p 'Private tty callout: ' tty_path; printf '\n' >&2
{ printf '%s\n' "$tty_path"; printf '%s\n' "$word"; } |
  jq -Rn --slurpfile base search-public.json \
    '[inputs] as $private | $base[0] + {privateTtyPath: $private[0], authorization: $private[1]}' |
  "$live_store/bin/dcxctl" discovery live > first-search-receipt.json
unset tty_path word prepared_search
```

Success exits zero and records one wire-observed device ID. Exhaustion or any
gate, carrier, identity, deadline, overflow, or cleanup failure emits one
sanitized JSON receipt to stdout and exits nonzero. The raw path, WORD, and raw
response never appear in packet or receipt output.

## Nine-trial repeat

The repeat envelope has the same outer fields, `action` is `repeat`, and
`priorSearch` is required. Populate it only from the exact successful Search
receipt:

```json
{
  "firstPacket": { "copy": "the complete receipt.packet object" },
  "firstPacketDigest": "copy receipt.packetDigest",
  "firstReceiptBody": { "copy": "the complete receipt except receiptBodyDigest" },
  "firstReceiptBodyDigest": "copy receipt.receiptBodyDigest",
  "firstResponseDigest": "copy the sole receipt.rxDigests value",
  "bindingDigest": "copy receipt.packet.bindingDigest",
  "expectedDeviceId": 0,
  "successfulBaud": 115200,
  "firstHostIdentityDigest": "copy receipt.packet.host.hostIdentityDigest",
  "firstBootDigest": "copy receipt.packet.host.bootDigest",
  "firstExecutableDigest": "copy receipt.packet.host.executableDigest",
  "firstProfileDigest": "copy receipt.packet.operatorDeclaredEvidence.profileDigest",
  "firstPassiveReceiptDigest": "copy receipt.packet.operatorDeclaredEvidence.passiveReceiptDigest",
  "firstAdapterElectricalEvidenceDigest": "copy receipt.packet.operatorDeclaredEvidence.adapterElectricalEvidenceDigest",
  "firstSourceRevision": "copy receipt.packet.operatorDeclaredEvidence.sourceRevision",
  "firstSourceTree": "copy receipt.packet.operatorDeclaredEvidence.sourceTree",
  "firstPhysicalDigest": "copy receipt.packet.operatorDeclaredPhysicalDigest"
}
```

`firstPacket` and `firstReceiptBody` above are whole JSON objects, not the
illustrative `{ "copy": ... }` placeholders. The CLI canonicalizes and rehashes
both objects, validates the exact primary-only or empty-primary-plus-fallback
history, and rejects host/boot/executable/profile/source/evidence/physical drift.
The passive capture timestamp embedded in `firstPacket` must equal the current
repeat template's `evidence.passiveCapturedAtUnixSeconds`.

```sh
read -r -s -p 'Private tty callout: ' tty_path; printf '\n' >&2
prepared_repeat=$(printf '%s\n' "$tty_path" |
  jq -Rn --slurpfile base repeat-public.json \
    'input as $tty | $base[0] + {privateTtyPath: $tty, authorization: null}' |
  "$live_store/bin/dcxctl" discovery prepare)
unset tty_path
printf '%s\n' "$prepared_repeat" | jq 'del(.requiredWord)' > prepared-repeat-sanitized.json

word=$(printf '%s\n' "$prepared_repeat" | jq -r .requiredWord)
read -r -s -p 'Private tty callout: ' tty_path; printf '\n' >&2
{ printf '%s\n' "$tty_path"; printf '%s\n' "$word"; } |
  jq -Rn --slurpfile base repeat-public.json \
    '[inputs] as $private | $base[0] + {privateTtyPath: $private[0], authorization: $private[1]}' |
  "$live_store/bin/dcxctl" discovery repeat > repeat-receipt.json
unset tty_path word prepared_repeat
```

Repeat is fixed at nine same-baud trials, at least 500 ms apart and within ten
seconds. A WORD is stateless and can be replayed until its maximum 15-minute
expiry; the binary does not claim one-use consumption. Reboot or any bound
runtime/evidence change invalidates the packet digest.

Only sanitized prepared/native stdout may be retained, and it remains outside
Git until Legalab ingests a reviewed receipt. Never persist, log, or add shell
tracing around the completed raw envelope, callout path, or WORD.
