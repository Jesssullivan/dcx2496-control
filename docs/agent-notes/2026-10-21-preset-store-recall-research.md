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

**Not established. No `dcxctl preset` command was added.** No public source
read in this lane gives a function code, payload or response for preset
store, recall, directory read or preset-name read. Every public
implementation uses only the five functions this repository already models
(Search `0x40`, Ping `0x44`, Dump `0x50`, remote mode `0x3f` and direct
parameter `0x20`), plus their responses (`0x00`, `0x04`, `0x10`). An encoder
for these frames would rest on guessed bytes. The repository forbids that
("Unknown identity, state, route, or value is an error"), and a wrong guess
could store over, delete or format a slot.

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
| S6 | DCX2496-ctrl-analyze `dcx_map.py` (domenut) | `97cdcca1` | none (read only) | `0x50`, `0x3f`, `0x20`, `0x44`. Its "load set" replays every direct parameter from a saved dump. It writes no preset. |
| S7 | DCX2496_Serial_Control_Win (PatrickMassler92) | `c2d54ebf` | none (read only) | `0x3f 04`, `0x20` only |
| S8 | DCX2496Communication `dcx_data_encoder.py` (JonathanHuthmann) | `1506d32c` | none (read only) | Says DCX-Remote "sends and receives presets ... via COM-Port" in the same 7-of-8 packing, and round-trips a `.xpc` "Save Set" file. It gives no function code. |
| S9 | WinRS-DCX `duinodcx-rs/src/ultradrive.rs` (ChongZhiJie0216) | `f33ca166` | MIT | Rust port of S3. Same five functions. |
| S10 | Behringer DCX2496 user manual, sections 4.7 RECALL and 4.8 STORE (manualsdump.com mirror, pp. 16-17) | read 2026-10-21 | vendor (read only) | 60 internal slots plus a PCMCIA card bank. A preset needs a name to be stored, and an empty slot can't be recalled. Locked presets can't be overwritten or deleted, and FORMAT erases even locked ones. CURRENT NR shows the current preset, with "EDITED" once it changes. AUTO-STORE saves all settings to non-volatile memory every few seconds and can't be disabled. RS-232 lets a PC "save and load files". |
| S11 | Stute Engineering DCX.Mixer FAQ, DCX.Client 2 video description | read 2026-10-21 | proprietary | A third-party Windows app stores and reloads DCX setups as `.DCC` files on the PC. Closed source, no protocol. |
| R1 | This repository: `crates/dcx-core/src/protocol.rs`, `docs/typed-writes.md` | `262375f` | MIT OR Apache-2.0 | five modelled functions; risk R4 (a stored preset recall unmutes O6) |
| R2 | Legalab `docs/dcx2496-dump-frame-format.md`, `docs/agent-notes/2026-10-19-dcx-dump0-101.md` | legalab main `47bc246` | private | Dump0/Dump1 payload record magics `XPCR`, `XPRB`, `XCUR`, `XPRE`. The `XCUR` header field at decoded 73 reads 23 and decoded 77 (frame 101) flipped 1 -> 0 after a front-panel edit. |

Searched without a protocol result: the GitHub repository search for `dcx2496`
and `ultradrive`, GitHub code search for `DCX2496`, and DuckDuckGo queries for
DCX2496 RS-232/SysEx preset recall. The built-in web search was unavailable
(session limit); SearXNG returned no usable results.

## 2. Claims

| # | Claim | Class | Sources | Confidence |
| --- | --- | --- | --- | --- |
| C1 | No public source documents a preset store, recall, directory or name-read function. | observed (negative, over the sources read) | S1-S9 | high for these sources. It does not prove that no such function exists. |
| C2 | The device firmware supports PC-side preset transfer over RS-232: Behringer's DCX-Remote saves and loads presets. | inferred | S8, S10 | medium. The function codes and direction are unknown. It is not known whether the transfer can target an internal slot or only the current setup. |
| C3 | Dump0 + Dump1 decoded hold preset-shaped records (`XPCR`/`XPRB`/`XCUR` in Dump0, `XPRE` in Dump1), so a complete snapshot is a byte-exact image of the current setup, in the same 7-of-8 packing as a `.xpc` file. | inferred | R2, S8 | medium. The magics are observed. Equivalence to the `.xpc` layout is not checked. |
| C4 | Decoded Dump0 73 (frame 96, carrier frame 100 bit 3) is the current preset number. Decoded 77 (frame 101) is a "not edited since recall/store" flag that front-panel edits clear and direct-parameter writes do not. | inferred | R2, S10 (CURRENT NR / EDITED) | medium-low. This lane adds S10 to the 10-19 H-edit hypothesis. Probe P1 confirms or falsifies it. |
| C5 | AUTO-STORE keeps the current setup, including every typed write, in non-volatile memory, so it survives a power cycle without a preset store. | vendor claim, unverified on the named unit | S10 | medium. Probe P2 checks it. |
| C6 | A preset recall replaces the whole setup, including the O5/O6 mutes. It bypasses every typed guard (cut-only EQ, gain ceiling, the O5/O6 never-unmute rule). | inferred from the manual's "all settings will be lost" plus R1 risk R4 | S10, R1 | high enough to design against. |
| C7 | Ping (`0x44`) with nonzero data returns something other than meters. | unverified | S4 | low. Not a lead for presets, and not to be probed blind. |

