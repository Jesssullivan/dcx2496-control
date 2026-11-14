# Helper 11 install packet

Linear: TIN-6964 and TIN-5385. Authority: Legalab ruling 55, TIN-5385 comment
`8b6baa67`. The single-window helper fix, PR 67, may be installed on PZM once
its review approves. Legalab `dec-autonomous-muted-bench-20261007` is the
standing bench authority. R-N13. Lexical: XL-ACC-004.

This packet was written without installing, signing or launching anything. It
read PZM files and code signatures only. The lead runs every PZM step below.

| Label | Meaning |
|---|---|
| **receipt** | A past PZM receipt shows this command working. The receipt is named |
| **source** | Code at a named revision backs the step. It has not run for this build |
| **UNPROVEN** | No code and no receipt back the step yet. The step names what is missing |

Marks on steps:

| Mark | Meaning |
|---|---|
| **SIGN** | Needs the Developer ID identity in the login keychain of the PZM user's Aqua session |
| **PRIV** | Enters root through lab's per-host become file. No agent types a password |
| **PROMPT** | macOS may show a dialog. Only the operator answers it |
| **LIFE** | Starts or stops an app. Ruling 55 covers it for this install |

## Lineage

PR 67 builds on Helper 9. The answer rests on these records.

| Record | What it says |
|---|---|
| `/Applications/DCXLogicHelper.app` Info.plist, read November 14 | `CFBundleVersion` 9, `CFBundleShortVersionString` 0.1.8 |
| `codesign -dv` on that app, November 14 | Identifier `io.tinyland.dcx2496.logic`, TeamIdentifier `QP994XQKNH`, hardened runtime, signed November 2 at 04:32:24 local |
| PZM state `dcx-install-helper9-20261102/transaction.jsonl` | Source `43086d44`, build 8 to build 9, last phase `complete`, status `installed-registered-not-launched` |
| PZM state `dcx-install-helper10-20261102/transaction.jsonl` | Source `9bff256e`, build 9 to build 10 at 17:38Z on November 2, then phase `build9-restored-and-registered`, status `rolled-back`. Build 10 stays in that folder as `new-helper.bundle-quarantine` |
| Lab `nix/lib/pzm-practice-artifacts.nix` on xoxd-ai/lab main `8ca03ba4` | The `dcx` pin is still version 0.1.4, build 5, source `18be049b`. `tinyland.pzmPractice.enable = false`, so the stale pin gates nothing today |
| Lab #3288, merged November 7 | Landed the Helper 9 placement source, `scripts/validation/pzm_dcx_helper9_upgrade.py` |
| Lab branch `feat/TIN-6964-helper10-placement-20261102` | The Helper 10 placement adapter. Not merged |
| This repo | `43086d44` is an ancestor of main and of PR 67. `9bff256e` and `feat/TIN-6964-helper10-recovery-20261102` are not |

PR 67 forks from main `7b2af6c`, the Helper 9 source line that lab #3288
placed. It carries none of the Helper 10 commits. Build 10 is taken by the
rolled-back `9bff256e` candidate, so PR 67 sets version 0.1.10 and build 11.
A version readback of 11 then names this source and nothing else.

No record says Helper 10 will be installed next. If that changes, the PR 67
commits must be carried onto the Helper 10 branch first.

## Mac unit gate

Run on PZM on November 14 at head `202c1e35`. macOS 26.7.1 build 25G313,
Xcode 27.1 build 27A9275, Apple Swift 6.4. The tree was a fresh clone under
the LegalabCache rebuildable root. Each command ran under
`lockf -k -t 1800 ~/git/legalab-lanes/.pzm-build.lock nice -n 10` with
`TMPDIR` and `TEST_TMPDIR` inside that tree. The tree was removed afterwards.

| Command | Result |
|---|---|
| `bash apple/Tests/run_helper_ui_typecheck.sh apple/Package.swift apple/DCXLogicHelper/DCXLogicHelperApp.swift apple/DCXLogicHelper/Info.plist` | rc 0. `DCX_HELPER_UI_TYPECHECK=pass NO-CARRIER NO-INSTALL NO-DEVICE` |
| `env -u SDKROOT DEVELOPER_DIR=XCODE XCODE_TOOLCHAIN/swift test --package-path apple --scratch-path SCRATCH --jobs 1` | rc 0. DCXLogicHelperCoreTests 126 with 8 skipped, DCXControlAUCoreTests 19, DCXLogicBridgeTests 20. 0 failures |
| `bash apple/Tests/run_room_verification.sh apple/Package.swift` | rc 0. 42 tests, 0 failures |

