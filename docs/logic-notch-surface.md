# Logic notch surface (bridge v2)

Issue #46. The Logic AUv3 and foreground helper can now carry the static O4
feedback-suppression path that `dcxctl feedback` provides
([`feedback-suppression.md`](feedback-suppression.md)): plan notches offline
from a measurement, list them in the AU, stage the resulting
`dcx.desired-profile/v2`, and preview, apply, read back and roll back it through
the existing transaction carriers. Nothing here adds adaptive feedback control,
changes the reviewed allowlist, or performs device work automatically.

## Wire contract

`dcx.logic-bridge/v2` replaces v1 on the App Group socket
([`schemas/v2/dcx_logic_bridge.schema.json`](../schemas/v2/dcx_logic_bridge.schema.json);
v1 is kept frozen in `schemas/v1`). The AU is embedded in the helper bundle,
so both ends always ship together; a v1-labelled frame is refused as
`unsupported_schema`.

- `desired` is `dcx.desired-profile/v1` (unchanged, byte for byte) or
  `dcx.desired-profile/v2`. Swift validation of v2 mirrors
  `DesiredPeqBankProfileV2`: one output 1-6 with its exact channel (output + 4),
  1-47 distinct actions on that channel, only PEQ on/off (`0x06`), band count
  (`0x07`) and band fields (`0x13`-`0x3f`), each value inside its device
  domain, gains cut-only (code 150 = 0 dB is the ceiling), a closed document
  with no unknown keys, and the same domain-separated digest. A test asserts the
  Swift digest equals dcxctl's for the synthetic three-notch plan.
- `diff` is `dcx.semantic-diff/v1` (at most one O1/PEQ9 slot change) or
  `dcx.semantic-diff/v2` (the per-field `output`, `field`, `channel`,
  `parameter`, `before`, `after` changes `control diff` emits for v2). A diff
  binds to its profile by generation and digest, and every v2 change must land
  on one of the profile's actions with the profile's value.
- `feedback.notch.plan` is a new offline, read-only operation. The request
  carries the target, a complete helper-stored baseline snapshot reference,
  the measurement (ring-out list text, REW Generic EQ text, or an imported
  `dcx.feedback-measurement/v1` document), an optional prior plan digest, and
  the profile identity. The reply carries a sanitized measurement summary, the
  notch plan summary (bands, codes, measured frequency, occurrences, dropped
  peaks and reasons), the v2 desired profile, and one receipt per child.

## Helper

The helper runs only offline children for `feedback.notch.plan`, each bounded
to 45 seconds, with no tty argument and without the device operation lock:

1. `feedback import --frequency-list|--rew <text> --target-output 4`, skipped
   for an imported measurement document. The helper checks that the reported
   `source_digest` is the sha256 of the exact text the AU sent, so it matches
   legalab's `studio-measure` `cumulativeDigestAfter` for the same file.
2. `feedback plan --measurement ... --snapshot <helper raw snapshot>
   [--prior-plan <helper stored plan>]`. The prior plan is resolved before any
   child runs; a missing prior is `invalid_request`.
3. `feedback desired-profile --plan <stored plan> --profile-id ... --revision ...`.

The raw plan is stored immutably under the App Group `Plans/notch/<digest>.json`
so a later round can name it as the prior plan by digest. The helper requires
the plan's own action list to equal what its notches imply (labels included),
the plan to be bound to the requested baseline and measurement, and the v2
profile to validate in Swift and carry exactly those actions. Any mismatch is
`malformed_child_response`. The operation must be enabled in the helper
configuration ("Feedback notch plan (offline)") and is unavailable while a
mutation recovery is pinned.

## Audio Unit

The AU view gains an O4 notch section:

- a measurement kind (ring-out list, REW Generic EQ, measurement JSON), a
  "Recurrence: build on staged plan" switch, **Plan Notches…** and
  **Stage Notch Plan**;
- a read-only list of the pending or staged plan: plan and baseline digests,
  the operator bands kept, and per notch the band, encoded frequency, cut and
  Q, measured frequency and occurrence count, followed by dropped peaks.

Plan Notches reads one operator-selected file in the UI (never in the render
path) against the latest complete snapshot of the configured target. Stage
Notch Plan is a pure model update: it stages the v2 profile and the plan
summary in Logic project state (`dcx.logic-project-state/v2`) together with
the exact planning snapshot. Previewing the diff is refused if the current
snapshot differs from the plan's baseline; replan instead. Apply, readback and
rollback are unchanged and explicit. Projects holding a v1 profile keep their
exact `dcx.logic-project-state/v1` bytes. **Stage Desired Profile…** now
accepts either v1 or v2 desired-profile files.

## Tests

- `//apple:feedback_notch` (`just native-feedback-notch`, Darwin only):
  bridge v2 bindings, helper planning over exact dcxctl fixtures, AU staging and
  recall, and one end to end run of the real dcxctl's offline feedback
  subcommands through the production process runner.
- `//:apple_feedback_fixture_parity` (in `//:check` and `just fixtures`):
  regenerates the Swift fixtures under
  `apple/Tests/DCXLogicHelperCoreTests/Fixtures/feedback/` from the synthetic
  ring-out and blank snapshot and requires identical output.

Every fixture is synthetic. None is a measurement of any room or device.
