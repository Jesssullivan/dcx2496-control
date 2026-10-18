# Typed parameter writes (`dcxctl write`) and the first live write

Authority: R-HOOK-CONVERGENCE-20261004 (R-N13). The operator cleared full-loop
work for 2026-10-15; OD-3 covers merge on green. Receipts go on Linear
TIN-5383 and TIN-5379. This lane changed source only. It opened no serial
device and sent no DCX traffic of any kind, no audio and no driver action.

**Status: offline planning only, pending an operator ruling.** Landing this
code authorizes no software write to the device. Legalab ruling D5 (option A,
front-panel entry, one parameter per step, snapshot before and after) stands
for every DCX write until the operator rules on decisions 1 and 2 below.
`write apply|rollback` build only with the non-default `typed-write-live`
feature, which `//:dcxctl_live_control` and `.#dcxctl-live` do not enable.

## What landed

- `dcx_core::typed_write` adds:
  - a strict `dcx.write-plan/v1` of typed changes;
  - encoders with golden byte vectors;
  - the write policy;
  - safe in-frame ordering;
  - `plan_writes`, which emits a dry-run `dcx.write-transaction/v1`: exact
    frame hex, per-action wire bytes and dump offsets, apply and rollback
    carriers, and a wire budget;
  - `check_policy` and `check_rollback`, which re-validate carriers;
  - `verify_written_values` for offline readback consistency;
  - `decode_typed_state` for `write show`.
