# Gated Darwin discovery

The live surface is present only in the `dcxctl-live` Nix package and the
Cargo/Bazel `live-discovery` feature. It can send only the fixed eight-byte
Search request. The default `dcxctl` package and `//:dcxctl` Bazel target remain
offline-only and contain no live subcommands.

This repository owns the typed carrier and its receipt contract. Legalab owns
hardware authorization, PZM evidence, exact artifact delivery, private input,
and execution. Do not substitute a direct shell session, raw SSH command, local
developer build, port enumeration, or hand-authored envelope for the Legalab
ceremony.

## CI and immutable artifact state

This repository has no direct GitHub Actions workflow. The accepted portable
source-producer design extends the existing protected lab workflow
`tinyland-inc/lab/.github/workflows/release-audit.yml` without involving Neo.
Its closed mode is `dcx-source-validation`: it accepts only an exact 40-hex DCX
revision, executes on the on-prem `tinyland-nix` GloriousFlywheel lane, and,
after the producer lands and succeeds, posts the SHA-bound status context
`tinyland/lab-onprem-dcx-source`. Until that exact signed lab commit is merged,
the source proof and status are unavailable. The Linux proof exercises only the
portable Cargo, Bazel, fixture, lockfile, and security graph; it does not compile
or test the cfg-gated Darwin live module.

There is currently no admitted aarch64-Darwin builder. PZM is not an SSH/Nix
builder, its GitHub Actions labels are unregistered, and the future
GloriousFlywheel Darwin REAPI carrier is not admitted. Therefore no runnable
`dcxctl-live` artifact exists, and Legalab must represent native artifact
availability as blocked/unavailable rather than substitute a Linux build or
local developer output.

Once a Darwin producer is separately ratified and admitted, the immutable
artifact must be named `dcxctl-live-<40-hex-merge-sha>` and contain:

```text
binary.sha256
closure-path-info.json
flake-lock.sha256
nix-cache/
out-path.txt
source-revision.txt
source-tree.txt
```

`nix-cache/` is a file-backed Nix binary cache containing the exact recursive
closure. `closure-path-info.json` records every store path's SRI `narHash`,
`narSize`, and references, including the root output. The remaining files bind
the output path, executable, flake lock, exact clean source revision, and Git
tree. The future producer receipt must also bind its trusted event/ref,
dispatcher and runner revisions, run ID/attempt, artifact ID/URL, and the
GitHub-computed artifact digest.

The cache is intentionally not a generally trusted substituter. Legalab must
select the exact successful merged-main run, verify the GitHub artifact digest,
verify all manifest files and recursive path metadata, then copy that exact
closure without activating a profile. A local `nix build`, a pull-request
artifact, a differently named artifact, or an artifact from a non-main event is
not admissible hardware evidence.

The following commands are build/validation operations only; neither opens a
TTY. The live-specific commands require Darwin and intentionally exit
unavailable elsewhere:

```sh
nix develop --command just bazel-check
nix develop --command just live-discovery-check  # admitted Darwin only
nix build .#dcxctl-live --no-link
```

## Typed execution policy

The feature-gated executable exposes three native subcommands under
`discovery`: `prepare`, `live`, and `repeat`. They accept one strict JSON object
from redirected stdin, reject interactive stdin and unknown fields, and cap
input at 16 KiB. They do not enumerate or accept arbitrary ports, frames,
queries, baud rates, retry counts, or timeouts.

The input reader uses one fixed, non-growing allocation and explicitly clears
it on normal, error, and drop paths. That is bounded process hygiene, not a
compiler-guaranteed whole-process zeroization claim: JSON deserialization owns
separate path and authorization strings until validation consumes and drops
them. Those values remain private to the process and are never emitted or
persisted.

The Legalab-owned envelope binds all of the following:

- current PZM LocalHostName, hardware model, OS build, boot digest, and exact
  executable digest;
- one private `/dev/cu.usbserial-*` callout digest, with the raw path retained
  only in process memory;
- the supported standard non-LE DCX2496, firmware 1.17, rear RS-232 connection,
  disconnected speakers, and expected device ID 0 from
  `fixtures/profiles/safe-muted-v1.json`;
- one exact `legalab.decision-record/v1` `dec-*` reference for the physical
  declarations, one exact `legalab.claim-record/v1` `clm-*` reference for the
  adapter electrical proof, and one exact `legalab.review-reference/v1` `rev-*`
  reference for the carrier review, each with its content digest;
- distinct `dcxSafeMutedProfileDigest` and `legalabIntegrationProfileDigest`
  bindings pinned respectively to
  `sha256/177836a70709a1a12c9bbf52d9a359c691d91c6dcfabe729a1e16b546cf60b0d`
  and
  `sha256/a75064b31387ebf4720218eb58bd542f0931460c38bd21f9511565ab7add2436`,
  plus a fresh passive receipt, source revision and tree, issue time, and an
  expiry no more than 15 minutes later;
- the exact Search-only byte, baud, deadline, fallback, repeat, and response
  ceilings.

Native code validates only the closed record-kind/ID syntax and canonical
content-digest shape of those three Legalab references; it does not resolve the
Legalab SSOT. Legalab must resolve `operatorPhysicalEvidence` to the applicable
ratified physical decision and `adapterElectricalEvidence` to an applicable
observed electrical claim—an interview item or operator report cannot satisfy
that claim. It must resolve `carrierReviewEvidence` to a satisfied NATIVE review
pointer whose carrier revision, DCX source revision/tree, review scope, and
content digest all match this packet. The runtime NATIVE token and review
receipt remain outside Git; only the sanitized `rev-*` pointer may persist.

