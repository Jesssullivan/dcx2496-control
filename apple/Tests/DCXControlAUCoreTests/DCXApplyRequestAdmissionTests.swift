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
            let different = try SemanticDiffV1(
                baselineSnapshotDigest: original.baselineSnapshotDigest,
                desiredProfileDigest: original.desiredProfileDigest,
                desiredSnapshotDigest: original.desiredSnapshotDigest,
                applyPlanDigest: Fixture.digest(digit),
                rollbackPlanDigest: original.rollbackPlanDigest,
                changes: original.changes
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
            let diff = try SemanticDiffV1(
                baselineSnapshotDigest: baseline.digest, desiredProfileDigest: desired.digest,
                desiredSnapshotDigest: Self.digest("4"), applyPlanDigest: Self.digest("a"),
                rollbackPlanDigest: Self.digest("b"), changes: []
            )
            try audioUnit.performControlStateMutation { state in
                try state.stage(.init(target: target, desired: desired))
                try state.accept(snapshot: baseline, validSearchResponses: 10)
                try state.accept(diff: diff)
            }
            request = ApplyRequest(target: target, plan: try .init(baseline: baseline, desired: desired, diff: diff))
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
