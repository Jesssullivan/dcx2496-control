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

## Immutable artifact contract

Every pull request compiles and tests the gated package. Only a successful
`push` workflow for merged `main` exports a runnable artifact named
`dcxctl-live-<40-hex-merge-sha>`. The artifact contains:

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
closure. `closure-path-info.json` records every store path's `narHash`,
`narSize`, and references. The remaining files bind the output path, executable,
flake lock, merge revision, and Git tree. The workflow summary records the
GitHub run ID, artifact ID, artifact URL, and GitHub-computed artifact digest.

The cache is intentionally not a generally trusted substituter. Legalab must
select the exact successful merged-main run, verify the GitHub artifact digest,
verify all manifest files and recursive path metadata, then copy that exact
closure without activating a profile. A local `nix build`, a pull-request
artifact, a differently named artifact, or an artifact from a non-main event is
not admissible hardware evidence.

The following commands are build/validation operations only; neither opens a
TTY:

```sh
nix develop --command just live-discovery-check
nix develop --command just bazel-check
nix build .#dcxctl-live --no-link
```

## Typed execution policy

The feature-gated executable exposes three native subcommands under
`discovery`: `prepare`, `live`, and `repeat`. They accept one strict JSON object
from redirected stdin, reject interactive stdin and unknown fields, and cap
input at 16 KiB. They do not enumerate or accept arbitrary ports, frames,
queries, baud rates, retry counts, or timeouts.

The Legalab-owned envelope binds all of the following:

- current PZM LocalHostName, hardware model, OS build, boot digest, and exact
  executable digest;
- one private `/dev/cu.usbserial-*` callout digest, with the raw path retained
  only in process memory;
- the supported standard non-LE DCX2496, firmware 1.17, rear RS-232 connection,
  verified RS-232 electrical mode, disconnected speakers, and desired device ID
  0 from the exact safe profile;
- fresh passive receipt, adapter electrical evidence, profile digest, source
  revision and tree, issue time, and an expiry no more than 15 minutes later;
- the exact Search-only byte, baud, deadline, fallback, repeat, and response
  ceilings.

`prepare` observes and validates the current runtime, constructs the canonical
packet, and returns the exact required WORD without opening the TTY. Legalab
keeps the private path and WORD in remote process memory and immediately passes
the otherwise identical authorized envelope to `live` or `repeat`. A reboot,
expiry, binding change, executable change, profile/evidence change, source
change, or physical-declaration change invalidates authorization.

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
state; and closes. It never flushes input or toggles DTR/RTS.

`repeat` requires the complete canonical packet and successful receipt body
from `live`, rehashes both, and checks every runtime, evidence, physical, and
response binding. It then performs exactly nine Searches at the already proven
baud, waits at least 500 ms before each trial, and caps the session at ten
seconds. It never restarts fallback discovery. The first timeout, identity,
transport, pacing, budget, or cleanup failure ends the session.

## Receipt and failure contract

Native receipts are sanitized JSON. They include the canonical public packet,
packet and body digests, selected baud, observed device ID, per-attempt byte
counts and response digests, monotonic elapsed time, cleanup results, and an
explicit effects record. They never include the raw path, WORD, or raw response.
Failures emit one sanitized receipt to stdout and exit nonzero.

A successful first receipt proves one valid 26-byte identity response. A
successful repeat receipt proves nine additional valid responses at the pinned
baud. Together they are the required 10/10 native observation. The receipts
explicitly report zero configuration writes and no audio-routing, sound, clock,
driver, system-extension, SIP, or persistent-host changes.

## Legalab operator entrypoint

Live execution is performed only from the Legalab repository through its
hardware gate and the lab-owned status-preserving remote wrapper. The owning
entrypoint is:

```sh
scripts/dcx_native_discovery.sh SANITIZED_MANIFEST [PZM_ALIAS]
```

The sanitized manifest binds the verified merged-main artifact, recursive Nix
closure, fresh current-boot passive evidence, operator-ratified electrical and
physical evidence, host identity, and safe profile. The remote runner derives
the one rooted FTDI callout on PZM, retains the raw callout and WORDs only in
memory, runs `prepare` plus `live` plus a separately prepared `repeat`, and emits
only the combined sanitized session receipt.

Do not invoke `dcxctl discovery live` or `repeat` directly for a Legalab proof.
Do not place a raw callout or WORD in an argument, environment variable, file,
log, terminal transcript, or workflow artifact. No direct invocation may widen
the Search-only authority or satisfy the cross-repository evidence gate.
