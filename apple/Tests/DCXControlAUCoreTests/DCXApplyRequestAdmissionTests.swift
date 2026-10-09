import AudioToolbox
import DCXLogicBridge
import XCTest
@testable import DCXControlAUCore

final class DCXApplyRequestAdmissionTests: XCTestCase {
    func testFirstApplyRetainsRecoveryBeforeOneDispatchAndBlocksRetry() throws {
        let fixture = try Fixture()
        var dispatchCount = 0
        try fixture.dispatch {
            dispatchCount += 1
            let state = fixture.audioUnit.controlState.view()
            XCTAssertEqual(state.transactionID, fixture.request.plan.diff.applyPlanDigest)
            XCTAssertEqual(state.rollbackBaseline, fixture.request.plan.baseline)
            XCTAssertTrue(state.deviceStateUncertain)
            XCTAssertTrue(state.recoveryActive)
        }
        XCTAssertEqual(dispatchCount, 1)
        let persisted = try fixture.audioUnit.controlState.persistedState()
        XCTAssertThrowsError(try fixture.dispatch { dispatchCount += 1 })
        XCTAssertEqual(dispatchCount, 1)
        XCTAssertEqual(try fixture.audioUnit.controlState.persistedState(), persisted)
    }

    func testUnavailableAuthorityRejectsWithoutChangingReviewedState() throws {
        let fixture = try Fixture()
        let before = try fixture.audioUnit.controlState.persistedState()
        var authorizationChecks = 0
        XCTAssertThrowsError(try DCXApplyRequestAdmission.prepare(
            fixture.request, state: fixture.audioUnit.controlState
        ) {
            authorizationChecks += 1
            return false
        }) { error in
            XCTAssertEqual(error as? DCXApplyRequestAdmissionError, .operationUnavailable)
        }
        XCTAssertEqual(authorizationChecks, 1)
        XCTAssertEqual(try fixture.audioUnit.controlState.persistedState(), before)
    }

    func testDifferentValidPreviewCannotBorrowAuthorization() throws {
        let fixture = try Fixture()
        let original = fixture.request.plan.diff
        // Each syntactically valid but different immutable plan is rejected.
        for digit in "0123456789abcdef" where digit != "a" {
            let different = try SemanticDiff(
                baselineSnapshotDigest: original.baselineSnapshotDigest,
                desiredProfileDigest: original.desiredProfileDigest,
                desiredSnapshotDigest: original.desiredSnapshotDigest,
                applyPlanDigest: Fixture.digest(digit),
                rollbackPlanDigest: original.rollbackPlanDigest,
                changes: []
            )
            let request = ApplyRequest(target: fixture.request.target, plan: try .init(
                baseline: fixture.request.plan.baseline,
                desired: fixture.request.plan.desired,
                diff: different
            ))
            let before = try fixture.audioUnit.controlState.persistedState()
            XCTAssertThrowsError(try DCXApplyRequestAdmission.prepare(
                request, state: fixture.audioUnit.controlState, isAuthorized: { true }
            )) { error in
                XCTAssertEqual(error as? DCXControlStateError, .invalidTransactionBinding)
            }
            XCTAssertEqual(try fixture.audioUnit.controlState.persistedState(), before)
        }
    }

    func testWrongTargetRejectsBeforeAuthorizationOrStateChange() throws {
        let fixture = try Fixture()
        let request = ApplyRequest(
            target: try .init(bindingID: "different-fixture-target", expectedDeviceAddress: 0),
            plan: fixture.request.plan
        )
        let before = try fixture.audioUnit.controlState.persistedState()
        var authorizationChecks = 0
        XCTAssertThrowsError(try DCXApplyRequestAdmission.prepare(
            request, state: fixture.audioUnit.controlState
        ) {
            authorizationChecks += 1
            return true
        })
        XCTAssertEqual(authorizationChecks, 0)
        XCTAssertEqual(try fixture.audioUnit.controlState.persistedState(), before)
    }