## 3. Why no command, and what one would need

A `dcxctl preset` built on this evidence would need invented function bytes.
The contract requires closed, typed operations, so each of these is needed
first:

1. **Directory read (read-only, first).** The function code, request payload,
   response function, exact response length and per-slot record (number,
   name, empty and lock flags). Without it, a store can't be verified and a
   "free slot" can't be identified by software.
2. **Store.** The function code, payload (slot 1-60, internal bank only,
   8-character name in the device's character set, plus whatever the
   device needs), whether it returns an acknowledgement, and how it treats a
   locked or occupied slot.
3. **Recall.** The function code and payload. Recall is a whole-setup write
   (C6), so a future recall must refuse every slot except one that `dcxctl`
   itself stored from a verified snapshot with O5/O6 muted. It must also
   compare a post-recall snapshot digest to that stored snapshot's digest,
   with the same complete-readback rules as `control apply`.

The planned design, recorded here so the implementation lane has it:

- `dcxctl preset list|store --slot N --name NAME|recall --slot N`, with
  `--dry-run` printing the exact frames. Slot 1-60 internal only. The name
  must be 1-8 printable ASCII characters, with no trailing-space ambiguity.
  The card bank, delete, copy, lock and format are never exposed.
- Store: fresh G1 snapshot S, the store frame, directory re-read (the slot
  must show NAME), then a second snapshot equal to S except the C4 header
  fields. The receipt records S's digest as the slot's expected image.
- Recall: refuse unless the slot's receipt exists and its snapshot had
  O5/O6 muted. After the recall frame, a complete snapshot whose digest
  must equal the receipt digest (modulo C4 fields). A mismatch reports
  `state_unresolved`; there is no automatic re-recall.
- Budget: count the store or recall frame and its remote-mode writes against
  the attended-write ruling. Legalab must add preset frames to that ruling
  before any live use. The current ruling covers typed direct-parameter
  writes only.

## 4. What already gives a programmatic store today

- **Off-device archive (works now).** `dcxctl control snapshot` captures
  the complete Dump0 + Dump1 image with a digest. Keep it in the private
  state directory with the receipt. Under C3 it is the whole current setup.
  Restoring from it is limited to the reviewed typed and routing addresses
  (`write plan` against a fresh snapshot). A whole-image write has no
  evidenced function and no address allowlist, so it stays out of scope.
- **On-device persistence (vendor claim C5).** The current setup already
  survives power cycles through AUTO-STORE, if P2 confirms it on this unit.
- **Named preset slot.** Front-panel STORE (section 4.8 of the manual) is
  the evidenced path today. P1 makes it verifiable by software.

## 5. Attended probe plan

Run in this order. P1-P3 send no frame beyond ordinary G1 snapshots.
P4 is passive. Hardware readiness, audio safety and the bench lock are
Legalab's (`legalab-hardware-gate`); this repository does not duplicate them.

| Step | Operator action | Software | Expect / decide |
| --- | --- | --- | --- |
| P0 | none | `control snapshot` -> S0 | Record digest, decoded 73 (frame 96, carrier 100 bit 3), frame 101, name bytes (frames 103-107, 109-111). |
| P1 | Front panel: STORE -> INTERNAL, pick an **empty** slot N (the store list shows used slots), name it e.g. `LGL1021`, OK. Touch nothing else. | snapshot -> S1 | C4 confirmed if decoded 73 becomes N (or N-1), frame 101 becomes 1, the name bytes read the new name, and nothing else changes (Dump1 identical, trailer balanced). Any other change: stop and record it. |
| P2 (optional) | Make the outputs safe first (amps off, or the setup 0x15 Mute Outs check). Power-cycle the DCX. | snapshot -> S2 | C5 confirmed if S2 equals S1 byte for byte. |
| P3 (optional) | Front panel: RECALL -> INT slot N only, the slot stored in P1 from the verified state. Never another slot (R4). | snapshot -> S3 | S3 equals S1, so recall reproduces the stored image. |
| P4 (separate window, needs its own decision) | Run Behringer DCX-Remote on a Windows host over a passive RS-232 tap (RX-only taps on both directions). Refresh the preset list, recall slot N, store to a second empty slot. | Capture privately, outside git. | Publish only the function codes, payload layout and response lengths. Those three frames are what section 3 needs. |

After P4, a read-only `preset list` (directory read) can be built and probed
once under the Ping/Search pacing. Store and recall follow only after a
directory read is observed and Legalab extends the write ruling. Never probe
unknown function codes blind: one of them can be store, delete or format.

## 6. Operator decisions

1. Run P0-P1 (and optionally P2/P3) at the next attended window. Zero
   software writes, about 10 minutes, and P1 also closes the open Dump0[101]
   question.
2. Whether a Windows host and a passive RS-232 tap are available for P4.
3. Whether Legalab extends `dec-dcx-attended-software-writes-20261019` to
   preset frames once P4 has identified them.
