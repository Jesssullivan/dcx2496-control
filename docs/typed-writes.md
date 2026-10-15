# Typed parameter writes (`dcxctl write`)

`dcxctl write` turns a list of typed changes into one reviewed
direct-parameter frame, bound to an exact baseline snapshot, with an exact
inverse. It covers per-output PEQ, crossover, output gain, mute, polarity,
delay and limiter on O1-O4, the input A-C gains and, behind two explicit
flags, the O3/O4 source. Planning, `show` and `verify` are offline. `write
apply` and `write rollback` exist only in the macOS `live-control` build, and
they run the same transaction as `control apply` and `control rollback`.

Evidence class: every address this lane added is a transcription from the
pinned MIT DuinoDCX `00b9d70` tables, and its value encoding comes from the
public UltradrivePi protocol notes (both cited in `NOTICE`). None of them has
been read back from the named device yet. `write verify` is the check a
readback must pass before an address can be called observed.

## Flow

```text
control snapshot -> write show -> write plan (dry run, exact bytes)
  -> write apply (fresh baseline, stale refusal, 1 frame, full readback)
  -> write verify (offline) -> write rollback (exact touched inverse)
```

```sh
dcxctl write show --snapshot snapshot.json
dcxctl write plan --snapshot snapshot.json --plan changes.json > tx.json
jq -e '.apply_plan' tx.json > apply-plan.json
jq -e '.rollback_plan' tx.json > rollback-plan.json
dcxctl write apply --tty /dev/cu.usbserial-EXACT --expected-device 0 --plan apply-plan.json > applied.json
jq -e '.readback' applied.json > readback.json
dcxctl write verify --plan apply-plan.json --readback readback.json
dcxctl write rollback --tty /dev/cu.usbserial-EXACT --expected-device 0 --plan rollback-plan.json
```

`write plan` is always a dry run. Its `dcx.write-transaction/v1` output lists
every changed action in apply order, with the baseline and written codes and
their decoded values, the exact four wire bytes, and the Dump0/Dump1 offsets
the value has to appear at. It also gives the complete apply and rollback
frames (`apply_frame_hex` and `rollback_frame_hex`), the wire budget, and the
two carriers. Changes whose values already equal the baseline are listed in
`unchanged_changes` and send nothing.

## Plan document (`dcx.write-plan/v1`)

```json
{
  "schema": "dcx.write-plan/v1",
  "device_id": 0,
  "allow_routing": false,
  "allow_unmute": false,
  "changes": [
    {"kind": "peq_band", "output": 1, "band": 1, "filter": "bell",
     "frequency_hz": 63, "gain_db": -3, "q": 4},
    {"kind": "peq_band_count", "output": 1, "count": 1},
    {"kind": "peq_enabled", "output": 1, "enabled": true},
    {"kind": "crossover", "output": 4, "side": "high_pass", "filter": "lr24", "frequency_hz": 100},
    {"kind": "output_gain", "output": 3, "gain_db": -6},
    {"kind": "output_mute", "output": 3, "muted": true},
    {"kind": "polarity", "output": 3, "inverted": false},
    {"kind": "delay", "output": 1, "enabled": true, "short_mm": 1716},
    {"kind": "limiter", "output": 1, "enabled": true, "threshold_db": -12, "release_ms": 100},
    {"kind": "input_gain", "input": "c", "gain_db": -3},
    {"kind": "output_source", "output": 3, "source": "sum"}
  ]
}
```

Unknown fields and unknown kinds are refused. A plan holds 1 to 32 changes.
Values are never clamped: a value outside its range, or a distance off its
step grid, is a refusal.

## Addresses and encodings

Each output channel is `4 + output` (O1 is channel 5). Input A, B and C are
channels 1, 2 and 3. The wire value is 14 bits, sent as `value >> 7` and then
`value & 0x7f`.

