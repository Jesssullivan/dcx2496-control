# DCX2496 preset store, recall and directory: research and probe plan

Date: 2026-10-21. Authority: R-HOOK-CONVERGENCE-20261004 (R-N13). Device
writes stay under Legalab `dec-dcx-attended-software-writes-20261019`
(P-TIN-170 comment `131db306`, ruling 2). Linear: legalab TIN-6827.

This lane read public sources and this repository only. It opened no serial
device, sent no DCX traffic, and copied no third-party code. No vendor PDF,
binary or disassembly is committed.

## Question and stop rule

Can `dcxctl` store the current setup to an internal preset slot, recall a
slot, and read the preset directory (slot numbers and names) over the
existing 38400 8N1 RS-232 session, with frames that are documented well
enough to encode offline and test from published examples?

Stop rule: implement `dcxctl preset` only if store, recall and directory read
each have a function code, payload layout and response shape from at least
one public source, preferably two. Otherwise record the evidence and the
attended probe that would close the gap, and add no store command.

## Verdict

**Not established. No `dcxctl preset` command was added.** None of the public
sources read in this lane (section 1) gives a function code, payload or
response for preset store, recall, directory read or preset-name read. Every
public implementation uses only the five functions this repository already
models (Search `0x40`, Ping `0x44`, Dump `0x50`, remote mode `0x3f` and
direct parameter `0x20`), plus their responses (`0x00`, `0x04`, `0x10`). An
encoder would have to guess those bytes. The repository forbids that
("Unknown identity, state, route, or value is an error"), and a wrong guess
could overwrite, clear or lock a slot, or erase the card bank.

Behringer's Sync Manual (S12) also suggests that the vendor editor may not
use three per-slot device functions. It moves the current set and whole
preset banks in both directions, then recalls and stores against its synced
copy. The three-function split in section 3 is therefore an assumption, not
a finding.

The operator's goal, the setup stored programmatically, is mostly reachable
without a preset command (section 4).

## 1. Sources

| ID | Source | Revision / access | License | What it covers |
| --- | --- | --- | --- | --- |
| S1 | UltradrivePi `protocol.md` (geftactics) | `259fa83f`, re-read 2026-10-21 | none (read only) | header, `0x3f`, `0x20`, parameter list |
| S2 | "DCX protocol" community PDF, jipihorn.free.fr mirror | sha256 `2b5c6875...7b66`, 2 pages, read 2026-10-21, not committed | unknown (read only) | Same text as S1. S1 is evidently a transcription of it. |
| S3 | DuinoDCX `Ultradrive.cpp` / `.h` (lasselukkari) | `00b9d70d` | MIT | `0x40`, `0x44`, `0x50`, `0x3f 0c`, `0x20`; responses 0/4/16/32 |
| S4 | pyDCX `protocoll.txt` (theProgramm) | `aa6d0455` | MIT | `0x3f`, `0x40`, `0x44`. Notes that Ping with nonzero data "produces somewhat different output". The list ends at an unfinished `0x` entry. |
| S5 | pyDCX `DCX2496/device.py` (susnux) | `43fba285` | MIT | `0x40`, `0x3f`, `0x44`, `0x50`, `0x20` only |
| S6 | DCX2496-ctrl-analyze `dcx_map.py` (domenut) | `97cdcca1` | none (read only) | `0x50`, `0x3f`, `0x20`, `0x44`. Its "load set" replays every direct parameter from a saved dump and writes no preset. |
| S7 | DCX2496_Serial_Control_Win (PatrickMassler92) | `c2d54ebf` | none (read only) | `0x3f 04`, `0x20` only |
| S8 | DCX2496Communication `dcx_data_encoder.py` (JonathanHuthmann) | `1506d32c` | none (read only) | Says DCX-Remote "sends and receives presets ... via COM-Port". It encodes an unpacked `.xpc` "Save Set" file into 7-of-8 packing, and decodes a separate capture. No function code. |
| S9 | WinRS-DCX `duinodcx-rs/src/ultradrive.rs` (ChongZhiJie0216) | `f33ca166` | MIT | Rust port of S3, with the same five functions. |
| S10 | Behringer DCX2496 user manual, sections 4.7 RECALL and 4.8 STORE (manualsdump.com mirror, pp. 16-17) | read 2026-10-21 | vendor (read only) | See below. |
| S11 | Stute Engineering DCX.Mixer FAQ; DCX.Client 2 video description | read 2026-10-21 | proprietary | A third-party Windows app stores and reloads DCX setups as `.DCC` files on the PC. Closed source; no protocol. |
| S12 | Behringer "DCX2496 Sync Manual" v1.0, September 2003 (docs.ampnuts.ru mirror) | sha256 `bf9260f1...6ff5`, read 2026-10-21, not committed | vendor (read only) | See below. |
| S13 | OpenDCX (gespel) `0577e449`; mediacenter (floor12) `lib/Ultradrive.py` `60a61e37` | found by the PR review, 2026-10-21 | none (read only) | `0x3f` and `0x20` only |
| R1 | This repository: `crates/dcx-core/src/protocol.rs`, `docs/typed-writes.md` | `262375f` | MIT OR Apache-2.0 | The five modelled functions and the O5/O6 never-unmute rule. |
| R2 | Legalab `docs/dcx2496-dump-frame-format.md`, `docs/agent-notes/2026-10-19-dcx-dump0-101.md` | legalab main `47bc246` | private | See below. |

