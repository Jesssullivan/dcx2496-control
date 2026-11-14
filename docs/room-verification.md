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
renew it. Expiry requires another actual verification.

The optional `approved-room-configuration.json` is owner-private inside the
helper App Group. Saving it validates the manifest offline and binds the exact
helper target, callout and expectation digest. All device-read switches default off.
`verifyOnForegroundLaunch` captures once when the helper opens before Logic.
An existing Logic process suppresses that automatic read. Launch the ASFW menu
receiver before the helper, then wait for admission before opening Logic.

`allowExplicitRefreshRequests` permits only the fixed
`tinyland-dcx://verify-approved-room` URL. It reads the saved manifest and target.
The URL accepts no path, query, port, profile or write parameter. Native controls
can request this foreground refresh before acquiring an LS56 transaction.
The helper has one window and never opens another for a URL. The app delegate
receives the URL, so a refresh runs when that window is closed.
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
configuration, exact expectation bytes and identity policy match. A different binding is refused
while that capture runs. Matching callers receive the same result or failure.
A successful completed comparison can reuse its original capture while the tty
exists and capture age is less than sixty seconds. Reuse preserves `verifiedAt`.
The existing two-second heartbeat expiry and sixty-second lifetime still apply.
A stale or disconnected result requires another actual capture.

Device operations, configuration or expectation changes and foreground exit
invalidate reuse. They also prevent an in-flight capture from publishing after
invalidation. A failed capture is never cached as readiness. A later explicit
request can try again. Verification never writes device parameters.

The existing read-only `helper.status` reports `activeTransactionID` as
`room-verification` during the owning capture. Status is observation, not approval
or a fresh capture. Tests cover coordinator-level concurrent triggers, shared
failure, mismatched bindings, invalidation and unchanged capture age.

## Native source validation

Authority: the operator's implementation direction, TIN-6964 and R-N13.
Source `bbaf5f64bbbcb0d41250f2932bee5d2d359c5340` passed the Lab-owned native
source lane on PZM. This receipt covers source compilation and injected tests.
It does not qualify an installed helper or approve a room baseline.

| Check | Observed result |
|---|---|
| Recipes | `just apple-package-check native-helper-ui-typecheck` |
| XCTest | 95 listed, 87 passed, 8 explicitly skipped and 0 failed |
| Room verification | 11 passed, including all six new cases |
| Helper UI | Actual SwiftUI source typecheck passed |
| Diagnostics | No compiler warnings or errors |
| Bounds | Swift jobs 1, maximum jobs 2, nice increment 10 |
| Completion | 56.381 seconds, exit 0 and owned process group quiescent |
| Archive SHA256 | `531d581f6f294731f268923111215101782da30a06b20e85ed7e01dbb7403b2e` |
| Receipt SHA256 | `b955b568c9c5d5301192670bfccfdf98bf7f715a1cc98e1c17daf3a77aa268ad` |
| Full log SHA256 | `5e3f982c9080842f0c6bf8091fe6688edad5a4e45de0844f4f8ccf87069c22b6` |

The seven real-child tests and one real Rust CLI test were skipped because their
explicit executable inputs were absent. Those skips provide no child or hardware
proof. The runner removed those executable inputs and retained its complete log.
No helper IPC or CoreMIDI endpoint was started. No application, device, serial,
installation or foreign-process action occurred.

Independent source review found and resolved cache admission and scope errors.
The final source and native packet reviews found no remaining concrete blocker.
This documentation follows the tested source without changing product code.

## Artifact signing hold

The subsequent artifact-only build of source `bbaf5f64` completed its Release
archive. Its first signing command failed on the embedded `dcxctl` with
`errSecInternalComponent`. Helper and AU signing were not reached. The caller's
bootstrap manager was Background. The underlying key-use failure remains
unverified. No signed candidate was exported and no retry or keychain workaround
was performed.

| Readback | Observed result |
|---|---|
| Recipe | Exit 1 after 132.404 seconds |
| Owned process group | Quiescent |
| Canonical build 3 | Manifest unchanged |
| Configuration | Exact prior bytes preserved |
| Scoped app and AU registrations | Unchanged |
| Rollback | Build-3 bytes and configuration preserved |
| Receipt SHA256 | `7147660a4fd75a13c98b71f21c58fd2df69dec8ce6896cfd9552634b103af0d5` |
| Log SHA256 | `20ff213974bac8cd31b450cb30d700e5189d27ccdb4e6a93e154121ab04dd402` |