| Field | Param | Code domain | Encoding (hypothesis) |
| --- | --- | --- | --- |
| Output gain | `0x02` | 0-300 | `code / 10 - 15` dB |
| Mute | `0x03` | 0-1 | 1 = muted |
| Delay on/off | `0x04` | 0-1 | |
| Long delay | `0x05` | 0-4000 | `code * 50` mm (0-200 m) |
| PEQ on/off | `0x06` | 0-1 | |
| PEQ band count | `0x07` | 0-9 | bands 1..count active |
| Band n frequency | `0x13 + 5(n-1)` | 0-320 | `20 * 2^(code/32)` Hz |
| Band n Q | `+1` | 0-40 | `0.1 * 10^(code/20)` |
| Band n gain | `+2` | 0-300 | `code / 10 - 15` dB |
| Band n kind | `+3` | 0-2 | 0 low shelf, 1 bell, 2 high shelf |
| Band n shelf slope | `+4` | 0-1 | 0 = 6 dB/oct, 1 = 12 dB/oct |
| High-pass type | `0x42` | 0-10 | see below |
| High-pass frequency | `0x43` | 0-320 | as band frequency |
| Low-pass type | `0x44` | 0-10 | see below |
| Low-pass frequency | `0x45` | 0-320 | as band frequency |
| Limiter on/off | `0x46` | 0-1 | |
| Limiter threshold | `0x47` | 0-240 | `code / 10 - 24` dB |
| Limiter release | `0x48` | 0-251 | 20-4000 ms; assumed logarithmic, `20 * 200^(code/251)` |
| Polarity | `0x49` | 0-1 | 1 = inverted |
| Short delay | `0x4b` | 0-2000 | `code * 2` mm (0-4 m) |
| Input A-C gain | `0x02` (ch 1-3) | 0-300 | `code / 10 - 15` dB |

Crossover type codes: 0 `off`, 1 `bw6`, 2 `bw12`, 3 `bes12`, 4 `lr12`,
5 `bw18`, 6 `bw24`, 7 `bes24`, 8 `lr24`, 9 `bw48`, 10 `lr48`. The public notes
list `0x44`/`0x45` under the same "hp" label as `0x42`/`0x43`. This lane reads
them as the low-pass pair, because the output has one high-pass and one
low-pass section; a readback has to confirm that.

The limiter release grid is the least certain encoding. `write show` labels
its decoded value `release_ms_log_hypothesis`.

Every location lives in the 6 x 74 `outputLocations` table and in row 0 of
`inputLocations[4][62]`. A unit test checks the whole table against the
pinned source, and other tests check that every field's device maximum fits
its location and the 14-bit wire range.

Golden vectors in `crates/dcx-core/src/typed_write/tests.rs`:

- The UltradrivePi example "set in A gain of device #1 to +6dB",
  `F0 00 20 32 00 0E 20 01 01 02 01 52 F7`, is reproduced byte for byte.
  The default policy refuses it, because +6 dB is above the 0 dB ceiling.
- D1, O1 HPF LR24 at 80 Hz: `05 43 00 40`, `05 42 00 08`.
- D4, O4 HPF LR24 at 100 Hz (99.4 Hz): `08 43 00 4A`, `08 42 00 08`.
- O1 band 1 bell at 63 Hz, -6 dB, Q 4: `05 13 00 35`, `05 14 00 20`,
  `05 15 00 5A`.

## Safety design

- **Dry run by default.** `write plan` opens no device and prints the exact
  frame bytes. Only the two carriers it emits can be applied.
- **Plan = typed changes.** Each change encodes to fixed addresses. A plan
  can't name a channel, parameter, raw value or frame.
- **Fresh snapshot before, complete readback after.** `write apply` runs the
  `control apply` transaction. It captures a complete baseline on the same
  descriptor and refuses the plan if that baseline differs from the bound one
  in any byte. It then writes exactly one direct-parameter frame and captures
  a complete readback. The result is `verified` only when the readback equals
  the desired snapshot exactly.
