# Offline practice-route dump inspection

`dcxctl diagnostics practice-route --snapshot snapshot.json` reads a saved,
validated `SnapshotV1` from disk. It opens no serial port and has no apply or
rollback path. Keep named-device snapshots outside Git: they contain raw device
state and identity. The output carries `evidence_class: unverified_candidate`.
It is **not** a safety gate or proof that the PA path is ready.

The candidate byte locations below come from the MIT-licensed
[DuinoDCX `Ultradrive.cpp`](https://github.com/lasselukkari/DuinoDCX/blob/00b9d70d6192e993f31ec94ff0bc20e6e2265b2c/DuinoDCX/Ultradrive.cpp),
commit `00b9d70d6192e993f31ec94ff0bc20e6e2265b2c`, accessed 2026-09-23.
Each index counts from the start of the **complete response frame**, including
its protocol header. The [public serial parameter
reference](https://www.yumpu.com/en/document/view/43829852/the-dcx2496-serial-communication-follows-the-midi-sysex-protocol-)
gives candidate value meanings: setup parameter `0x04` is Input C gain mode
(`0` line, `1` mic); output parameter `0x41` is source (`0` A, `1` B,
`2` C, `3` SUM); output parameter `0x03` is mute (`1` muted, `0` unmuted).

| Candidate field | Frame | Byte index | Address |
| --- | --- | ---: | --- |
| Input C line/mic | Dump0 | 121 | setup `0x04` |
| O4 input source | Dump1 | 365 | output channel 8, `0x41` |
| O1 mute | Dump0 | 715 | output channel 5, `0x03` |
| O2 mute | Dump0 | 885 | output channel 6, `0x03` |
| O3 mute | Dump1 | 54 | output channel 7, `0x03` |
| O4 mute | Dump1 | 223 | output channel 8, `0x03` |
| O5 mute | Dump1 | 392 | output channel 9, `0x03` |
| O6 mute | Dump1 | 561 | output channel 10, `0x03` |

The decoder rejects values outside each listed domain. `SnapshotV1::from_json`
first checks exact frame lengths, framing, identity, and digests. No paired,
sanitized, named-device snapshots with independent front-panel observations
currently qualify the byte meanings or polarity. The public sources do not
identify separate dump fields for Auto Align or the +15 V supply, so the report
leaves both `unverified_not_decoded`. Input C reading `line` is not an independent
phantom-power readback.

Before connecting an LS56 line output to Input C, inspect the DCX front panel
with downstream equipment muted. Read Input C line mode, Auto Align and +15 V,
O4 source, and O1–O6 mutes from the device itself. A future named-device
qualification can pair each displayed state with a complete snapshot and
repeat after one bounded, reversible change; only then consider promoting a
specific decoder field into a safety gate.