A retained archive is not a verified signed helper. Installation, launch,
serial access, room approval and startup enablement remain outside this receipt.

## Periodic approved reads

`periodicVerificationEnabled` is a separate optional boolean in the existing
`dcx.approved-room-configuration/v1` document. Missing means false. Startup and
fixed-URL permission do not grant this authority. The signed helper advertises
`TinylandRoomPeriodicVerificationProtocol` as `dcx.room-periodic-verification/v1`.
Lab must bind that capability to the exact signed artifact before opting in.

The existing foreground app owns one timer. Local configuration checks run every
five seconds. An enabled timer can read while Logic plays. It shares active
verification with startup, button and URL requests. A periodic attempt bypasses
completed-cache reuse and requires an actual fresh snapshot. No MIDI input,
heartbeat, daemon or competing serial owner starts a device operation.

After a successful capture, the next periodic attempt becomes eligible after
thirty seconds. The existing local timer checks eligibility every five seconds. A real
explicit capture defers that deadline too. Known lock contention before child
admission backs off from fifteen seconds to five minutes. Failed transport,
invalid comparison and drift latch periodic reads off. An explicit successful
verification or a changed approved configuration resets that latch.

A Dump0 or Dump1 timeout after some bytes, with a verified close, retries on
the next thirty-second period. Three such partial dumps with no successful
capture between them latch periodic reads off. The reason names the cap and the
last byte count. The helper window shows it under Periodic room reads, and the
unified log records it under `io.tinyland.dcx2496.logic`. Dump0 and Dump1 are
reads, so a retry replays no write. A partial dump with an unknown close or
byte count still latches at once. No timer
performs parameter repair, rollback or unknown-child recovery.

Saved helper configuration, target, callout and exact expectation bytes must
match before capture and publication. Foreground exit or a changed binding
invalidates pending work. Joined requests add their binding checks to the shared
flight. Those checks also guard later heartbeats. An old worker cannot publish
or rearm after leaving and reentering the foreground.

Capture freshness stays sixty seconds and heartbeat expiry stays two seconds.
Only a periodic read may retain a prior successful proof during capture. Its
full configuration, callout, expectation and identity-policy key must match.
Admission reloads recovery authority first. Heartbeats retain the original
capture time, so expiration still blocks readiness during an unfinished read.
The new result replaces the old proof atomically. Drift, failed capture,
foreground exit or changed authority invalidate the prior proof. Mutating
operations and explicit fresh reads still invalidate before capture.
`helper.status.activeTransactionID` remains `room-verification` during a read.
A fresh prior proof means the earlier capture remains valid, not that the
in-flight capture has completed. Global Stop continues to block native room
admission through the existing ASFW gate.

Saved-approved verification uses the protected `profile room-refresh` path
specified in [Approved room refresh](approved-room-refresh.md). Arbitrary staged
expectations keep the generic ten-identity snapshot policy. The saved target and
configuration must equal the actual capture configuration before any child.
The helper checks complete frames, comparison binding, verified close and actual
Search counters. It retains the CLI capture-completion time. Future, stale or
pre-invocation times are refused. Receipts include measured capture metadata.

The standalone faster path passed the named-device qualification below.
Periodic delivery remains disabled until the signed helper and its foreground
workflow are qualified. Capture freshness and heartbeat expiry remain unchanged.

## Qualification record

Source `141392d4067c1b723cc5dca40bdae5813b3a0557` passed the native package and
helper UI checks on PZM. The run listed 114 tests: 106 passed, eight were
explicitly skipped and none failed. All nineteen new cases passed. The actual
SwiftUI source typecheck passed without compiler warnings. The owned process
group was quiescent after 57.439 seconds. Root released exact source packet
`a67201a61fa3805709f4c8b8786671ec48806d4bd60c5a73d79e93fefcb9fa01`.
The receipt digest is
`728fb6b643bfb94bdfe23096d9942eee6ec2fdf2c4c485bef91c93afad1c777c`.

The combined Rust state also passed `nix develop --command just check` on
Sting: 300 tests passed, none failed and none were ignored. Formatting, Clippy,
fixtures and front-door checks passed. Independent source review found no
remaining definite blocker after lifecycle, saved-binding and identity-policy
fixes. These checks requested no application launch, device access or install.

The standalone Darwin fast-refresh artifact and named-device timing remain
unqualified. No periodic option was enabled on PZM during that source run. That revision
retained the earlier five-minute cadence, failed-read latch and existing expiry.

Lexical: XL-ACC-004

## Named fast capture