What S10 covers:

- 60 internal slots, plus a PCMCIA card bank.
- A preset needs a name to be stored, and an empty slot can't be recalled.
- A locked preset can't be overwritten or deleted. Formatting the card
  erases even locked presets.
- CURRENT NR shows the current preset, with "EDITED" under it after an
  edit.
- Presets can be stored externally on a PC by "data dump via RS-232".
- AUTO-STORE saves all settings every few seconds to a temporary,
  non-volatile memory, and it can't be disabled.

What S12 covers:

- DCX-Remote synchronizes in a chosen direction, DCX2496 -> PC or
  PC -> DCX2496.
- It synchronizes "Current Set", "Internal Presets" and "Card Presets",
  each selected separately.
- Its "Recall nr.." menu works on user presets only after an internal-preset
  sync. Its "Store to.." menu can also "clear or lock presets".
- PC -> DCX2496 overwrites what is on the device.

What R2 records:

- Dump0/Dump1 payload record magics `XPCR`, `XPRB`, `XCUR` and `XPRE`.
- The `XCUR` header reads 23 at decoded 73.
- Decoded 77 (frame 101) flipped 1 -> 0 after a front-panel edit.

Searched without a protocol result:

- GitHub repository search for `dcx2496` and `ultradrive`.
- GitHub code search for `DCX2496`, `XPCR`, "DCX2496 preset", "DCX2496
  recall" and "Ultradrive preset".
- DuckDuckGo and web searches for DCX2496 RS-232/SysEx preset recall.
  SearXNG returned no usable results.
- The diyaudio and prosoundweb threads were behind bot walls and were not
  read.

## 2. Claims

| # | Claim | Class | Sources | Confidence |
| --- | --- | --- | --- | --- |
| C1 | None of the sources read documents a preset store, recall, directory or name-read function. | observed (negative, over the sources read) | S1-S9, S13 | High for these sources. It doesn't prove that no such function exists. |
| C2 | The firmware supports preset transfer over RS-232: the current set and the internal and card banks, in both directions. | vendor claim | S10, S12, S8 | High that the transfer exists. The function codes are unknown. Whether recall and store are device functions, or operations on the PC's synced copy followed by a set or bank write, is also unknown. |
| C3 | Dump0 + Dump1 decoded hold preset-shaped records (`XPCR`/`XPRB`/`XCUR` in Dump0, `XPRE` in Dump1), so a complete snapshot is a byte-exact image of the current set. The `XPC` prefix matches the `.xpc` file extension (S8). | inferred | R2, S8 | Medium. The magics are observed. Equivalence to the unpacked `.xpc` layout is not checked. |
| C4 | Decoded Dump0 73 (frame 96, carrier frame 100 bit 3) is the current preset number (CURRENT NR). Decoded 77 (frame 101) is a "not edited" flag: front-panel edits clear it, direct-parameter writes don't. | inferred | R2, S10 | Medium-low. This adds S10 to the 10-19 H-edit hypothesis. P0a and P1 confirm or falsify it. |
| C5 | AUTO-STORE keeps the current set in non-volatile memory up to its last save, a few seconds after the last edit, so it survives a power cycle without a preset store. That typed writes are covered is inferred. | vendor claim, unverified on the named unit | S10 | Medium. P2 checks it. |
| C6 | A preset recall or a PC -> DCX sync replaces the whole set, including the O5/O6 mutes. It bypasses every typed guard: cut-only EQ, the gain ceiling and the O5/O6 never-unmute rule. | inferred | S10 ("all settings will be lost"), S12 | High enough to design against. |
| C7 | Ping (`0x44`) with nonzero data returns something other than meters. | unverified | S4 | Low. Not a preset lead; never to be probed blind. |
| C8 | The `01 00` in the Dump request (`50 01 00 <part>`) might select a bank or the current set. | unverified | S3, S5, S6 | Low. Never to be probed blind. |