- **Rollback = the touched inverse.** The rollback carrier re-applies the
  baseline values of exactly the addresses the apply wrote, in reverse order.
  It is never a stored-preset recall (risk R4: the DCX stored rollback unmutes
  O6). `write rollback` re-checks that the carrier never addresses O5 or O6,
  so it cannot unmute either.
- **Hard refusals (policy).** Every plan is refused while O5 or O6 reads
  unmuted in the baseline. Other refusals:
  - any change that targets O5 or O6;
  - unmuting O1-O4 unless both the plan's `allow_unmute` and
    `--allow-unmute` are set;
  - an O3/O4 source change unless both `allow_routing` and `--allow-routing`
    are set (the O1/O2 sources, the input sum and the input routing have no
    typed address);
  - an unmute and a source change in the same plan;
  - a level gain or limiter threshold rise of more than `--max-gain-step-db`
    (default and maximum 3 dB);
  - an output or input gain above `--gain-ceiling-db` (default and maximum
    0 dB);
  - switching an enabled limiter off;
  - more than `--max-actions` actions (default 16, maximum 32).
- **EQ is cut-only.** A PEQ gain above 0 dB is refused, and there is no
  boost flag. The core carrier has refused PEQ boosts under every policy
  since the notch work, and it still refuses any step that would activate a
  band holding a stored boost. Allowing boosts would be a contract change,
  not a flag.
- **Core carrier guards, for every lane.** `ApplyPlanV1` now also refuses a
  level gain rise above unity or of more than +3 dB, a limiter-threshold rise
  of more than +3 dB, and a limiter switch-off. So a hand-built carrier fed to
  `control apply` can't get past the policy either. Rollback is exempt
  because it restores a verified baseline.
- **Safe ordering inside the frame.** The device works through one frame in
  order, so the frame is sorted so that every intermediate state is the safer
  one:
  1. mutes on;
  2. level and threshold cuts, limiter on and release;
  3. PEQ deactivation (off, then fewer bands);
  4. crossover, delay, polarity and band fields. A band's gain goes first
     when the cut gets shallower and last when it gets deeper;
  5. PEQ activation (more bands, then on);
  6. level rises;
  7. sources;
  8. unmutes.

  Rollback is the exact reverse.

## Wire budget and proposed rate limit

Each live transaction sends exactly one direct-parameter (`0x20`) frame:
9 + 4 x actions bytes, so 25 bytes for a 4-action band write, about 7 ms at
38400 baud. Remote-mode writes per successful transaction:

| Transaction | Remote-mode writes | Direct frames |
| --- | --- | --- |
| `write apply` | 5: `3F 08` and `3F 04` around the baseline dumps, `3F 0C` before the frame, `3F 08` and `3F 04` around the readback dumps | 1 |
| `write rollback` | 3: `3F 0C` before the frame, then `3F 08` and `3F 04` around the readback dumps | 1 |

A failure after a transmit-capable mode adds at most one `ReceiveDirect`
quiescence write at finish. G1's limit of at most 2 remote-mode writes per
attempt covers snapshot attempts, so apply and rollback need their own gate.

Proposed rate limit, for Legalab to adopt or change:

- one frame per transaction, with no automatic retry of a frame;
- at most 4 actions in the first live write, then the default of 16;
- at least 20 s between live transactions, the same pacing as G1;
- at most 3 apply transactions, each with its rollback, per hardware window
  before the operator checks in.

## Offline consistency check

`write verify --plan apply-plan.json --readback readback.json` exits non-zero
unless all three of these hold:

- every written code decodes from the readback at its transcribed location;
- no bit changed outside the plan's projected bits and the touched dump
  trailer;
- the readback equals the desired snapshot.

A unit test runs this over all 55 typed addresses (13 on each of O1-O4, plus
inputs A-C). Each value projected into a snapshot decodes from the named frame
offsets and passes; the untouched baseline fails. Run `write verify` on each
new address's first named-device readback. A pass is the evidence that
promotes an address from transcribed to observed.
