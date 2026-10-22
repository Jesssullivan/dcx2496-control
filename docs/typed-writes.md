# Typed parameter writes (`dcxctl write`)

`dcxctl write` turns a list of typed changes into one reviewed
direct-parameter frame, bound to an exact baseline snapshot, with an exact
inverse. It covers per-output PEQ, crossover, output gain, mute, polarity,
delay and limiter on O1-O4, the input A-C gains and, behind two explicit
flags, the O3/O4 source. Planning, `show` and `verify` are offline.

**Attended software writes only.** Legalab ruling
`dec-dcx-attended-software-writes-20261019` (operator, 2026-10-19, P-TIN-170
comment `131db306`, ruling 2) allows attended software writes of typed
fields: an attended `write apply` may send at most 5 remote-mode
writes and a `write rollback` at most 3, each transaction after a fresh G1
snapshot, with the exact touched-address rollback, and O5/O6 are never
unmuted. Legalab owns the attendance and readiness ceremony; this repository
does not duplicate it. `write apply` and `write rollback` are compiled only
with the non-default macOS `typed-write-live` feature, which unattended
builds never enable: `//:dcxctl_live_control` and `.#dcxctl-live` build
`live-control` alone. Once built, they run the same transaction as `control
apply` and `control rollback`. The first named-device use was the
2026-10-20 PA leg (O1-O3 muted, O4 high-pass LR24 100 Hz; O4 already
unmuted, so its unmute sent nothing).

Evidence class: every address this lane added is a transcription from the
pinned MIT DuinoDCX `00b9d70` tables, and its value encoding comes from the
public UltradrivePi protocol notes (both cited in `NOTICE`). `write verify`
is the check a readback must pass before an address can be called observed.
Observed 2026-10-20 (PA leg): a separate snapshot after the apply read back
all five written values (O1-O3 mutes, O4 high-pass type LR24 and frequency
code 74, 99.35 Hz) at their transcribed offsets, but containment also flagged
Dump0 byte 101 (`0 -> 1`, unexplained, out of scope here) and the linked O3
low-pass move described below, so that readback did not pass `write verify`
and no address is promoted by it.

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
# typed-write-live builds only, attended use under the 2026-10-19 ruling:
dcxctl write apply --tty /dev/cu.usbserial-EXACT --expected-device 0 \
  --write-plan changes.json --plan apply-plan.json > applied.json