The 8 skips need a real child process or a real `dcxctl`: 7
DCXCTLProcessRunnerTests and `FeedbackNotchHelperTests.testRealDcxctlProducesTheFixturePlan`.
Not run: `just native-room-verification`, which starts Bazel, and
`just apple-bundle-check`. No signing, install, app launch, serial port or
CoreMIDI endpoint was used.

## How Helper 9 was built, signed and placed

| Stage | Command | Record |
|---|---|---|
| Metadata | Commit `43086d44` set version 0.1.8 and build 9 | This repo |
| Build and sign | Lab `just pzm-dcx-team-signed-build CHECKOUT REVISION SCRATCH`. It enters the PZM user's Aqua session and runs `nix develop --command just apple-team-signed-bundle` in the pinned checkout. The recipe archives unsigned, then signs CLI, AU and helper with the existing Developer ID identity. It never imports or unlocks a keychain | PZM state `dcx-helper9-product-20261102`, logs written November 2 at 04:30 to 04:32. Lab notes `2026-11-01-TIN-6964-dcx-aqua-product.md` and `2026-11-01-TIN-6964-dcx-aqua-sign-only.md` |
| Placement | Lab `just pzm-dcx-helper9-upgrade MODE OPERATION_DIR LINKAGE_CHECKER OWNED_APP_CUSTODY`, modes `plan`, `install` and `rollback`. Root does only fixed inode-bound renames. It never launches or signals an app | PZM state `dcx-install-helper9-20261102`. Lab note `2026-11-02-TIN-6964-helper9-owned-app-placement.md` |
| Handoff | First launch and runtime readback after placement | PZM state `dcx-helper9-handoff-20261102` and `dcx-helper9-existing-app-runtime-20261102` |

The Helper 9 adapter pins exact digests. Its new-bundle manifest is
`55d4cd4c15e2ddc7745063a1fd5f3b1dcd9abf6d347af26e721730ef3da26eef`. Its helper
executable is `02f543e246a8b7e0aa7dedf89ee69bb9bb07584f86c69bc03e25785244362d0c`.
The Helper 10 transaction recorded the same manifest as `oldManifest` when it
replaced build 9, and its rollback restored build 9.

## Steps for Helper 11

| # | Step | Mark | Basis |
|---|---|---|---|
| 1 | Review approves PR 67. The lead merges it or pins its approved head. Name the source revision `REV` | | |
| 2 | Swift unit tests on PZM at `REV`, unit only, under the shared build lock. Repeat only if `REV` differs from `202c1e35` in Swift source | | **receipt**: Mac unit gate below |
| 3 | `just pzm-dcx-team-signed-build CHECKOUT REV SCRATCH` from lab, with a clean checkout of `REV` under the scratch root | **SIGN**, **PRIV** | **receipt** for Helper 9 on November 2. Not run for `REV` |
| 4 | Candidate readback before any placement. Run the commands under Readbacks on the signed bundle in scratch. Record the bundle manifest and three executable digests | | **source** |
| 5 | Write a lab Helper 11 placement adapter. Copy the Helper 9 adapter. Set `OLD` to the Helper 9 manifest above and the old binaries to the Helper 9 new binaries. Set `NEW` and the new binaries from step 4. Bind the owned-App custody to the running App 61 actor, not App 57 | | **UNPROVEN**: no Helper 11 adapter exists. The unmerged Helper 10 adapter on lab `feat/TIN-6964-helper10-placement-20261102` is the closest template |
| 6 | Run that adapter in `plan` mode | **PRIV** | **source** once step 5 lands |
| 7 | Quit the running helper. Logic stays closed. The adapter refuses a live helper, AU host, Logic, DCX client or serial holder | **LIFE** | Helper PID 868 was running on November 14 |
| 8 | Run the adapter in `install` mode. It preserves build 9 outside `/Applications` as `old-helper.bundle-retired` in the operation folder | **PRIV** | **receipt** for Helper 9 |
| 9 | Launch `/Applications/DCXLogicHelper.app` once | **LIFE**, **PROMPT** | A changed signature can bring back an App Group or Automation dialog. Only the operator answers it |
| 10 | Run every readback below | | **source** |
| 11 | Run the PR 67 Mac checks 1 to 5 | **LIFE** | Check 5 is a Dock click. A person or Fruitctl makes it |
| 12 | Lab PR: set the `dcx` block in `nix/lib/pzm-practice-artifacts.nix` to build 11 and its digests | | Separate lab review |