- `layout::reviewed_address` adds:
  - the O1-O3 mutes;
  - on O1-O4 only: gain, delay on/long/short, HPF/LPF type and frequency,
    limiter on/threshold/release, polarity;
  - the input A-C gains.

  All are from the pinned DuinoDCX tables. A test checks the packing rule
  over all 444 rows; an offline cross-check against the pinned
  `Ultradrive.cpp` (PR #58 adversarial review, 2026-10-17) matched every row,
  and the input A-C gain rows, with 0 mismatches.
- Core `ApplyPlanV1` now refuses, for every lane:
  - a level gain rise above unity, or of more than +3 dB;
  - a limiter threshold rise of more than +3 dB;
  - a limiter switch-off;
  - any typed-write-lane action while O5 or O6 reads unmuted;
  - any apply or rollback action that writes an O5/O6 mute other than 1
    (`ReservedUnmute`), so no lane can unmute O5 or O6, and a plan that would
    mute a live O5/O6 cannot be staged (its exact rollback would unmute).

  `control apply|rollback` refuse typed-write-lane carriers (the typed fields
  and the O1-O3 mutes), so those can go only through `write
  apply|rollback`. PEQ-only and O4-mute-only carriers on O1-O4 still pass
  `control apply|rollback` and the Apple helper under the core rules only, as
  before this lane.
- Review fixes made before the PR:
  - band gain order (deepening first, shallower last);
  - sources in routing rank;
  - a summed rise budget that includes effective PEQ band gains;
  - consent for loosening an active high-pass;
  - the ceiling binds rises only;
  - wire bytes come from the frame encoder;
  - the `control apply` bypass is closed.
- `dcxctl write show|plan|verify` is offline. `write apply|rollback` exists
  only in a `typed-write-live` build, which nothing ships, and wraps `control
  apply|rollback`: apply rebuilds the carrier from `--write-plan` under the
  same flags and requires an exact match; rollback requires the exact reverse
  order; both exit non-zero unless the readback verifies.
- Adversarial review fixes (review of `b543b1a`, 2026-10-17; reviewer PoCs
  turned into regression tests in `typed_write/tests.rs` and
  `routing/tests.rs`):
  - R4 closed in the core carrier: `ReservedUnmute` on every apply and
    rollback carrier; `routing desired-profile --o5-mute|--o6-mute` accept
    `on` only; the routing and write pipelines use the new
    `SYNTHETIC-reserve-muted-snapshot.json` (the blank fixture with only
    O5/O6 muted, derived with the `b543b1a` binary);
  - live typed writes moved behind `typed-write-live`;
  - in-frame order enforced on carriers (`UnsafeOrder`) and exact-reverse
    rollback (`RollbackOrder`);
  - moving or reshaping an active PEQ cut spends its released depth from the
    rise budget;
  - `write apply|rollback` exit codes; scope of the `control apply` refusal
    and of the 444-row test reworded.
- Docs: `docs/typed-writes.md`, a README section, and AGENTS.md (the allowlist
  contract).

## Validation

- **sting:** `just check` passes on Linux x86_64 under the pinned Nix shell:
  fmt, clippy pedantic, every workspace test, the fixture pipelines including
  the new `tests/write_pipeline.sh`, lockfiles and front-door.
- **macOS live-control build:** cross-clippy for `aarch64-apple-darwin` with
  `--features live-control`, on sting with the pinned 1.98.0 toolchain plus
  the darwin std, `-D warnings`.
- **Not run at `b543b1a`:** a Bazel or native macOS build on PZM, and
  anything on the named device.
- **Review fixes:** the six reviewer PoCs passed against `b543b1a` on PZM
  (finding reproduced), and the inverted regression tests plus the PZM native
  gate run on the landing head. The head sha, host and targets are in the
  TIN-5383 merge receipt. Nothing ran on the named device.

## Offline decode of the existing 2026-10-09 readback (no device contact)

`14c4347c` (`routing-20261009/final-mvp/post-apply.json` on PZM, private) was
decoded with a read-only script against the same pinned tables. Every typed
field lies inside its documented domain. The values are plausible; they are
**not** observations of the new addresses:

- Untouched PEQ bands sit at code 180 / Q 20 / gain 150 / bell / slope 1,
  that is 987 Hz, Q 1, 0 dB. That looks like a factory default.
- **O1:** PEQ on, band count 0. Band 1 holds an inactive low shelf at code 58
  (70 Hz), -5.5 dB.
- **O4 (PA):** high-pass type code 6 (BW24 under the hypothesis) at code 180
  (about 1 kHz). O6 has the same.
  - If the front panel confirms it, the PA runs a 1 kHz 24 dB/oct high-pass
    today, and D4's LR24 at 100 Hz is a large change. It loosens an active
    high-pass, so the plan needs `allow_protection_change` and
    `--allow-protection-change`.
  - If the front panel disagrees, the crossover mapping is wrong.
  - Either way, a front-panel **read** of the O4 crossover page is the
    cheapest independent check of the `0x42`/`0x43` mapping. Look only; don't
    edit.
- **Gains:** O3 and O5 at code 130 (-2 dB). Every other output and input is at
  0 dB.
- **Limiters:** off. Threshold 0 dB on every output except O1 (-0.5 dB).

## Bench procedure: first live write (main session, hardware window)

**BLOCKED.** This needs an operator ruling that supersedes D5 for software
writes plus a remote-mode write budget for apply (5) and rollback (3). G1
covers snapshot remote-mode writes only, and the qualification packet section
3.3 preconditions are those of the read-only meter run; neither authorizes a
direct-parameter frame. The procedure is kept as the proposal for that
ruling. It also needs a `typed-write-live` build, which no shipped binary is.

Gate (after the ruling): legalab qualification packet section 3.3
preconditions plus the ruling's write budget.

- Hold neo's bench lock and use the quiet-step wrapper.
- The tty is free, no other `dcxctl-live` is running, and the DCX Logic helper
  is closed.
- The Pro 24's power state is held.
- Record the JBL/Alto power state and the LS56 Monitor knob position.
- Play no programme material during steps 1-6.

Binary (after the ruling): a `typed-write-live` build of `dcxctl` on PZM
from merged main, under the PZM build lock, with its sha256 recorded
(`$BIN`). `//:dcxctl_live_control` does not carry `write apply|rollback`.
`$OUT=~/.local/state/legalab/dcx2496/typed-write-<UTC>`, private.

1. **Fresh snapshot S0** (G1: 2 remote-mode writes per attempt, at most 3
   attempts, 1 recovery):
   `"$BIN" control snapshot --tty "$TTY" --expected-device 0 > "$OUT/S0.json"`.
   Re-baselined on the 2026-10-18 S0 `be965a63` (TIN-5383 B0 receipt
   `1b1c4d0b`), not `14c4347c`: diff the fresh S0 against `be965a63`; any
   byte change needs explanation before continuing. Against `14c4347c`,
   `be965a63` changed Dump0[119] 1 -> 0 (A/B to analog, as predicted), the
   trailer [1013] 3 -> 5, and Dump0[101] 1 -> 0, which is in no DuinoDCX
   table and is unexplained. Per packet section 4.3 nothing is applied until
   Dump0[101] is explained.
2. **Offline decode:**
   `"$BIN" write show --snapshot "$OUT/S0.json" > "$OUT/S0-typed.json"`.
   - O5 and O6 must read `muted: true`.
   - Record O1 `eq_enabled`, `eq_count` and band 1.
   - Optional zero-write cross-check: the operator reads the O4 and O1
     crossover pages on the front panel (look, don't edit) and compares them
     with `high_pass.*` and `low_pass.*`.
3. **Plan the first write.** It is one PEQ band on O1, kept inaudible: band 1
   only, with no count or enable change.

   ```json
   {"schema":"dcx.write-plan/v1","device_id":0,"changes":[
     {"kind":"peq_band","output":1,"band":1,"filter":"bell",
      "frequency_hz":63,"gain_db":-3,"q":4}]}
   ```

   Run
   `"$BIN" write plan --snapshot "$OUT/S0.json" --plan p1.json --max-actions 4 > "$OUT/p1-tx.json"`.

   **Stop** unless S0 shows O1 band 1 inactive (`eq_count` 0, or PEQ off).
   If S0 equals the 10-09 state, the frame is
   `F0 00 20 32 00 0E 20 04 05 13 00 35 05 14 00 20 05 16 00 01 05 15 00 78 F7`:
   - frequency 58 -> 53 (63.1 Hz);
   - Q 20 -> 32 (3.98);
   - low shelf -> bell;
   - gain 95 -> 120 last, because the cut gets shallower.

   The rollback frame is
   `F0 00 20 32 00 0E 20 04 05 15 00 5F 05 16 00 00 05 14 00 14 05 13 00 3A F7`.
   A unit test pins both frames.
   Check that `writes` is only channel 5 parameters `0x13`-`0x16`, then
   extract the two carriers with `jq -e .apply_plan` and
   `jq -e .rollback_plan`.
4. **Apply:**
   `"$BIN" write apply --tty "$TTY" --expected-device 0 --write-plan p1.json --plan p1-apply.json --max-actions 4 > "$OUT/p1-apply.json"`.
   Wire traffic is 5 remote-mode writes and 1 direct frame. A stale-baseline
   refusal means someone touched the device: stop. Expect
   `"status": "verified"`.
5. **Readback checks**, offline:
   - `jq -e .readback "$OUT/p1-apply.json" > "$OUT/R1.json"`
   - `"$BIN" write verify --plan p1-apply.json --readback "$OUT/R1.json"`
     must exit 0 with `consistent`.
   - `"$BIN" control verify-containment --plan p1-apply.json --readback "$OUT/R1.json"`
     must report `contained`.

   This is the first named-device readback of O1 band 1 (only O1/PEQ9 had one
   before).
6. **Rollback test:**
   `"$BIN" write rollback --tty "$TTY" --expected-device 0 --plan p1-rollback.json > "$OUT/p1-rollback.json"`.
   Wire traffic is 3 remote-mode writes and 1 direct frame. Expect
   `rolled_back` with `equals_baseline: true`, which means the restored
   digest equals S0's.
7. **Optional audible step,** a separate transaction and a separate operator
   go. Take a fresh snapshot, then plan band 1 plus `peq_band_count` 1 (PEQ
   is already on) on O1. Sweep at position 1 with the same -30 dBFS ESS
   before and after; expect about -3 dB at 63 Hz on L only. Then roll back
   and sweep again.

Stop conditions:

- Apply returns `rollback_required`: roll back once, immediately.
- Rollback returns `state_unresolved`: stop. Read the front panel, run at most
  one `control recover-receive-direct`, then take a fresh snapshot.
- Any sound at O5/O6, or any change in sound during steps 1-6.
- Any O5/O6 field differs in a readback.
- An operator stop.

Record on TIN-5383: binary sha256, S0, R1 and restored digests, both frame
hex strings, the `write verify` and containment outputs, attempt counts, and
UTC times.

## Operator decisions this lane needs

1. **Software writes and D5.** Do typed DCX writes (`dcxctl write`) supersede
   D5 (front-panel-only entry) for the parameters they cover (PEQ, crossover,
   output gain, O1-O4 mute, polarity, delay, limiter, input A-C gain)? The
   proposal is yes, address by address: an address is usable for tuning once
   its first named-device readback passes `write verify` and containment.
   Until then, and for any address that fails, front-panel entry with a
   snapshot before and after stays the method.
2. **Remote-mode budget for writes.** G1 covers snapshots only (2 per
   attempt). An apply transaction sends 5 remote-mode writes and a rollback
   sends 3, each plus one direct frame and at most one failure-path quiesce.
   Proposal:
   - one attempt each, with no automatic retry;
   - at least 20 s between transactions;
   - at most 3 apply/rollback pairs per window.
3. **Settled in code, in line with the operator's "O5/O6 muted forever":**
   "never unmute O5/O6" is now a core carrier rule for every lane
   (`ReservedUnmute`). The 10-09 routing run's
   `EMERGENCY-ONLY-unmutes-O5-O6-rollback-plan.json` carries O5/O6 unmute
   actions, so `RollbackPlanV1::from_json`, and with it `control rollback`
   and the Apple helper, refuses it. If the operator ever wants a reserved
   output unmuted, that is a new ruling and a code change.
4. **EQ boosts stay unavailable.** There is no boost flag; the core carrier is
   cut-only. Changing that is a contract decision, not a flag.