## 3. Why no command, and what one would need

A `dcxctl preset` built on this evidence would need invented function bytes.
The contract requires closed, typed operations. Assuming per-slot device
functions exist (S12 leaves this open), each of these is needed first:

1. **Directory read (read-only, first).** The function code, request payload,
   response function, exact response length, and the per-slot record:
   number, name, empty flag and lock flag. Without it, software can't verify
   a store or find a free slot.
2. **Store.** The function code and payload: slot 1-60, internal bank only,
   an 8-character name in the device's character set, plus anything else the
   device needs. Also whether it returns an acknowledgement, and how it
   treats a locked or occupied slot.
3. **Recall.** The function code and payload. Recall is a whole-set write
   (C6). A future recall must refuse every slot except one that `dcxctl`
   itself stored from a verified snapshot with O5/O6 muted. It must compare
   a post-recall snapshot digest with that stored snapshot's digest, under
   the same complete-readback rules as `control apply`.

If the observed mechanism is instead a bank transfer, a store becomes a
read-modify-write of a 60-slot bank. That is a larger blast radius, and it
needs its own design and ruling.

The planned design for per-slot functions, recorded for the implementation
lane:

- `dcxctl preset list|store --slot N --name NAME|recall --slot N`, with
  `--dry-run` printing the exact frames.
- Slots 1-60, internal bank only.
- The name is exactly 8 printable ASCII characters, so the caller pads it.
  That removes any ambiguity about trailing spaces.
- The card bank, delete/clear, copy, lock and format are never exposed.
- **Store:** take a fresh G1 snapshot S, send the store frame, and re-read
  the directory: the slot must show NAME. Then take a second snapshot. It
  must equal S except for the C4 header fields. The receipt records S's
  digest as the slot's expected image.
- **Recall:** refuse unless the slot has a receipt whose snapshot had O5/O6
  muted. After the recall frame, take a complete snapshot. Its digest must
  equal the receipt digest, ignoring the C4 fields. A mismatch reports
  `state_unresolved`. Nothing re-recalls automatically.
- **Budget:** count the store or recall frame and its remote-mode writes
  against the attended-write ruling. That ruling covers typed
  direct-parameter writes only, so Legalab must add preset frames to it
  before any live use.

## 4. What already gives a programmatic store today

- **Off-device archive (works now).** `dcxctl control snapshot` captures the
  complete Dump0 + Dump1 image with a digest. Keep it, with its receipt, in
  the private state directory. Under C3 it is the whole current set.
  Restoring from it is limited to the reviewed typed and routing addresses
  (`write plan` against a fresh snapshot). A whole-image write has no
  evidenced function and no address allowlist, so it stays out of scope.
- **On-device persistence (vendor claim C5).** If P2 confirms it on this
  unit, the current set already survives power cycles through AUTO-STORE.
- **Named preset slot.** Front-panel STORE (manual section 4.8) is the
  evidenced path today. P1 makes it verifiable by software.

## 5. Attended probe plan