On November 1, the source-141 protected reader completed one actual PZM capture.
It took 6.142 seconds: two Search attempts, one empty timeout and one valid
identity. Close verification passed. The complete 26-byte identity, 1,015-byte
Dump0 and 911-byte Dump1 matched the approved snapshot. All 62 comparison fields,
including the 55 O4 fields, matched. The controller exited zero after 6.572
seconds. The owned group was quiescent, ports were clear and helper and Logic
process custody was preserved.

The canonical snapshot remained
`sha256/20a5b485a4b4053b4b7026b9fb53517d210cbb3b456aa7f135b578c9b728b5e7`.
The actual receipt digest is
`e3facafc1bb1afb4eee67037403d2e897afa04ee353ee833221491aa2d22822a`.
The capture capsule digest is
`956c702a9f876fd1e51d5e65662131491fac59bd9a1002c1827835acdf68a76e`.
Authority was the implementation direction and TIN-5379 live-output authority
`e86eb6e8`. No muted-bench premise was asserted.

This run used a retained Nix closure after a prior unrooted artifact failed
before main with a missing runtime library. The final containing app must use
Apple-system linkage or qualified private bundled libraries. Its dependency
admission must reject every Nix load path before installation or default use.
No helper was installed, periodic option enabled or MIDI attestation fabricated
by the standalone capture.

The success delay is now thirty seconds. The protected twenty-second capture
budget plus that delay is fifty seconds, below the unchanged sixty-second
capture-age ceiling. The local five-second eligibility timer can add scheduling
latency. Failed reads still latch and old captures are never restamped. Only a
still-fresh proof with unchanged binding may survive a periodic read. One fast
capture does not qualify the installed periodic workflow or continuous Ready
availability.

Source `db7a8eea111bd403eebe862f77f036cc0bdf6729` passed the next native
source gate on November 1, 2026. It listed 122 unique tests: 114 passed,
eight were explicitly skipped and none failed. All 27 required cases passed.
The actual helper SwiftUI source typechecked without compiler warnings or
errors. The owned group and subsequent global compiler inventory were quiet.
Receipt SHA256 is `e35c02654863a4a2242188f9cd3fec84260a3321760c0ac1b15e15e40e84cdfe`.
This receipt qualifies the source change, not an installed periodic workflow.
Authority: user direction to implement the plan and Root's exact packet
`1ac3dd1e` release. Lexical: XL-ACC-005.

## Helper packaging

Helper build five declares version `0.1.4`. Its embedded Cargo build uses the
selected Xcode clang and macOS SDK in a closed environment. Ambient Rust, Nix,
compiler and dynamic-loader flags are excluded. Hidden Cargo configuration is
refused, dependencies must already be available and the lockfile stays fixed.
The build records exact tool hashes, SDK and child environment in its scratch
`apple-cli-link-inputs.json` file.

Selecting Apple tools does not prove portable linkage. Final artifact admission
checks every Mach-O image, load command and RPATH in the bundle. Every Nix or
unknown external runtime path is refused. Only Apple system paths and resolved
bundle-contained dependencies are admitted. The same check must pass on the
archive and exported signed copy. It does not repair install names or sign
binaries. Native build, signature verification and parent-owned child execution
remain separate qualification steps.

## Failure diagnostics

A failed room capture invalidates readiness before any diagnostic storage work.
The helper keeps the original failure and automatic-retry latch. Optional
`helper.status` metadata identifies the attempt, phase, typed failure, completed
child exit code and duration, output digests and whether close was verified.
A timeout or absent child never implies verified serial close.

Private records live under `DCXLogicBridge/RoomDiagnostics`. The store admits
at most eight records, each bounded to 32 kibibytes. It retains at most 16
kibibytes of child stderr per record. Directory permissions are `0700` and
exclusive record permissions are `0600`. Redirected directory leaves, symlinks,
hardlinks, collisions and unexpected files refuse persistence. Status exposes
only typed metadata and a relative record pointer. Private stderr never enters
MIDI or readiness receipts. Persistence failure reports unavailable diagnostics
without turning a failed capture into a match.

The installed build-five failure after the earlier bounded periodic trial
cannot be reconstructed. That carrier discarded child errors and removed
attempt workspaces. Build six adds diagnostic retention, not a speculative
transport repair. It preserves the serial policy, proof lifetime and failure
latch. Source tests, native compilation, installed behavior and a new actual
failure remain separate evidence. Authority: user implementation direction,
Root's diagnostic-loss source scope and TIN-6964. Lexical: XL-ACC-004.
