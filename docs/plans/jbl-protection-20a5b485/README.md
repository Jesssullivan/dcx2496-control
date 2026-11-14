# JBL protection on O1 and O2, planned from 20a5b485

Linear: TIN-6964 and TIN-5385. Authority: Legalab ruling 54,
`dec-level-target-jbl-limit-pa-105dbc-20261114`, TIN-5385 comment `f028c121`:
the JBLs run to their limit with a high-pass and limiter. Write rules:
`dec-dcx-attended-software-writes-20261019`, P-TIN-170 comment `131db306`.
Declared target: the `jbl-protected` preset in Legalab
`profiles/dcx-scene-map.json` and `jblProtection` in `profiles/levels.json`.

This is an offline plan. Nothing here was sent to a device. The plan opens no
serial port. Applying it is an attended act outside this change.

## Dry run

```sh
just jbl-protection-plan
just jbl-protection-plan --snapshot PATH-TO-20a5b485-snapshot.json
```

The first form prints each apply and chains the plans on a synthetic copy of
the `20a5b485` typed codes in `cargo test`. The second form also chains them on
the private device capture. It requires every digest, frame and plan digest in
`expected.json`, then decodes the final state with `dcxctl write show`. The
capture stays outside Git. Its decode is Legalab
`docs/agent-notes/2026-11-13-jbl-level/show-20a5b485.json`.

## Applies

Each apply is one `write apply` transaction: a fresh snapshot that must equal
the apply's baseline byte for byte, one direct-parameter frame and a full
readback. That is 5 remote-mode writes. Each rollback is the exact touched
inverse in reverse order: one frame and 3 remote-mode writes. Each apply holds
2 parameter actions, so a reading that counts parameter writes also stays
within 5 per apply and 3 per rollback. No apply addresses O3 to O6.

| Apply | Plan | Writes | Expected digest after |
|---|---|---|---|
| 1 | `apply-1-trim` | O1 and O2 gain 0.0 to -15.0 dB, code 150 to 0 | `e10681a8` |
| 2 | `apply-2-limiter-threshold` | O1 threshold -0.5 to -13.8 dB, code 235 to 102. O2 0.0 to -13.8 dB, code 240 to 102 | `852b277a` |
| 3 | `apply-3-limiter-on` | O1 and O2 limiter off to on | `b31a7e38` |
| 4 | `apply-4-high-pass-corner` | O1 high-pass corner code 0 to 37. O2 code 180 to 37. 44.575 Hz, type still off | `01d57878` |
| 5 | `apply-5-high-pass-on` | O1 and O2 high-pass type off to LR24 | `0b1f125a` |

Baseline: `sha256/20a5b485a4b4053b4b7026b9fb53517d210cbb3b456aa7f135b578c9b728b5e7`.
Full digests, frames and rollback frames are in `expected.json`.

| Apply | Apply frame | Rollback frame |
|---|---|---|
| 1 | `F0002032000E20020502000006020000F7` | `F0002032000E20020602011605020116F7` |
| 2 | `F0002032000E20020547006606470066F7` | `F0002032000E2002064701700547016BF7` |
| 3 | `F0002032000E20020546000106460001F7` | `F0002032000E20020646000005460000F7` |
| 4 | `F0002032000E20020543002506430025F7` | `F0002032000E20020643013405430000F7` |
| 5 | `F0002032000E20020542000806420008F7` | `F0002032000E20020642000005420000F7` |

Order: the trim cut goes first, so every later step plays quieter. The
threshold is set while the limiter is off. The high-pass corner moves while
the filter is off, so apply 4 makes no audible change. The limiter release
stays at the baseline codes, 142 on O1 and 141 on O2. No file declares a
release.

The planner puts 45 Hz on code 37, 44.575 Hz, the nearest step on its
logarithmic grid. The same ten writes in one plan reach the same final digest
`0b1f125a`. That single transaction also sends 5 remote-mode writes, but its
rollback carries 10 parameter actions.

## Numbers

Sources:

| Fact | Value | Source |
|---|---|---|
| DCX limiter threshold | codes 0 to 240, `code / 10 - 24` dB, so -24.0 to 0.0 dB in 0.1 dB steps | `docs/typed-writes.md`, `encode_limiter_threshold` in `crates/dcx-core/src/typed_write.rs` |
| DCX maximum output level | +22 dBu | Behringer DCX2496 quick start, Legalab source `src-wiring-dcx-quick`, sha256 `ae6c0f08` |
| DCX maximum input level | +22 dBu | Same document |
| JBL 306P MkII sensitivity | 92 dB SPL at 1 m for a -10 dBV input | JBL 3 Series MkII owner's manual, section 8, Legalab source `src-wiring-jbl-mkii-guide`, sha256 `c2033378` |
| JBL 306P MkII maximum continuous SPL | 98 dB | Same table |
| JBL 306P MkII maximum peak SPL | 110 dB | Same table |
| JBL 306P MkII maximum peak input | +6 dBV at the -10 dBV setting, +20.3 dBu at the +4 dBu setting | Same table |
| JBL product page continuous rating | 92 dB | TIN-5385 `f028c121` |
| Rack state | Both JBLs at -10 dBV, volume at maximum | Operator, TIN-5385 `7b0f53f2` |