`prepare` observes and validates the current runtime, constructs the canonical
packet, and returns only a sanitized semantic action plus packet digest without
opening the TTY. It emits neither the packet nor a WORD. Legalab independently
recomputes the packet digest, maps `search` or `repeat` to the closed WORD kind,
and derives `WORD DCX_QUERY_V1 <packetDigest>` for `search` or
`WORD DCX_QUERY_REPEAT_V1 <packetDigest>` for `repeat` only inside its attended,
nonredirectable operator surface. The concrete WORD never reaches stdout. The
otherwise identical authorized envelope is then passed privately
to `live` or `repeat`. A reboot, expiry, binding change, executable change,
profile/evidence/review change, source change, or physical-declaration change
invalidates authorization.

`live` performs one Search policy:

1. attempt 115200 baud, 8N1, no flow control, with a 500 ms whole-attempt
   deadline;
2. try 38400 exactly once only if the primary attempt reached its deadline with
   zero received bytes;
3. stop on any partial response, identity failure, transport failure, overflow,
   cleanup failure, or other non-empty primary outcome.

Every attempt opens only the pre-bound callout with `O_NOCTTY`, nonblocking,
`O_NOFOLLOW`, and `TIOCEXCL`; rejects queued input before and after raw serial
configuration; performs one write syscall containing
`F0002032200E40F7`; reads at most 26 bytes; restores termios and control-line
state; reads both back (`tcgetattr` and `TIOCMGET`) and reports
`verified_restored` only when the restore syscalls succeed and readback matches
all four saved termios flag sets, every `NCCS` special-code byte, both speeds,
and the exact saved modem-control bits; and closes. Any restore write, readback,
or equality failure is terminal `cleanup_failed`. Descriptor ownership is still
dropped. It never flushes input or toggles DTR/RTS.

`repeat` requires the complete canonical packet and successful receipt body
from `live`, rehashes both, and checks every runtime, evidence, physical, and
response binding. It then performs exactly nine Searches at the already proven
baud, waits at least 500 ms before each trial, and caps the session at ten
seconds. It never restarts fallback discovery. The first timeout, identity,
transport, pacing, budget, or cleanup failure ends the session.

## Receipt and failure contract

Successful `prepare` output uses
`dcx.native-discovery-prepare-response/v1` and contains only its exact sanitized
action/digest/effects shape. A failure before packet validation uses the distinct
`dcx.native-discovery-gate-failure/v1` shape with `failureBodyDigest`; it never
masquerades as a native attempt receipt. Native attempt results use
`dcx.native-discovery-receipt/v1`. They include the canonical public packet,
packet and body digests, selected baud, observed device ID, per-attempt byte
counts and response digests, monotonic elapsed time, cleanup results, and an
explicit effects record. They never include the raw path, WORD, or raw response.
All failure shapes are sanitized, independently hashable, and exit nonzero.

All v1 JSON digests use the same canonical bytes: recursively sort object keys
lexicographically; preserve array order; encode compact UTF-8 JSON with no
insignificant whitespace; use standard JSON string escaping; and encode the
schema's unsigned integers in shortest decimal form. Hash those bytes with
SHA-256 and encode lowercase `sha256/<64hex>`. `packetDigest` covers the whole
packet. To recompute `receiptBodyDigest` or `failureBodyDigest`, remove only that
digest field from the respective result object and canonicalize everything
else. V1 digest-bearing packet and receipt fields do not admit floating-point
numbers.

A successful first receipt proves one valid 26-byte identity response. A
successful repeat receipt proves nine additional valid responses at the pinned
baud. Together they are the required 10/10 native observation. The receipts
explicitly report zero configuration writes and no audio-routing, sound, clock,
driver, system-extension, SIP, or persistent-host changes.

## Legalab operator entrypoint

Live execution is performed only from the Legalab repository through its
hardware gate and the lab-owned status-preserving remote wrapper. Its four
separately attended front-door phases are:

```sh
just dcx-native-prepare-search SANITIZED_MANIFEST
just dcx-native-execute-search SANITIZED_MANIFEST
just dcx-native-prepare-repeat SANITIZED_MANIFEST SEARCH_RECEIPT
just dcx-native-execute-repeat SANITIZED_MANIFEST SEARCH_RECEIPT
```

The sanitized manifest is not runnable until an admitted Darwin artifact and
complete recursive closure are available. When that producer exists, the
manifest binds its exact merged-main artifact evidence, fresh current-boot
passive evidence, resolved decision/claim/review references, host identity, and
both profile digests. The remote runner derives
the one rooted FTDI callout on PZM. Each prepare phase returns only its
sanitized semantic action and packet digest; the attended Legalab surface
derives and displays that phase's concrete WORD transiently. Each execute phase
accepts its separately attended WORD only through private stdin. The Search
receipt is then the exact input to repeat preparation, and repeat execution
emits the final sanitized session receipt. No phase accepts a PZM alias, and no
single remote invocation combines all four phases.

Do not invoke `dcxctl discovery live` or `repeat` directly for a Legalab proof.
Do not place a raw callout or WORD in an argument, environment variable, file,
log, terminal transcript, or workflow artifact. No direct invocation may widen
the Search-only authority or satisfy the cross-repository evidence gate.