jq -e '.readback' applied.json > readback.json
dcxctl write verify --plan apply-plan.json --readback readback.json
dcxctl write rollback --tty /dev/cu.usbserial-EXACT --expected-device 0 --plan rollback-plan.json
```

`write apply` exits non-zero unless the readback verified, and `write
rollback` exits non-zero unless the restored readback equals the baseline,
so automation never continues past a failed or mismatched write. (`control
apply|rollback` keep their historical exit 0 after printing a
`rollback_required` or `state_unresolved` status with a captured readback.)

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
  "allow_protection_change": false,
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
| PEQ band count | `0x07` | 0-8 | bands 1..count active; 9 is refused (device refused it, 2026-10-21) |
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
`inputLocations[4][62]`. A unit test checks the 7-of-8 packing rule over all
444 output rows plus a few hand-reviewed offsets; it does not compare against
the source. An offline cross-check against the pinned `Ultradrive.cpp`
(PR #58 adversarial review, 2026-10-17) matched every row, and the input
A-C gain rows, with 0 mismatches. Other tests check that every field's device
maximum fits its location and the 14-bit wire range.

Golden vectors in `crates/dcx-core/src/typed_write/tests.rs`:

- The UltradrivePi example "set in A gain of device #1 to +6dB",
  `F0 00 20 32 00 0E 20 01 01 02 01 52 F7`, is reproduced byte for byte.
  The default policy refuses it, because +6 dB is above the 0 dB ceiling.
- D1, O1 HPF LR24 at 80 Hz: `05 43 00 40`, `05 42 00 08`.
- D4, O4 HPF LR24 at 100 Hz (99.4 Hz): `08 43 00 4A`, `08 42 00 08`.
- O1 band 1 bell at 63 Hz, -6 dB, Q 4, from a flat band: `05 15 00 5A`
  (the deepening gain goes first), `05 13 00 35`, `05 14 00 20`.

## Safety design

- **Dry run by default.** `write plan` opens no device and prints the exact
  frame bytes. Only the two carriers it emits can be applied: `write apply`
  rebuilds the carrier from the same `--write-plan` document, the carrier's
  own baseline and the same policy flags, and refuses unless the rebuilt
  carrier is identical.
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
  so it cannot unmute either, and that its order is the exact reverse of the
  apply. A rollback carries no policy flags: it restores the baseline's mutes
  (including unmuting an O1-O4 output that was live in the baseline), levels
  and high-pass without the consents an apply needs, and it writes the
  baseline values without first checking that the live state is the apply's
  desired state or one of its prefixes.
- **Hard refusals (policy).** Every plan is refused while O5 or O6 reads
  unmuted in the baseline. Other refusals:
  - any change that targets O5 or O6;
  - unmuting O1-O4 unless both the plan's `allow_unmute` and
    `--allow-unmute` are set;
  - an O3/O4 source change unless both `allow_routing` and `--allow-routing`
    are set (the O1/O2 sources, the input sum and the input routing have no
    typed address);
  - an unmute and a source change in the same plan;
  - loosening an active high-pass unless both `allow_protection_change` and
    `--allow-protection-change` are set. Loosening means changing its type,
    switching it off, or lowering its corner;
  - a summed level rise of more than `--max-gain-step-db` (default and
    maximum 3 dB). The sum covers every output and input gain rise, every
    limiter threshold rise, and every rise in a PEQ band's effective gain
    (its gain while active, 0 dB while inactive). Moving or reshaping an
    active band (its frequency, Q, kind or slope) counts as releasing its
    whole baseline cut at the old position, then a new cut, so moving a
    -15 dB cut off a room mode spends 15 dB. Shallower cuts, moved cuts,
    switching PEQ off over a cut, and raising two gains on one path
    therefore share one budget;
  - a rising output or input gain above `--gain-ceiling-db` (default and
    maximum 0 dB). A cut from above the ceiling is admitted;
  - switching an enabled limiter off;
  - more than `--max-actions` actions (default 16, maximum 32);
  - a carrier whose actions are not in the safe in-frame order below.

  Consent is checked twice. At plan time it needs the plan field and the
  flag. At apply time `write apply` re-plans from the `--write-plan`
  document with the flags given to it, so both consents are needed again;
  the carrier itself does not store the plan document.
- **EQ is cut-only.** A PEQ gain above 0 dB is refused, and there is no
  boost flag. The core carrier has refused PEQ boosts under every policy
  since the notch work, and it still refuses any step that would activate a
  band holding a stored boost. Allowing boosts would be a contract change,
  not a flag.
- **Core carrier guards, for every lane.** No apply or rollback carrier on
  any lane may write an O5/O6 mute value other than 1 (`ReservedUnmute`), so
  `control apply|rollback`, `write apply|rollback` and the Apple helper can
  never unmute O5 or O6, and the stored 10-09
  `EMERGENCY-ONLY-unmutes-O5-O6-rollback-plan.json` no longer parses. A plan
  that would mute a live O5/O6 cannot be staged either, because its exact
  rollback would unmute it; O5 and O6 read muted on the bench since
  2026-10-09, so this costs nothing operationally. `dcxctl routing
  desired-profile` accepts `--o5-mute on` and `--o6-mute on` only.
  `ApplyPlanV1` also refuses a level gain rise above unity or of more than
  +3 dB, a limiter-threshold rise of more than +3 dB, a limiter switch-off,
  and any typed-write-lane action (O1-O3 mute or a typed field) while O5 or
  O6 reads unmuted. Rollback is exempt from the level rules because it
  restores a verified baseline. `control apply` and `control rollback` refuse
  any carrier that holds a typed-write-lane address (the typed fields and the
  O1-O3 mutes), so the write policy cannot be skipped for those addresses by
  switching commands. That refusal does not cover PEQ or the O4 mute: a
  carrier that touches only PEQ on O1-O4, or only the O4 mute, still runs
  through `control apply` and the Apple helper under the core rules only
  (cut-only, no boost exposure, the reserved-mute rule) with no write policy,
  as those lanes did before typed writes existed.
- **Safe ordering inside the frame.** The device works through one frame in
  order, so the frame is sorted so that every intermediate state is the safer
  one:
  1. mutes on;
  2. level and threshold cuts, limiter on and release;
  3. PEQ deactivation (off, then fewer bands);
  4. crossover, delay, polarity and band fields. A band's gain goes first
     when its cut deepens and last when it gets shallower, so a moving band
     is never louder than either end state. A filter being switched off goes
     off before its corner moves;
  5. PEQ activation (more bands, then on);
  6. level rises;
  7. sources, O4 before O3 (the routing rank);
  8. unmutes.

  Rollback is the exact reverse. The order is enforced on carriers too, not
  only produced by `write plan`: the policy check refuses an apply carrier
  whose actions step back to an earlier phase (`UnsafeOrder`), and the
  rollback check refuses one that is not the exact reverse of its apply
  (`RollbackOrder`).

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
attempt covers snapshot attempts only; the 2026-10-19 ruling sets apply at
5 and rollback at 3, which is what one transaction sends when counted as
function-`0x3f` commands.

**Pending operator decision:** whether a read-only readback retry (below)
counts against the apply's 5 is not settled. If remote-mode writes are
`0x3f` commands, one fresh retry snapshot adds 2 (transmit-only, then
receive-only), plus at most one `ReceiveDirect` quiescence per failed
session. If the ruling counts parameter writes (its rationale cites about
45 for the 2.1 plan), a retry adds none. This repository implements the
retry as an ordinary read-only G1 snapshot session and does not claim
either reading; Legalab must confirm it before relying on the retry.

Rate limit proposed before that ruling, for Legalab to keep or change:

- one frame per transaction, with no automatic retry of a frame;
- at most 4 actions in the first live write, then the default of 16;
- at least 20 s between live transactions, the same pacing as G1;
- at most 3 apply transactions, each with its rollback, per hardware window
  before the operator checks in.

## Post-apply readback retry

On 2026-10-20 the PA-leg apply wrote its frame, but the same-descriptor
readback failed with a transport error (`UnexpectedTrailingFrame`), so the
result was `rollback_required` / `post_apply_state_uncertain` with `readback:
null`. A separate snapshot seconds later was clean. That carrier failure
leaves the open session refusing every further operation until it is closed,
so a retry on the same descriptor cannot help.

`write apply` and `control apply` therefore repeat only the read-only
snapshot when the post-apply readback fails with a transport or read
failure (platform transport error, bounded timeout, response over its exact
limit, or an exhausted empty-Search window) and the failed session closed
verified, and only while the retry fits the existing apply envelope:

- the whole transaction keeps the 150 s `APPLY_TRANSACTION_BUDGET`, measured
  from the start of the apply, so the Apple helper's 155 s child timeout
  (`HelperConfiguration.swift`) still outlives it unchanged. A retry starts
  only if the fixed 20 s wait plus `POST_APPLY_RETRY_SNAPSHOT_FLOOR` (54 s:
  nine paced Search gaps, ten Search deadlines and two dump deadlines, the
  least a replay-free ten-Search snapshot needs) still fits, and the retry
  snapshot then runs under the remaining envelope, so it stops on its budget
  rather than outliving the helper;
- the ceiling is 3 readbacks in total, but with the fixed Search cadence the
  envelope usually admits one fresh snapshot: a second retry after a full
  first retry would start at 110 s and need 74 s more. A first retry that
  fails at its first Search (about 65 s in) still leaves room for a second
  (65 + 74 = 139 s). Every path stays inside 150 s. No bound is caller input;
- each retry opens a fresh session on the same exact callout and runs one
  ordinary complete G1 snapshot (ten Searches, transmit-only mode, Dump0,
  Dump1, receive-only mode, verified close). The retry session type has no
  direct-parameter operation, so no write frame and no receive-and-transmit
  mode is ever repeated;
- the first complete readback is verified exactly as before. A mismatch, a
  wrong-device or malformed readback, a remote-mode, pacing or budget
  failure, an unverified close, or a failed open is never retried;
- if every admitted readback fails, the result is exactly today's:
  `rollback_required`, `failure: post_apply_state_uncertain`, `readback:
  null`. A readback is never synthesized.

The apply output records `readback_attempts` (0 when the transaction failed
before its readback), `readback_failures` (one sanitized receipt per failed
readback: `kind`, `operation`, the carrier failure class such as
`unexpected_trailing_frame`, `session_closed_verified`, and the close
failure class; never bytes or paths), and `readback_retry_stop`
(`not_retryable`, `attempt_limit`, `apply_envelope`, `pacing_failed`,
`open_failed`, or null). `session_closed_verified: false` on the first
failure means the retry was not eligible.

## Linked O3/O4 crossover

Observed 2026-10-20 (PA leg): writing O4 high-pass frequency code 74 also
moved the O3 low-pass frequency, which had equalled the O4 high-pass code
(180) before the write, to 74: Dump1 205 `52 -> 74` (low seven bits) and
carrier 212 `81 -> 80` (bit 0 is the field's bit 7; bits 4 and 6 are the O3
limiter threshold and release and did not change). The O3 low-pass type did
not change. Inferred: the device links adjacent outputs' crossover points
(its linked, not FREE, crossover adjust mode); that mode has no reviewed
address.

The model is deliberately narrow:

- only O4 high-pass frequency (channel 8, `0x43`; Dump1 369, carrier 372
  bit 4, high 370) driving O3 low-pass frequency (channel 7, `0x45`; Dump1
  205, carrier 212 bit 0, high 206). No O1/O2 or O2/O3 pair and no crossover
  type is modelled;
- a link is predicted only when the baseline holds the follower at the
  driver's code (the only available sign the pair is linked) and the plan
  does not also write the follower. A plan writing both fields of a pair the
  baseline shows linked is refused;
- a predicted follower move on an active filter (type not off in the
  baseline, or switched on by the plan) needs `allow_protection_change` in
  the plan and `--allow-protection-change`, whichever direction it moves and
  whether the direction is observed or inferred, because the device moves it
  without a frame byte of its own;
- `write plan` lists the prediction in `linked_effects` and adds no frame
  byte. The desired snapshot, and so `write verify` and the live readback
  check, expect the follower at the written code. The rollback, which
  rewrites only the driver, expects the follower back at its baseline code;
- `control verify-containment` and `write verify` list a follower move
  under `linked_changes` (never dropped) only when the follower reads
  exactly the written driver code and the readback driver holds it too. Only
  an observed direction counts as contained (`contained: true`). Any other
  bit of those bytes (such as the limiter threshold or release high bits in
  212) or any other code stays uncontained.

The reverse direction (an O3 low-pass frequency write moving O4 high-pass
frequency) is inferred and unverified. `linked_effects` and `linked_changes`
report it as `inferred_unverified` with `contained: false`; its bits stay in
`uncontained_changes`, so both gates exit non-zero, and the desired snapshot
never expects it, so a live apply reports `rollback_required`.

The equal-codes gate is a heuristic with a false-positive mode: on a device
in FREE mode, any state where O4 high-pass and O3 low-pass share a code
(after an aligning write, or at a shared default) predicts a link on every
later O4 high-pass write. The device then moves only O4, the readback does
not equal the desired state, and the apply fails safe into its exact
rollback; a plan writing both fields is refused. The way out is to write
the O3 low-pass frequency first, in its own plan, so the codes differ.

## Offline consistency check

`write verify --plan apply-plan.json --readback readback.json` exits non-zero
unless all three of these hold:

- every written code decodes from the readback at its transcribed location;
- no bit changed outside the plan's projected bits, the touched dump
  trailer and a matched linked-crossover follower field;
- the readback equals the desired snapshot.

A unit test runs this over all 55 typed addresses (13 on each of O1-O4, plus
inputs A-C). Each value projected into a snapshot decodes from the named frame
offsets and passes; the untouched baseline fails. Run `write verify` on each
new address's first named-device readback. A pass is the evidence that
promotes an address from transcribed to observed.

## Output profile chains

`dcxctl profile plan|verify` (offline) and `profile apply` (`typed-write-live`
only) move one output to a Legalab `DcxOutputProfileV1`
(`legalab.dcx-output-profile/v1`). Legalab owns that contract, the scene that
links a profile to an output, and the ceremony around an apply; this
repository only reads the profile.

**Ruling.** Legalab `dec-scene-dcx-apply-20261021` (operator interview,
2026-10-21): the scene engine may apply DCX PEQ, gain and crossover diffs
within the 5/3 remote-mode write budget per transaction, the 3 dB max-rise
rule and no boost above 0 dB, readback-verified, with the rollback ready,
stopping on any `rollback_required`. The profile's other fields (mute,
source, polarity, delay, limiter) are verify-only: `profile plan` refuses
while one of them differs (`VerifyOnlyDrift`), so a chain never unmutes,
routes, or touches a limiter. The one admitted difference is a muted output
under an unmuted profile (`held_muted: true`): the chain tunes the output
while it is muted and leaves it muted, so the caller can apply the EQ before
its own reviewed unmute write. A live output under a muted profile is
refused.

```sh
dcxctl control snapshot --tty /dev/cu.usbserial-EXACT --expected-device 0 > S.json
dcxctl profile plan --profile dcx-o4-vocal.json --snapshot S.json > chain.json
jq -r .chain_id chain.json
# typed-write-live builds only, under the ruling above:
dcxctl profile apply --tty /dev/cu.usbserial-EXACT --expected-device 0 \
  --profile dcx-o4-vocal.json --snapshot S.json --chain-id sha256/... > applied.json