Inferred: the threshold is in dB below DCX digital full scale, and full scale
equals the +22 dBu maximum output. Output ceiling in dBu is the threshold plus
22, so -2.0 to +22.0 dBu. Whether the limiter acts before or after the output
gain is UNKNOWN. If it acts after the gain, the trim does not move the
ceiling. Conversion: dBu equals dBV plus 2.2.

Inferred: the 92 dB on the product page equals the manual's sensitivity
figure. The page probably labels sensitivity as a continuous rating. The
manual's sensitivity is taken at the -10 dBV setting with volume at maximum,
which matches the rack.

JBL input for each rating, and the threshold that caps it:

| Switch | Limit | JBL input | Threshold, input minus 22 | Inside -24 to 0 |
|---|---|---|---|---|
| -10 dBV | Input clip, +6 dBV | +8.2 dBu, 108 dB at 1 m | -13.8 dB, code 102 | Yes |
| -10 dBV | 98 dB continuous | -10 + 6 = -4 dBV = -1.8 dBu | -23.8 dB, code 2 | Yes, 0.2 dB above the floor |
| -10 dBV | 92 dB product page | -10 dBV = -7.8 dBu | -29.8 dB | No |
| +4 dBu | Input clip, +20.3 dBu | +20.3 dBu | -1.7 dB, code 223 | Yes |
| +4 dBu | 98 dB continuous | -1.8 + 12.1 = +10.3 dBu | -11.7 dB, code 123 | Yes |
| +4 dBu | 92 dB | -7.8 + 12.1 = +4.3 dBu | -17.7 dB, code 63 | Yes |

The +4 dBu rows are inferred. The manual gives no +4 dBu sensitivity. The
12.1 dB shift is the gap between its two peak input figures, 20.3 - 8.2.

At -10 dBV with volume at maximum the limiter can stop input clipping, with
10.2 dB of threshold range to spare. It can hold a sine to the 98 dB
continuous rating only at -23.8 dB, 0.2 dB above its floor. It cannot hold
the 92 dB figure at all. Moving the JBL switch to +4 dBu puts every limit in
the middle of the range.

The plan sets -13.8 dB, the input clip limit at the attested switch position.
A peak limiter cannot hold a continuous rating, because program sits below
its peaks by its crest factor. Ruling 54 sets the continuous level as the
highest clean level the microphone measures. The level pass owns that number
and may lower the threshold in a later apply.

## Starting trim

The DCX can only cut. Gain codes run 0 to 300, `code / 10 - 15` dB, and the
write policy caps a rise at 0 dB. The plan starts O1 and O2 at -15.0 dB, the
lowest the DCX gives and `minDbTenths -150` in `profiles/levels.json`. The
level pass then rises in steps of at most 3 dB, 5 applies to reach 0 dB.

First played level per JBL at 1 m, for a stimulus at -12 dBFS sample peak
from `tools/measure/sweep.py`:

```text
SPL = 92 + (X - 12 - 15) - (-7.8) = X + 72.8 dB
```

X is the LS56 Line Out 1 and 2 level in dBu for a 0 dBFS signal with the
Monitor knob fully clockwise. X is UNKNOWN: no record states it. The stimulus
level is then the remaining lever before the first play. At any X the trim
puts the first play 15 dB below the same signal at 0 dB.

## O4 and the Alto

O4 is not in this plan. It stays unmuted, LR24 at 99.35 Hz, limiter off at
0.0 dB, as in `20a5b485`. The Alto TX315 user guide v1.4, sha256 `a278b23b`,
gives 121 dB maximum SPL at 1 m and no input sensitivity. The Alto's own
volume knob position is not recorded. The O4 threshold that protects the Alto
is therefore UNKNOWN. The O4 ceiling range is the same -2.0 to +22.0 dBu.

## Open items

| Item | Effect | Evidence |
|---|---|---|
| The `jbl-protected` preset declares O3 and O4 muted | `20a5b485` has O4 unmuted, and ruling 54 puts the Alto in the target. This plan leaves O4 as it is. Muting O4 would take a separate mute write | Legalab `profiles/dcx-scene-map.json`, `decisions.jsonl:262` |
| Legalab files hold gain 0 and a null threshold | The preset gain and `levels.json` threshold need -15.0 and -13.8 to match this plan | Legalab `profiles/levels.json` |
| O1 low-pass and O2 high-pass share code 180 in `20a5b485` | The planner models only the O4 high-pass to O3 low-pass link. If the device links O1 and O2 the same way, apply 4 also moves the O1 low-pass corner to 37 with its type off. The readback then differs from the desired state and the apply returns `rollback_required`. Both filters are off in apply 4, so nothing audible changes | `docs/typed-writes.md`, Linked O3/O4 crossover |
| Write containment | An earlier apply moved bytes outside its plan. `write verify` checks every readback | TIN-6805 |
| Limiter and high-pass addresses have no named-device readback | The first readback that passes `write verify` promotes them | `docs/typed-writes.md`, evidence class |
| Dump0 byte 101 | Already 1 in `20a5b485`, so the modified flag does not move | Device capture |
| Live build | Applying needs a `typed-write-live` build from current main | Legalab DCX lane, `dcxctl-tw-262375fb` predates #61 |
