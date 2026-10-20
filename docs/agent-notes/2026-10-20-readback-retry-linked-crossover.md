# Post-apply readback retry and the linked O3/O4 crossover

Authority: R-HOOK-CONVERGENCE-20261004 (R-N13). Device writes are governed by
Legalab `dec-dcx-attended-software-writes-20261019` (operator 2026-10-19,
P-TIN-170 comment `131db306`, ruling 2). This lane changed source and docs
only on neo (edit-only host). It opened no serial device, sent no DCX traffic,
and ran no build or test: sting's user bus is faulted (TIN-3692 `48210445`)
and PZM LegalabCache is below the 10 GiB build floor. Gate: blocked.

## Evidence (2026-10-20 PA leg, binary built at main `9c56384e`)

- Observed: `write apply` (O1-O3 mute, O4 high-pass LR24 100 Hz) returned
  `rollback_required` / `post_apply_state_uncertain` with `readback: null`;
  the same-descriptor readback failed with `UnexpectedTrailingFrame`. A
  separate snapshot seconds later was clean and read back all five values.
- Observed: containment on that readback flagged Dump0 101 `0 -> 1`
  (unexplained, untouched here), Dump1 205 `52 -> 74` and Dump1 212
  `81 -> 80`. Before the write, O3 low-pass frequency and O4 high-pass
  frequency were both code 180.
- Confirmed against `crates/dcx-core/src/layout.rs` (DuinoDCX `00b9d70`):
  O3 low-pass frequency is `split(1, 205, 212, 0, 206)`; O3 limiter threshold
  and release use carrier 212 bits 4 and 6; O4 high-pass frequency is
  `split(1, 369, 372, 4, 370)`.
- Inferred: the DCX links adjacent outputs' crossover points (linked, not
  FREE, crossover adjust mode). The reverse direction is unverified.

## What changed

- `dcx_transport::snapshot::execute_apply_readback_with_snapshot_retry`:
  at most 3 readbacks, 20 s apart, retries as fresh `PersistentSnapshotSession`
  snapshots only (the type has no write operation). `dcxctl` apply output
  gains `readback_attempts`.
- `layout::LINKED_CROSSOVERS`, `SnapshotV1::linked_predictions` and
  `project_linked_actions`, `ApplyPlanV1::linked_changes`; planner
  `linked_effects`; verify and containment `linked_changes`.
- Docs: `docs/typed-writes.md`, `docs/live-discovery.md`, `AGENTS.md`,
  `README.md`, `CHANGELOG.md`.

## Open

- Whether the ruling counts read-only retry snapshots against the apply's
  5 remote-mode writes (this lane treats them as separate G1 snapshots).
- Carriers staged before this change that write O4 high-pass frequency on a
  linked baseline no longer reconstruct; re-run `write plan`.
- Dump0 101 remains unexplained.
