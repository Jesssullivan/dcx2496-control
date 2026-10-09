# MVP routing: rollback fix, three probes, and the combined apply left on

Authority: Jess's ruling `dec-autonomous-muted-bench-20261007` (Linear TIN-5379
comment `d24dac45-715d-4f93-82be-aaad015e61bf`, reply
`40fefa34-edca-4435-acef-c3154be5795a`). The muted premise was renewed in reply
`654a3074-74d6-458a-82d2-3d6bc7b75530` ("All muted, proceed", about 23:40 EDT
2026-10-08), and no later TIN-5379 record contradicts it.
R-HOOK-CONVERGENCE-20261004 (R-N11, R-N13). This note continues
`2026-10-08-mvp-routing-bench.md`, which stopped at p1 (O5 muted) on the
rollback identity defect.

## Rollback fix (#54, merge `ef884685`)

- `execute_rollback_readback` now runs the shared `capture_search_identities`:
  - one required identity, and at most 11 attempts (the same ten paced
    empty-timeout replays as snapshot and apply);
  - partial, invalid and transport failures are still terminal.
- The rollback budget is 90 s.
- The fake-transport regression test drops the first Search. Before the fix
  it failed with the device's exact error
  (`Capture(Timeout { Search { sequence: 1 }, received: 0 })`).
- Qualified on macbook-neo with `just check`.
- Qualified on petting-zoo-mini at `a8c4be5`, 6 of 6 steps:
  - `just check` and `just bazel-check`;
  - the transport and core suites;
  - the native suites: logic_recall 15, response_binding 7,
    mutation_recovery 14, process_runner 22, child_failure_diagnostics 10;
  - the apple package and bundle checks.
  - The return code was captured explicitly, not through `PIPESTATUS`.
- Live binary: `dcxctl-live-ef884685ce96`, built by Bazel on PZM from merged
  main `ef884685`, sha256
  `be0e3a90a1bbef1b4b211fd2a4ce2e0841846fd35ca84164bc9c4f1561740f40`.

## Bench run (2026-10-09 UTC, petting-zoo-mini, DCX device 0, `/dev/cu.usbserial-FTE902IA`)

Every serial step ran under the neo `pzm-bench.lock` (`lockf`). Before each
snapshot, apply and rollback, the guards checked three things, and each guard
fails closed:

- no Home Manager or darwin switch, using a `pgrep` pattern in bracketed form
  so it cannot match its own command line;
- the tty is free (`/usr/sbin/lsof`);
- no other `dcxctl-live` is running.

Probe method:

1. The pre-snapshot must equal the reference.
2. `control apply`.
3. A fresh snapshot whose digest and independent raw byte diff must equal the
   projection.
4. `control rollback`.
5. A fresh snapshot that must equal the reference.

Snapshots were retried at most three times with a 20 s pace. A rollback whose
plan could unmute O5 or O6 is refused by the script.

| Step | Time (UTC) | Result |
| --- | --- | --- |
| (a) fresh snapshot | ended 09:23:19 | `dc35af5f...292e` = expected; raw diff vs `531ca64a` exactly Dump1[392] 0->1, [909] 78->77 |
| q1 O4 source=C (8, `0x41`, 2) | 09:24-09:31 | apply `verified`; readback `e33fa444...` exact: Dump1[365] 1->2, [909] 77->76; rollback `rolled_back`; restore = `dc35af5f` |
| q2 input sum A+B (0, `0x02`, 4) | 09:32-09:39 | apply `verified`; readback `c87ae677...` exact: Dump0[117] 0->4, [1013] 7->3; rollback `rolled_back`; restore = `dc35af5f` |
| q3 O3 source=SUM (7, `0x41`, 3) | 09:39-09:48 | pre-snapshot attempt 1 `Timeout{Dump1}`, attempt 2 ok; apply `verified`; readback `fd103276...` exact: Dump1[195] 1->3, [909] 77->75; rollback attempt 1 `state_unresolved` (inverse written, readback incomplete), attempt 2 `rolled_back`; restore = `dc35af5f` |
| (c) combined apply, left on | 09:51-09:58 | pre-snapshot attempt 1 `Timeout{Dump0}`, attempt 2 = `dc35af5f`; apply `verified`; readback `14c4347c...e6e5` = projection |

Combined apply, in the plan's fixed order: O6 mute (10, `0x03`, 1), input sum
(0, `0x02`, 4), O4 source C (8, `0x41`, 2), O3 source SUM (7, `0x41`, 3). The
raw diff of the final readback against baseline `531ca64a` is exactly:

- Dump0 117 0->4 and trailer 1013 7->3;
- Dump1 195 1->3, 365 1->2, 392 0->1, 561 0->1, and trailer 909 78->73.

Both trailers keep their modulo-128 balance. This is the first named-device
observation of the Dump0 trailer rule and of the O3/O4 source, input sum and
O6 mute addresses.

## Final decoded state (`14c4347c`, left applied)

| Field | Value | Address | Evidence |
| --- | --- | --- | --- |
| O1/O2/O3/O4 mute | 0 (unmuted) | D0 715, D0 885, D1 54, D1 223 | unchanged from baseline; O4 address admitted |
| O5 mute | 1 (muted) | D1 392 | read back (p1, 2026-10-09 03:57Z) |
| O6 mute | 1 (muted) | D1 561 | read back here |
| O3 source | SUM | D1 195 | read back here |
| O4 source | C | D1 365 | read back here |
| O1/O2/O5/O6 source | A/B/C/C | D0 857, D1 26/534/703 | transcribed decode, unchanged from baseline |
| Input sum | A+B (4) | D0 117 | read back here |
| Input C gain/mode | 0, carrier bit 0 | D0 121 / D0 124 | unchanged from baseline (not touched) |
| Mute Outs | 0 | D0 57 | unchanged from baseline (not touched) |

The rollback of the combined apply restores O3/O4 to B, the sum to off and O6
to unmuted. That plan is stored privately as
`EMERGENCY-ONLY-unmutes-O5-O6-rollback-plan.json`, and it unmutes O6. The
Legalab hardware gate's "Never" list forbids unmuting O5/O6, so the plan is
for emergency use only and was not run.

## Findings

- The rollback identity fix held on all three rollbacks. None failed at the
  identity step. The CLI output does not report how many Search attempts each
  rollback used, so these runs do not show whether a replay was exercised.
- New: the q3 rollback's post-write readback did not complete on its first
  attempt (`post_rollback_state_uncertain`). The inverse is idempotent: the
  rerun re-wrote O3=B and verified it exactly, and the independent snapshot
  equals the reference. Post-write readback still allows one empty Search
  replay, and a dump timeout is terminal. That is the next robustness gap.
- Dump timeouts recur on standalone snapshots: Dump1 at 09:41Z, Dump0 at
  09:52Z. One paced retry cleared each.

Never touched: Input C mode/gain, Auto Align, +15 V, crossover, firmware. Raw
snapshots, plans and logs stay private on PZM under
`~/.local/state/legalab/dcx2496/routing-20261009/`.
