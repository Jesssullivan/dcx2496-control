# MVP routing bench: five single probes and the combined apply

Authority: Jess's ruling `dec-autonomous-muted-bench-20261007` (Linear TIN-5379
comment `d24dac45-715d-4f93-82be-aaad015e61bf`). The muted premise was renewed
for these probes and the combined apply in reply
`654a3074-74d6-458a-82d2-3d6bc7b75530` ("All muted, proceed", about 23:40 EDT
2026-10-08: "No speakers or amps are energized."). That reply holds until any
speaker or amp is energized. R-HOOK-CONVERGENCE-20261004 (R-N11, R-N13). The
code and offline projection are in `2026-10-08-mvp-routing-allowlist.md`.

## Setup

- Host: petting-zoo-mini, DCX2496 device 0 on `/dev/cu.usbserial-FTE902IA`.
- Binary: `dcxctl-live-a96dc5f577dd` (sha256 `22adbb96...`, built from
  `a96dc5f`).
- Every serial step ran under the neo-side `pzm-bench.lock` (`lockf`). Before
  each probe the runner checked for a running `darwin-rebuild`/`home-manager`
  (pgrep, which fails closed if it cannot run) and for any holder of the tty
  (`/usr/sbin/lsof`, which also fails closed if it cannot run).
- Step0 snapshot `step0-20261009T033820Z.json`: digest
  `sha256/531ca64a2785e749f29e73d1e4949c47c35426dc18ea5508440daec46aec2352`.
  It equals the stored baseline byte for byte, so the plans were staged from it
  without a replan.

## Method (per probe)

1. Pre-snapshot. It must equal the baseline digest and identity.
2. `control apply` with the single-field plan from `control diff`.
3. A fresh `control snapshot`. Its digest must equal the projected digest, and
   an independent raw byte diff against the baseline must equal the projected
   byte list exactly.
4. `control rollback` with the inverse plan, then a fresh snapshot. That
   snapshot must equal the baseline digest with an empty raw diff.

Any mismatch triggers an immediate rollback and stops the run. The combined
MVP apply uses steps 1 to 3 and is left applied.

## Result: p1 applied and verified; rollback blocked by a tool defect; run stopped

The run stopped at probe 1 (O5 mute). Probes 2 to 5 and the combined apply
did not run. The device is left at the p1 state: O5 muted, everything else at
baseline.

| Time (UTC, 2026-10-09) | Step | Outcome |
| --- | --- | --- |
| 03:53:34 | guards, pre-snapshot | baseline `531ca64a...2352`, raw diff empty |
| 03:55:17-03:57:02 | apply (9, `0x03`, 1) | rc 0, `status: verified` |
| 03:58:40 | post-apply snapshot | `dc35af5f...292e` = projection; raw diff exactly Dump1[392] 0->1, [909] 78->77 |
| 03:58:40 | rollback | rc 1 within 1 s: `Capture(Timeout { Search { sequence: 1 }, received: 0 })` |
| 03:58:40-04:00:20 | snapshot | failed: `Timeout { Dump0, received: 0 }`, no document |
| 04:03:13 | recovery guards | no HM switch; tty free (`/usr/sbin/lsof` rc 1); FTDI present in `ioreg` |
| 04:05:07 | recovery snapshot 1 | rc 0, `dc35af5f`, identity equal, same two-byte raw diff |
| 04:05:18 | rollback (after a 10 s gap) | rc 1, the same Search{1} timeout, about 1 s after opening |
| 04:07:06 | snapshot | rc 0, `dc35af5f`, identity equal, same two-byte raw diff |

Observed on the named device: O5 mute is Dump1 byte 392 (0/1, 1 = muted), and
the Dump1 trailer (909) keeps its modulo-128 balance (78 -> 77). The decoded
state (`routing inspect`) is O5 muted, with O4/O6 unmuted, input sum off, and O4
and O3 sourced from B. Input C setup bytes Dump0[121]/[124] are unchanged
(0/0).

### Cause

`execute_rollback_readback` (`crates/dcx-transport/src/snapshot.rs`, the
identity step from `0e22896d`, 2026-08-29) does exactly one `Search{1}`
exchange through `exchange_validated`, with no replay. Snapshot and apply go
through `capture_body`, which allows up to ten paced empty-timeout Search
replays. `6485dd1` (2026-09-01) added replay only for post-mutation readback.
This DCX does not answer the first Search of a fresh session reliably, so
rollback fails its identity step before `begin_rollback` and writes nothing.
The 10 s gap at 04:05 rules out port-close timing. The earlier O4 rollbacks
(2026-10-08 A4 note) presumably passed because the first Search happened to be
answered.

### Open decisions (not taken by this lane)

- **Rollback fix:** replay the rollback identity Search under the same paced
  attempt limit as `capture_body`, with a fixture for the empty first Search.
  This needs a source PR, a rebuilt live binary on PZM, and requalification.
- **Conflict:** the p1/p2 rollbacks restore O5/O6 to unmuted, but the Legalab
  hardware gate's "Never" list forbids unmuting DCX O5/O6, and the ruling does
  not waive that. The O5/O6 single probes may need to stay muted (no rollback).
  The MVP final state mutes both anyway.
- **Alternative:** diff `--mvp` against the live `dc35af5f` snapshot and apply
  the combined MVP directly. The apply path is proven on this device, but its
  rollback would hit the same defect.

Private artifacts (raw snapshots, plans, logs) are on PZM under
`~/.local/state/legalab/dcx2496/routing-20261007/` (`p1-o5-mute/`,
`recovery-p1/`).