    func testTypedPreAdmissionRejectionRetainsPreviewForExplicitRetry() throws {
        let fixture = try Fixture()
        let reviewed = try fixture.audioUnit.controlState.persistedState()
        try fixture.dispatch {
            try fixture.audioUnit.performControlStateMutation {
                try $0.rejectApplyBeforeAdmission(
                    transactionID: fixture.request.plan.diff.applyPlanDigest,
                    baseline: fixture.request.plan.baseline
                )
            }
        }
        XCTAssertEqual(try fixture.audioUnit.controlState.persistedState(), reviewed)
        var retried = false
        try fixture.dispatch { retried = true }
        XCTAssertTrue(retried)
    }

    func testLostResponseSurvivesDocumentRecallWithoutAutomaticDispatch() throws {
        let fixture = try Fixture()
        var dispatchCount = 0
        XCTAssertThrowsError(try fixture.dispatch {
            dispatchCount += 1
            throw FixtureError.lostResponse
        })
        let unresolved = try fixture.audioUnit.controlState.persistedState()
        XCTAssertTrue(unresolved.deviceStateUncertain)
        let document = try XCTUnwrap(fixture.audioUnit.fullStateForDocument)
        let restored = try Fixture.makeAudioUnit()
        restored.fullStateForDocument = document
        XCTAssertEqual(try restored.controlState.persistedState(), unresolved)
        XCTAssertEqual(dispatchCount, 1)
        XCTAssertThrowsError(try DCXApplyRequestAdmission.prepare(
            fixture.request, state: restored.controlState,
            isAuthorized: { !restored.controlState.view().recoveryActive }
        ))
        XCTAssertEqual(try restored.controlState.persistedState(), unresolved)
    }

    func testSameInstanceHostRecallReplacesPreviousProjectOperationSummary() throws {
        for previousSummary in ["Semantic diff previewed", "Rollback restored the baseline digest"] {
            let fixture = try Fixture()
            var presentation = DCXControlPresentation()
            XCTAssertEqual(presentation.statusAfterStateRefresh(
                previousSummary, state: fixture.audioUnit.controlState.view()
            ), previousSummary)
            let projectB = StagedProjectStateV1(
                target: try .init(bindingID: "synthetic-project-b", expectedDeviceAddress: 1),
                desired: fixture.request.plan.desired
            )
            let savedB = DCXControlPersistedStateV1(
                projectState: projectB, currentSnapshot: nil, diff: nil,
                transactionID: nil, rollbackBaseline: nil, deviceStateUncertain: false
            )
            fixture.audioUnit.fullStateForDocument = try carrier(savedB)
            let view = fixture.audioUnit.controlState.view()
            XCTAssertEqual(try fixture.audioUnit.controlState.persistedState(), savedB)
            XCTAssertNil(view.currentSnapshot)
            XCTAssertNil(view.diff)
            XCTAssertEqual(view.restorationGeneration, 1)
            XCTAssertEqual(presentation.statusAfterStateRefresh(previousSummary, state: view),
                           DCXControlPresentation.restoredStatus)
            // Ordinary refreshes after recall preserve a new explicit result.
            XCTAssertEqual(presentation.statusAfterStateRefresh("Helper is unavailable", state: view),
                           "Helper is unavailable")
            // A fresh result can arrive before the host's queued notification.
            fixture.audioUnit.fullStateForDocument = try carrier(savedB)
            let recalledAgain = fixture.audioUnit.controlState.view()
            let freshResult = presentation.statusAfterExplicitResult(
                "Helper is unavailable", state: recalledAgain
            )
            XCTAssertEqual(presentation.statusAfterStateRefresh(freshResult, state: recalledAgain),
                           "Helper is unavailable")
        }
    }

