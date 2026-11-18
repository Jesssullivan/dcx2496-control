# Linked crossover corners, planned from fa3b05b4

Linear: TIN-5385. Authority: Legalab ruling 54,
`dec-level-target-jbl-limit-pa-105dbc-20261114`, for the JBL high-pass.
Ruling 113, TIN-5385 comment `b3d25971`, for the bass on the sub. Write
rules: `dec-dcx-attended-software-writes-20261019`, P-TIN-170 comment
`131db306`.

These are offline plans. Nothing here was sent to a device. Each apply is
an attended act outside this change, with speakers off, after a fresh
snapshot. The baseline `fa3b05b4` is the image after the limiter removal of
November 18. Plan each file against a fresh snapshot on the day.

## Plans

| Plan | Writes | Linked side | Consent |
| --- | --- | --- | --- |
| `jbl-1-high-pass-corner` | O1 and O2 high-pass corner 45 Hz, code 37, type off | O1 low-pass follows O2 from 180 to 37, type off. Predicted, observed on November 16 | None |
| `jbl-2-high-pass-on` | O1 and O2 high-pass type LR24 | None | None |
| `sub-o3-low-pass-80hz-alone` | O3 low-pass LR24 80 Hz | O4 high-pass, the Alto protection at 99.35 Hz | Refused, `LinkedProtectionChange` |
| `sub-o3-low-pass-80hz-with-o4` | O3 low-pass LR24 80 Hz and O4 high-pass LR24 80 Hz, code 64 | Both sides written | `allow_protection_change`. The Alto high-pass goes down |
| `sub-o3-low-pass-120hz-with-o4` | O3 low-pass LR24 and O4 high-pass LR24, code 83, 120.733 Hz | Both sides written | None. The Alto high-pass goes up |

The sub corner is the operator's pick. The 80 Hz and 120 Hz files show
the two cases, one below the Alto corner and one above it. O3 stays muted
in every plan. Unmuting it is a separate plan.

Each plan holds at most 3 actions in one frame, so it stays inside 5
remote-mode writes per apply and 3 per rollback under either reading.

## Replay

`crates/dcx-core/tests/linked_corners.rs` replays every plan on a fake DCX,
with and without the two unseen reverse links. Every apply verifies
exactly and every rollback returns the exact baseline. Its synthetic image
holds the `fa3b05b4` codes at the fields the plans read.

Two ignored tests replay from the private captured bytes:

```sh
DCX_REPLAY_VISIT1_DIR=~/.local/state/legalab/level-session-visit-1-20261116/dcx \
DCX_REPLAY_LIMITER_DIR=~/.local/state/legalab/jbl-limiter-20261118/dcx \
  cargo test --locked --jobs 2 -p dcx-core --test linked_corners -- --ignored
```

## Dry run from the private fa3b05b4

Run on sting with `dcxctl write plan` at this change, against the private
`fa3b05b4` snapshot of November 18, `L2.json`. Opens no device.

| Plan | Result | Desired digest | Apply frame | Rollback frame |
| --- | --- | --- | --- | --- |
| `jbl-1-high-pass-corner` | Planned. Linked effect O2 to O1, 180 to 37, observed | `eac5555e` | `F0002032000E20020543002506430025F7` | `F0002032000E20020643013405430000F7` |
| `jbl-2-high-pass-on`, from `eac5555e` | Planned. No linked effect | `8180b696` | `F0002032000E20020542000806420008F7` | `F0002032000E20020642000005420000F7` |
| `sub-o3-low-pass-80hz-alone` | Refused, `LinkedProtectionChange`, O4 high-pass at 99.349 Hz | | | |
| `sub-o3-low-pass-80hz-with-o4` | Refused without `--allow-protection-change`, `ProtectionChange` on O4. Planned with it | `8bbc3638` | `F0002032000E2003074500400744000808430040F7` | `F0002032000E20030843004A074400000745004AF7` |
| `sub-o3-low-pass-120hz-with-o4` | Planned with no consent | `86272a0a` | `F0002032000E2003074500530744000808430053F7` | `F0002032000E20030843004A074400000745004AF7` |

The ignored replay `private_visit_1_apply_4_readback_is_the_desired_state`
passed on sting. From the captured `b31a7e38`, the fake device's readback of
`jbl-1-high-pass-corner` equals the device readback `e2088871` byte for
byte, and that is the plan's desired digest. The same test reads
`after-rb4.json` equal to `b31a7e38`. `private_fa3b05b4_plans_replay_on_the_fake_device`
also passed from `L2.json`.