## Readbacks

Run these on PZM as the logged-in user. All are read only except the URL open
in row 5. That open asks the helper for one saved-room read, a device read
the approved configuration already allows.

| # | Proves | Command | Pass |
|---|---|---|---|
| 1 | Version | `plutil -p /Applications/DCXLogicHelper.app/Contents/Info.plist` | `CFBundleVersion` 11, short version 0.1.10 |
| 2 | Signature | `codesign --verify --strict --deep /Applications/DCXLogicHelper.app` then `codesign -dv` on the app | Exit 0. Identifier `io.tinyland.dcx2496.logic`, TeamIdentifier `QP994XQKNH`, runtime flag set |
| 3 | Same bytes as the candidate | `shasum -a 256` on `Contents/MacOS/DCXLogicHelper`, the AU executable and `Contents/Helpers/dcxctl` | Equal to step 4 |
| 4 | AU registration | `pluginkit -m -v -i io.tinyland.dcx2496.logic.midi` | One row, version 0.1.10, path inside `/Applications/DCXLogicHelper.app` |
| 5 | One window after a verify request | `open tinyland-dcx://verify-approved-room`, then the window count below | Exactly one helper window and one helper process |
| 6 | Room receipt | `cat "$HOME/Library/Application Support/ASFW/LS56/dcx-room-verification.json"` | `status` `match`, `matches` true, `sceneID` `practice-ready`, `sceneRevision` 1, `compiledDigest` starting `60566ba2`, `expectedSnapshotDigest` and `snapshotDigest` starting `20a5b485`, `captureMetrics.closeVerified` true, a `helperInstanceID` new since step 9 |
| 7 | Reader state | Click Refresh Local Status in the helper window. Or read `log show --last 30m --predicate 'subsystem == "io.tinyland.dcx2496.logic"'` | Periodic room reads reads `Not stopped`. The log has no `periodic room reads stopped` line |

Window count for row 5. It reads the window list only and needs no
permission for owner names:

```sh
swift -e 'import CoreGraphics
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
print(list.filter { ($0[kCGWindowOwnerName as String] as? String) == "DCXLogicHelper" && ($0[kCGWindowLayer as String] as? Int) == 0 }.count)'
pgrep -x DCXLogicHelper | wc -l
```

Row 6 holds only while the DCX is still at snapshot `20a5b485`. After any
JBL protection apply from `docs/plans/jbl-protection-20a5b485`, the approved
expectation must change first, or row 6 reads drift.

## Rollback

The retained current helper is build 9, the bytes in `/Applications` today.
No other copy of build 9 was found on PZM. The older retained bundles are
build 3 and earlier, and the Helper 9 operation folder holds build 8. So step 8
is the only step that creates the rollback copy.

1. Quit the helper. **LIFE**
2. Run the Helper 11 adapter in `rollback` mode. It renames build 11 out and
   build 9 back, and re-registers build 9. **PRIV**. **source**: the Helper 10
   transaction ran the same rollback on November 2 and recorded
   `build9-restored-and-registered`.
3. Read back: `CFBundleVersion` 9, short version 0.1.8, helper executable
   `02f543e2`, AU version 0.1.8 in `pluginkit`, `codesign --verify --strict
   --deep` exit 0.
4. Launch build 9 once and read the room receipt as in row 6. **LIFE**

## Not covered

- No signed Helper 11 exists yet. No placement adapter exists for it.
- The runtime wiring of the reader claim, the rearm and the URL routing has
  no automated test. Mac checks 1 to 5 and readback rows 5 to 7 cover it.
- The rearm retries a verified-close partial dump twice at thirty seconds and
  stops on the third. No record yet shows why partial dumps happen.