    func testIdenticalDocumentRecallAlsoInvalidatesOldOperationSummary() throws {
        let fixture = try Fixture()
        let persisted = try fixture.audioUnit.controlState.persistedState()
        let document = try XCTUnwrap(fixture.audioUnit.fullStateForDocument)
        var presentation = DCXControlPresentation()
        let previousSummary = "Semantic diff previewed"
        XCTAssertEqual(presentation.statusAfterStateRefresh(
            previousSummary, state: fixture.audioUnit.controlState.view()
        ), previousSummary)
        for generation in UInt64(1)...3 {
            fixture.audioUnit.fullStateForDocument = document
            let view = fixture.audioUnit.controlState.view()
            XCTAssertEqual(try fixture.audioUnit.controlState.persistedState(), persisted)
            XCTAssertEqual(view.restorationGeneration, generation)
            XCTAssertEqual(presentation.statusAfterStateRefresh(previousSummary, state: view),
                           DCXControlPresentation.restoredStatus)
            XCTAssertEqual(fixture.audioUnit.fullStateForDocument?[DCXControlPersistedStateV1.schemaVersion]
                           as? String, document[DCXControlPersistedStateV1.schemaVersion] as? String)
        }
    }

    func testLegacyAndEmptyHostRecallClearPreviousOperationSummary() throws {
        let fixture = try Fixture()
        var presentation = DCXControlPresentation()
        let project = try XCTUnwrap(fixture.audioUnit.controlState.view().projectState)
        let encoded = try BridgeJSONCodec.encoder().encode(project).base64EncodedString()
        fixture.audioUnit.fullStateForDocument = [StagedProjectStateV1.schemaVersion: encoded]
        XCTAssertEqual(presentation.statusAfterStateRefresh(
            "Apply readback matched the desired digest", state: fixture.audioUnit.controlState.view()
        ), DCXControlPresentation.restoredStatus)
        XCTAssertEqual(fixture.audioUnit.controlState.view().projectState, project)
        XCTAssertNil(fixture.audioUnit.controlState.view().diff)
        fixture.audioUnit.fullStateForDocument = try invalidLegacyCarrier(project)
        XCTAssertEqual(presentation.statusAfterStateRefresh(
            "Semantic diff previewed", state: fixture.audioUnit.controlState.view()
        ), DCXControlPresentation.unstagedStatus)
        XCTAssertNil(fixture.audioUnit.controlState.view().projectState)
        XCTAssertEqual(fixture.audioUnit.controlState.view().restorationGeneration, 2)
        try fixture.audioUnit.performControlStateMutation { try $0.stage(project) }
        fixture.audioUnit.fullStateForDocument = nil
        XCTAssertEqual(presentation.statusAfterStateRefresh(
            "Rollback restored the baseline digest", state: fixture.audioUnit.controlState.view()
        ), DCXControlPresentation.unstagedStatus)
        XCTAssertNil(fixture.audioUnit.controlState.view().projectState)
        XCTAssertEqual(fixture.audioUnit.controlState.view().restorationGeneration, 3)
    }

    func testRestoredRecoverySummaryAndRejectedRecallPreservePinnedTransaction() throws {
        let fixture = try Fixture()
        // Prepare recovery locally without dispatching a helper request.
        try fixture.audioUnit.performControlStateMutation { state in
            try DCXApplyRequestAdmission.prepare(fixture.request, state: state, isAuthorized: { true })
        }
        let unresolved = try fixture.audioUnit.controlState.persistedState()
        let restored = try Fixture.makeAudioUnit()
        restored.fullStateForDocument = try XCTUnwrap(fixture.audioUnit.fullStateForDocument)
        var presentation = DCXControlPresentation()
        XCTAssertEqual(presentation.statusAfterStateRefresh(
            "Rollback restored the baseline digest", state: restored.controlState.view()
        ), DCXControlPresentation.restoredRecoveryStatus)
        XCTAssertEqual(try restored.controlState.persistedState(), unresolved)
        // An active recovery rejects replacement; it must not report a recall
        // or lose the exact transaction/baseline merely because Logic sets state.
        restored.fullStateForDocument = nil
        restored.fullStateForDocument = try invalidLegacyCarrier(
            XCTUnwrap(unresolved.projectState)
        )
        XCTAssertEqual(restored.controlState.view().restorationGeneration, 1)
        XCTAssertEqual(try restored.controlState.persistedState(), unresolved)
        XCTAssertEqual(presentation.statusAfterStateRefresh(
            "Readback is required", state: restored.controlState.view()
        ), "Readback is required")
    }