dcxctl profile verify --profile dcx-o4-vocal.json --snapshot readback.json
```

**Planning.** Each step is an ordinary `dcx.write-plan/v1` document planned
by `write plan`'s code against the previous step's predicted snapshot, so it
is exactly one `write apply` transaction (5 remote-mode writes, one frame)
with its exact touched-address rollback (3), and every write-policy rule
above holds per step. The planner orders what one step may not do:

- free moves go first, together: a lower output gain, crossover moves,
  deeper cuts on a band whose shape stays, full writes of inactive bands, and
  the retune of a band already at 0 dB;
- a band entering the active set is written before the count rises (the same
  frame may do both, because band fields precede activation);
- a cut band whose frequency, Q, kind or slope changes is raised to 0 dB in
  moves of at most the step budget, then retuned from 0 dB in a later step.
  A -6 dB band goes -6, -3, 0, then the new cut, as band 7 did on
  2026-10-21. Where that night combined the last release with the retune
  (band 6, -3 dB straight to 1053 Hz), the policy admits it, but the planner
  takes one more transaction;
- a band leaving the active set (a lower count, or PEQ off) is raised to
  0 dB before the count drops; PEQ-off bands are written back to their
  profiled values afterwards;
- rises share the step budget band by band, then the output gain;
- an active band holding a stored boost that is retuned or leaves the
  active set is first lowered to 0 dB (a cut);
- crossover moves are not counted in the rise budget, so a chain that
  loosens an active low-pass (type change, off, or a higher corner) needs
  `--allow-protection-change`, as the high-pass does in every typed write;
- a profile with a count above 8 is refused: the device refused a count of 9
  twice on 2026-10-21 (stored band 9, read back count 8), so
  `MAX_ACTIVE_PEQ_BANDS` is 8: every apply carrier on every lane refuses a
  count above 8 (`ApplyPlanError::DeviceBandCount`), `write plan` refuses it
  while planning, and the feedback lane places notches in bands up to 8.
  The stored values of inactive bands are not profiled and never written.

A high-pass loosening still needs `--allow-protection-change` (set in every
step's plan as well), and the O4 high-pass to O3 low-pass link is predicted
per step exactly as for `write plan`. Profile validation follows the Legalab schema's constants, enums, patterns
and lengths, and is narrower in one place: a band boost is refused. The
`dcx.profile-chain/v1` receipt
lists every step with its baseline and predicted digests, rise, write plan
and full transaction (frames, carriers), `rollback_rise_db` (a rollback
carries no policy: a step that adds several new cuts releases all of them
when rolled back, so this can exceed 3 dB), the whole chain's budget, the
predicted final snapshot's `final_verification`, and a `chain_id`: a digest
of the profile bytes, the baseline, the policy and every step's apply and
rollback carrier digests.

**Apply.** `profile apply` re-plans from the same profile, snapshot and
policy flags and refuses unless the chain id equals `--chain-id`. It then
runs steps `--from-step` through `--to-step` (default all) as `write apply`
transactions, 20 s apart (fixed): each takes a fresh baseline and refuses a
stale one (so a later step runs only on its predicted state), sends one
frame, and reads back, repeating a readback that failed in transport as a
separate read-only snapshot (see "Post-apply readback retry"). The first
step that does not verify stops the chain with `status: stopped`, the step
and every receipt so far:

- `applied: rollback_required` (the write was attempted): the receipt adds
  that step's `rollback_plan` and `rollback_rise_db`. Take a fresh snapshot:
  equal to the step's baseline, nothing changed; equal to its prediction, it
  landed; otherwise `control verify-containment` against the step's apply
  plan must pass before `write rollback --plan` with that rollback plan;
- `applied: not_applied` (a stale baseline, a failed capture or open: the
  transaction never reached its write): no rollback plan is given, because
  rolling a step back from a state it never touched could raise a level.

Nothing rolls back automatically. To continue after a stop, take a snapshot
and plan again from it. `--from-step` relies on the stale-baseline refusal
and does not pace its first step; the caller keeps 20 s since its previous
transaction. A full run ends with `profile_verification` of the last
readback (`status: profile_drift` if it does not match); the command exits
non-zero unless every run step verified and, after the last step, the
profile matches. A zero-step chain prints `already_matching` with
`offline: true`: nothing was sent or read.

**Self-check (offline).** `dcxctl profile plan`, run on PZM against the
saved `143d32b5` snapshot of the named unit (a lane file, not committed;
no device opened) to `dcx-o4-vocal-1dc18140`, gives 6 transactions and 30
remote-mode writes (chain `sha256/bd283b5d...` at the first PR head):

| Step | Rise | Changes |
| --- | --- | --- |
| 1 | 3 dB | bands 6-8 written (1053 Hz -5, 424 Hz -5, 2792 Hz -6), count 5 -> 8, band 2 -6 -> -3 |
| 2 | 3 dB | band 2 -3 -> 0 |
| 3 | 3 dB | band 2 -> 199 Hz low shelf -15; band 3 -5 -> -2 |
| 4 | 3 dB | band 3 -2 -> 0; band 4 -4 -> -3 |
| 5 | 3 dB | band 3 -> 654 Hz Q 6.3 -6; band 4 -3 -> 0 |
| 6 | 0 dB | band 4 -> 3393 Hz Q 0.89 -9 |

Its predicted final snapshot passes `profile verify`. Compared with the
saved `1dc18140` snapshot it differs only in the inactive band 9's stored
values (left by the night's refused count-9 write) and the Dump1 trailer
(observed, offline comparison of the two files).
`crates/dcx-core/tests/profile_chain.rs` checks the same order on a synthetic
reconstruction (codes from the Legalab records; bands 6-8 of `24296ae3`
inferred), including that the planned end state equals the replayed night
apart from band 9.

## Preset store and recall

`dcxctl` has no preset store, recall or directory read. None of the public
sources read so far documents those frames, so there is nothing reviewed to
encode. Writes change the current setup only. The device's AUTO-STORE keeps
that setup in non-volatile memory (a vendor claim, not yet confirmed on the
named unit). `control snapshot` remains the off-device archive. The
research, claims and attended probe plan are in
`docs/agent-notes/2026-10-21-preset-store-recall-research.md`.