Run in this order. P0-P3 send no direct-parameter frame and no preset frame.
Their only traffic is ordinary G1 snapshots, which do send remote-mode
`0x3f` writes. In P4 only the tap is passive. DCX-Remote itself sends
unmodelled frames to the device (search, remote mode, and current-set and
bank read requests), whose codes and effects are unverified. P4 therefore
needs its own decision, with the outputs made safe first. Hardware readiness, audio safety and the bench
lock belong to Legalab (`legalab-hardware-gate`); this repository does not
duplicate them.

| Step | Operator action | Software | Expect / decide |
| --- | --- | --- | --- |
| P0 | none | `control snapshot` -> S0 | Record the digest, decoded 73 (frame 96, carrier 100 bit 3), frame 101, and the name bytes (frames 103-107 and 109-111). Record whether O5 and O6 read muted. |
| P0a | Front panel: open RECALL and read CURRENT NR and whether "EDITED" shows. Then CANCEL. Change nothing. | none | Under C4, CURRENT NR equals the decoded-73 value P0 just recorded (or that value + 1 if 1-based). EDITED shows exactly when frame 101 reads 0. A mismatch weakens C4 before anything is stored. |
| P1 | Front panel: STORE -> INTERNAL. Pick an **empty** slot N (the store list shows used slots). Name it with 8 characters, e.g. `LGL21OCT`, then OK. Touch nothing else. | snapshot -> S1 | C4 is confirmed if decoded 73 becomes N (or N-1), frame 101 becomes 1, the name bytes read the new name, Dump1 is identical and the trailer stays balanced. Any other change: stop and record it. |
| P2 (optional) | Wait at least 30 s after P1. Follow the `legalab-hardware-gate` audio-safety step with amplifiers off. Power-cycle the DCX. | snapshot -> S2 | C5 is confirmed if S2 equals S1 byte for byte. |
| P3 (optional) | Only if S1 shows O5 and O6 muted. Front panel: RECALL -> INT slot N, the slot stored in P1. Never any other slot. | snapshot -> S3 | Recall reproduces the stored image if S3 equals S1. |
| P4 (separate window, its own decision) | Run DCX-Remote, from Behringer's own distribution, on a Windows host through a passive RS-232 tap, RX-only on both directions and carrying DTR/RTS (the editor depends on them, per the ChongZhiJie0216 README). Use only Direction DCX2496 -> PC: sync Current Set, then Internal Presets. Never use PC -> DCX2496, "Quick Synchronize To DCX", "Stay Synchronized", Store to.., Clear or Lock, and never touch an editor control. PC -> DCX2496, Store to.., Clear and Lock are unguarded whole-set or whole-bank writes. Quick Synchronize and Stay Synchronized are excluded as a precaution: S12 describes the first as receiving data, and the second pushes PC edits live. | Capture privately, outside git. | Publish only the function codes, payload layout and response lengths. With store and recall forbidden, P4 shows only the current-set and bank read frames. A bank read is the read-only listing a directory would come from. It does not show whether per-slot store or recall functions exist. |

P5 is an alternative to P4 with no device involved, and it needs an operator
and legal decision. DCX-Remote 1.16a is publicly mirrored
(`ChongZhiJie0216/Behringer_DCX2496`, no license). A one-off `strings` pass over
that mirrored binary during the PR review shows UI strings including "Recall
nr..", "Store to..", "Clear Preset" and "Destination Preset is Locked". The
reviewer's copy was deleted afterwards, and whether even that read fits the
RE scope is the operator's call. Static analysis could identify the
frames. Legalab's
reverse-engineering permission covers the LS56 only
(`dec-ls56-re-permitted-20261014`), so it is listed as an option and was
not done. Nothing derived from the binary may be committed.

After P4 or P5, a read-only directory or bank read can be built and
probed once, under the Ping/Search pacing. Store and recall follow only after
that read is observed and Legalab extends the write ruling. Never probe
unknown function codes blind: one of them could overwrite, clear or lock a
slot.

## 6. Operator decisions

1. Run P0, P0a and P1, and optionally P2 and P3, at the next attended
   window. They need no direct-parameter writes and take about 10 minutes.
   P0a and P1 also close the open Dump0[101] question.
2. Whether a Windows host and a passive RS-232 tap are available for P4, or
   whether to authorize P5 instead.
3. Whether Legalab extends `dec-dcx-attended-software-writes-20261019` to
   preset frames once P4 or P5 has identified them.