    func testPreviewResponseCannotCrossMatchingHostRestoration() throws {
        let fixture = try Fixture()
        let requestGeneration = fixture.audioUnit.controlState.view().restorationGeneration
        let savedB = DCXControlPersistedStateV1(
            projectState: fixture.audioUnit.controlState.view().projectState,
            currentSnapshot: fixture.request.plan.baseline, diff: nil,
            transactionID: nil, rollbackBaseline: nil, deviceStateUncertain: false
        )
        fixture.audioUnit.fullStateForDocument = try carrier(savedB)
        // Digests and target deliberately still match A's reply. A digest-only
        // admission would attach A's preview to the recalled document B.
        XCTAssertEqual(savedB.currentSnapshot?.digest, fixture.request.plan.diff.baselineSnapshotDigest)
        XCTAssertEqual(savedB.projectState?.desired.digest, fixture.request.plan.diff.desiredProfileDigest)
        XCTAssertThrowsError(try fixture.audioUnit.performControlStateMutation {
            try $0.accept(diff: fixture.request.plan.diff,
                          expectedRestorationGeneration: requestGeneration)
        }) { error in
            XCTAssertEqual(error as? DCXControlStateError, .supersededReadResponse)
        }
        XCTAssertEqual(try fixture.audioUnit.controlState.persistedState(), savedB)
        var presentation = DCXControlPresentation()
        XCTAssertEqual(presentation.statusAfterReadResult(
            "Semantic diff previewed", requestRestorationGeneration: requestGeneration,
            state: fixture.audioUnit.controlState.view()
        ), DCXControlPresentation.restoredStatus)

        // A new explicit preview in B is accepted. If another recall occurs
        // after model acceptance but before its summary, that summary is stale.
        let currentGeneration = fixture.audioUnit.controlState.view().restorationGeneration
        try fixture.audioUnit.performControlStateMutation {
            try $0.accept(diff: fixture.request.plan.diff,
                          expectedRestorationGeneration: currentGeneration)
        }
        XCTAssertEqual(presentation.statusAfterReadResult(
            "Semantic diff previewed", requestRestorationGeneration: currentGeneration,
            state: fixture.audioUnit.controlState.view()
        ), "Semantic diff previewed")
        fixture.audioUnit.fullStateForDocument = try carrier(savedB)
        XCTAssertEqual(presentation.statusAfterReadResult(
            "Semantic diff previewed", requestRestorationGeneration: currentGeneration,
            state: fixture.audioUnit.controlState.view()
        ), DCXControlPresentation.restoredStatus)
        XCTAssertEqual(try fixture.audioUnit.controlState.persistedState(), savedB)
    }

    func testSnapshotResponseCannotDiscardRestoredMatchingReview() throws {
        let fixture = try Fixture()
        let reviewed = try fixture.audioUnit.controlState.persistedState()
        let requestGeneration = fixture.audioUnit.controlState.view().restorationGeneration
        let document = try XCTUnwrap(fixture.audioUnit.fullStateForDocument)
        for _ in 1...3 {
            fixture.audioUnit.fullStateForDocument = document
            XCTAssertEqual(fixture.audioUnit.controlState.view().projectState?.target,
                           fixture.request.plan.baseline.target)
            XCTAssertThrowsError(try fixture.audioUnit.performControlStateMutation {
                try $0.accept(snapshot: fixture.request.plan.baseline, validSearchResponses: 10,
                              expectedRestorationGeneration: requestGeneration)
            }) { error in
                XCTAssertEqual(error as? DCXControlStateError, .supersededReadResponse)
            }
            XCTAssertEqual(try fixture.audioUnit.controlState.persistedState(), reviewed)
        }
        // The same snapshot can be captured deliberately in the current
        // document; only that new action is allowed to invalidate its diff.
        let generation = fixture.audioUnit.controlState.view().restorationGeneration
        try fixture.audioUnit.performControlStateMutation {
            try $0.accept(snapshot: fixture.request.plan.baseline, validSearchResponses: 10,
                          expectedRestorationGeneration: generation)
        }
        XCTAssertNil(fixture.audioUnit.controlState.view().diff)
        XCTAssertEqual(fixture.audioUnit.controlState.view().currentSnapshot, reviewed.currentSnapshot)
    }

