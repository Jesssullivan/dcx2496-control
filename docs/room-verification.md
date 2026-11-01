# Room verification

Linear: TIN-6964. Authority: the operator's Practice Ready plan and implementation
direction, recorded on TIN-5379 comment c05bc4e5. Lexical: XL-ACC-004.

The foreground helper's **Approved room verification** panel accepts an
independently approved expectation. **Snapshot and Verify Room** captures once,
then compares offline. It performs no parameter write. Incoming MIDI never
invokes this action.

The expectation is strict `dcx.room-expectation/v1` JSON:

| Field | Meaning |
|---|---|
| `sceneId`, `sceneRevision`, `compiledDigest` | Named scene binding |
| `presetDigest` | Shared DCX preset contract digest |
| `approvedSnapshotDigest` | Exact whole-device image accepted on the named device |
| `outputMutes` | Six physical output mutes, with O5/O6 muted |
| `profiles` | One to four embedded Legalab `DcxOutputProfileV1` documents |

Every digest uses `sha256/` plus 64 lowercase hexadecimal digits. The helper
never constructs an approved expectation from a capture. Legalab owns physical
attribution and acceptance of the baseline. No baseline is supplied by default.

`dcxctl profile room-verify --expectation approved.json --snapshot current.json`
is offline. It requires exact snapshot identity, all six mute values and every
reviewed field in each profile. O1/O2 source addresses remain unreviewed. Image
equality protects their bytes without inventing a source decoder. Any raw-image
drift refuses, including changes outside the profiled output.

The helper publishes `dcx.room-verification/v1` only after its own live capture.
The receipt contains scene, preset, expectation and snapshot digests. Its
`validationScope` is `exact-approved-snapshot-and-reviewed-profiles`. It contains
no raw device frames, serial identity or port path.

The receipt reaches the exact ASFW app CoreMIDI destination. Publication never
causes a hardware action. The consumer must independently pin `presetDigest`
and `expectationDigest`. Missing or multiple destination matches refuse delivery.

Heartbeats run once per second without serial polling. Each expires within two
seconds, never later than sixty seconds after the actual capture. Device
operations, configuration changes, helper foreground exit and disappearance of
the configured callout invalidate the attestation. A crashed helper cannot
renew it. Expiry requires another explicit verification.

The optional `approved-room-configuration.json` is owner-private inside the
helper App Group. Saving it validates the manifest offline and binds the exact
helper target, callout and expectation digest. Both switches default off.
`verifyOnForegroundLaunch` captures once when the helper opens before Logic.
An existing Logic process suppresses that automatic read. Launch the ASFW menu
receiver before the helper, then wait for admission before opening Logic.

`allowExplicitRefreshRequests` permits only the fixed
`tinyland-dcx://verify-approved-room` URL. It reads the saved manifest and target.
The URL accepts no path, query, port, profile or write parameter. Native controls
can request this foreground refresh before acquiring an LS56 transaction.
Receipt arrival itself never applies a scene. No startup configuration or
approved device image ships by default.

`just native-room-verification` runs the Darwin fixture suite. It exercises
injected child processes and receipt lifetimes, without serial or audio. Rust
room-admission tests run in the existing `profile_chain` family. Source checks
cannot establish an approved image, installed helper or working practice audio.

`just native-helper-ui-typecheck` builds the Swift package modules and checks
the containing SwiftUI app source and plist with the selected Xcode SDK. It
uses an owned temporary build directory. It produces no executable carrier,
signature, app installation or device contact. `//apple:helper_ui_typecheck`
indexes the same check for the Darwin Bazel lane.

## Shared preparation

`TinylandRoomPreparationProtocol` in the helper's signed `Info.plist` is
`dcx.room-preparation/v1`. Lab must check this value, its pinned executable hash
and the strict bundle signature before using foreground launch plus the fixed URL.
Older helpers lack this contract.

Both triggers share one in-flight verification when their complete helper
configuration and exact expectation bytes match. A different binding is refused
while that capture runs. Matching callers receive the same result or failure.
A successful completed comparison can reuse its original capture while the tty
exists and capture age is less than sixty seconds. Reuse preserves `verifiedAt`.
The existing two-second heartbeat expiry and sixty-second lifetime still apply.
A stale or disconnected result requires another explicit capture.

Device operations, configuration or expectation changes and foreground exit
invalidate reuse. They also prevent an in-flight capture from publishing after
invalidation. A failed capture is never cached as readiness. A later explicit
request can try again. No retry, parameter write or serial poll was added.

The existing read-only `helper.status` reports `activeTransactionID` as
`room-verification` during the owning capture. Status is observation, not approval
or a fresh capture. Tests cover coordinator-level concurrent triggers, shared
failure, mismatched bindings, invalidation and unchanged capture age.