    private func carrier(_ state: DCXControlPersistedStateV1) throws -> [String: Any] {
        [DCXControlPersistedStateV1.schemaVersion:
            try BridgeJSONCodec.encoder().encode(state).base64EncodedString()]
    }

    private func invalidLegacyCarrier(_ project: StagedProjectStateV1) throws -> [String: Any] {
        var document = try XCTUnwrap(JSONSerialization.jsonObject(
            with: BridgeJSONCodec.encoder().encode(project)
        ) as? [String: Any])
        document["schemaVersion"] = "unsupported-legacy-schema"
        return [StagedProjectStateV1.schemaVersion:
            try JSONSerialization.data(withJSONObject: document).base64EncodedString()]
    }

    private enum FixtureError: Error { case lostResponse }

    private struct Fixture {
        let audioUnit: DCXControlAudioUnit
        let request: ApplyRequest

        init() throws {
            audioUnit = try Self.makeAudioUnit()
            let target = try DCXTargetReference(bindingID: "synthetic-fixture-dcx", expectedDeviceAddress: 0)
            let desired = try DesiredProfileV1(
                profileID: "pzm-rew-o1-peq9-qualification", revision: "2026-09-01",
                digest: "sha256/6135022f405de2d172475865d2ecac3479b898eba209c687a0e6e5d92d774ec4",
                document: .object([
                    "target_output": .number(1), "parameter_channel": .number(5), "slot": .number(9),
                    "actions": .array(zip([59, 60, 61, 62], [53, 32, 118, 1]).map { parameter, value in
                        .object(["channel": .number(5), "parameter": .number(Double(parameter)),
                                 "value": .number(Double(value))])
                    }),
                ])
            )
            let baseline = try SnapshotV1(
                target: target,
                identity: .init(manufacturer: "Behringer", model: "DCX2496", deviceAddress: 0,
                                selectedBaud: 38_400, validSearchResponses: 10),
                capturedAt: Date(timeIntervalSince1970: 0), digest: Self.digest("0"), complete: true,
                sectionDigests: .init(identity: Self.digest("1"), dump0: Self.digest("2"), dump1: Self.digest("3"))
            )
            let diff = try SemanticDiff(
                baselineSnapshotDigest: baseline.digest, desiredProfileDigest: desired.digest,
                desiredSnapshotDigest: Self.digest("4"), applyPlanDigest: Self.digest("a"),
                rollbackPlanDigest: Self.digest("b"), changes: []
            )
            try audioUnit.performControlStateMutation { state in
                try state.stage(.init(target: target, desired: desired))
                try state.accept(snapshot: baseline, validSearchResponses: 10)
                try state.accept(diff: diff)
            }
            request = ApplyRequest(target: target, plan: try .init(baseline: baseline, desired: .v1(desired), diff: diff))
        }

        func dispatch(_ transport: () throws -> Void) throws {
            try audioUnit.performControlStateMutation { state in
                try DCXApplyRequestAdmission.prepare(request, state: state) {
                    !state.view().recoveryActive
                }
            }
            try transport()
        }

        static func makeAudioUnit() throws -> DCXControlAudioUnit {
            try DCXControlAudioUnit(componentDescription: .init(
                componentType: kAudioUnitType_MIDIProcessor,
                componentSubType: 0x4463_7843, componentManufacturer: 0x546E_4C64,
                componentFlags: 0, componentFlagsMask: 0
            ))
        }

        static func digest(_ digit: Character) -> String {
            "sha256/" + String(repeating: String(digit), count: 64)
        }
    }
}
