import AudioToolbox
@testable import DCXControlAUCore
import DCXLogicBridge
import Foundation
import XCTest

/// AU model side of bridge v2 notch staging: pure state transitions and Logic
/// document recall. No helper, socket, child, serial, or render-path work.
final class DCXControlNotchStateTests: XCTestCase {
    func testPlannedProfileStagesWithItsBaselineAndRecallsThroughFullState() throws {
        let fixture = try Fixture()
        let source = try Fixture.makeAudioUnit()
        try source.performControlStateMutation { try $0.stage(fixture.staged, baseline: fixture.baseline) }
        let view = source.controlState.view()
        XCTAssertEqual(view.currentSnapshot, fixture.baseline)
        XCTAssertEqual(view.projectState?.schemaVersion, "dcx.logic-project-state/v2")
        XCTAssertEqual(view.projectState?.notchPlan, fixture.plan)

        try source.performControlStateMutation { try $0.accept(diff: fixture.diff(baseline: fixture.baseline.digest)) }
        let restored = try Fixture.makeAudioUnit()
        restored.fullState = source.fullState
        let recalled = restored.controlState.view()
        XCTAssertEqual(recalled.projectState, view.projectState)
        XCTAssertEqual(recalled.diff?.fieldChanges.count, 1)
        XCTAssertEqual(try restored.controlState.persistedState(), try source.controlState.persistedState())
    }

    func testV1ProjectStateKeepsItsExactLegacyShape() throws {
        let fixture = try Fixture()
        let staged = StagedProjectStateV1(target: fixture.target, desired: try Fixture.v1Profile())
        let json = try XCTUnwrap(JSONSerialization.jsonObject(
            with: BridgeJSONCodec.encoder().encode(staged)) as? [String: Any])
        XCTAssertEqual(Set(json.keys), ["schemaVersion", "target", "desired"])
        XCTAssertEqual(json["schemaVersion"] as? String, "dcx.logic-project-state/v1")
        XCTAssertNoThrow(try staged.validate())
    }

    func testStagedPlanRefusesAnyOtherBaseline() throws {
        let fixture = try Fixture()
        let other = try Fixture.snapshot(fixture.target, digest: Fixture.digest("7"))
        let state = DCXControlState()
        XCTAssertThrowsError(try state.stage(fixture.staged, baseline: other)) {
            XCTAssertEqual($0 as? DCXControlStateError, .invalidSnapshotBinding)
        }
        try state.stage(fixture.staged, baseline: fixture.baseline)
        // A fresh capture that differs from the planning snapshot cannot be
        // previewed under the stale plan.
        try state.accept(snapshot: other, validSearchResponses: 10)
        XCTAssertThrowsError(try state.accept(diff: fixture.diff(baseline: other.digest))) {
            XCTAssertEqual($0 as? DCXControlStateError, .invalidDiffBinding)
        }
        try state.accept(snapshot: fixture.baseline, validSearchResponses: 10)
        XCTAssertNoThrow(try state.accept(diff: fixture.diff(baseline: fixture.baseline.digest)))
        let v1Diff = try SemanticDiff(
            baselineSnapshotDigest: fixture.baseline.digest, desiredProfileDigest: fixture.profile.digest,
            desiredSnapshotDigest: Fixture.digest("4"), applyPlanDigest: Fixture.digest("a"),
            rollbackPlanDigest: Fixture.digest("b"), changes: [])
        XCTAssertThrowsError(try state.accept(diff: v1Diff))
    }

    func testProjectStateRejectsMismatchedPlansAndSchemaLabels() throws {
        let fixture = try Fixture()
        let otherPlan = try Fixture.plan(baseline: fixture.baseline.digest, gainCode: 60)
        XCTAssertThrowsError(try StagedProjectStateV1(
            target: fixture.target, desired: .v2(fixture.profile), notchPlan: otherPlan).validate())
        let encoded = String(decoding: try BridgeJSONCodec.encoder().encode(fixture.staged), as: UTF8.self)
        let mislabeled = encoded.replacingOccurrences(
            of: "dcx.logic-project-state/v2", with: "dcx.logic-project-state/v1")
        let decoded = try BridgeJSONCodec.decoder().decode(StagedProjectStateV1.self, from: Data(mislabeled.utf8))
        XCTAssertThrowsError(try decoded.validate())

        // A Logic document whose plan no longer matches its profile restores
        // to the empty state rather than to a partially trusted plan.
        let source = try Fixture.makeAudioUnit()
        try source.performControlStateMutation { try $0.stage(fixture.staged, baseline: fixture.baseline) }
        var document = try XCTUnwrap(source.fullState)
        let key = DCXControlPersistedStateV1.schemaVersion
        let carrier = String(decoding: try XCTUnwrap(Data(base64Encoded: try XCTUnwrap(document[key] as? String))),
                             as: UTF8.self)
        let tampered = carrier.replacingOccurrences(of: "\"gainCode\":90", with: "\"gainCode\":60")
        XCTAssertNotEqual(tampered, carrier)
        document[key] = Data(tampered.utf8).base64EncodedString()
        let restored = try Fixture.makeAudioUnit()
        restored.fullState = document
        XCTAssertNil(restored.controlState.view().projectState)
    }

    private struct Fixture {
        let target: DCXTargetReference
        let baseline: SnapshotV1
        let plan: NotchPlanSummaryV1
        let profile: DesiredProfileV2
        let staged: StagedProjectStateV1

        init() throws {
            target = try .init(bindingID: "synthetic-fixture-dcx", expectedDeviceAddress: 0)
            baseline = try Self.snapshot(target, digest: Self.digest("0"))
            plan = try Self.plan(baseline: baseline.digest, gainCode: 90)
            profile = try .init(profileID: "o4-feedback", revision: "synthetic-1", bank: .init(
                targetOutput: 4, parameterChannel: 8, actions: plan.expectedActions))
            staged = .init(target: target, desired: .v2(profile), notchPlan: plan)
        }

        func diff(baseline: String) throws -> SemanticDiff {
            try .init(
                baselineSnapshotDigest: baseline, desiredProfileDigest: profile.digest,
                desiredSnapshotDigest: Self.digest("4"), applyPlanDigest: Self.digest("a"),
                rollbackPlanDigest: Self.digest("b"),
                fieldChanges: [try .init(output: 4, field: "band1.gain", channel: 8, parameter: 0x15,
                                         before: 150, after: plan.notches[0].gainCode)])
        }

        static func plan(baseline: String, gainCode: UInt16) throws -> NotchPlanSummaryV1 {
            try .init(
                planDigest: digest("e"), baselineSnapshotDigest: baseline, measurementDigest: digest("d"),
                eqEnabledBefore: false, eqCountBefore: 0, operatorBandCount: 0,
                notches: [.init(band: 1, frequencyHz: 630, occurrences: 1, levelDb: 4, frequencyCode: 159,
                                qCode: 40, gainCode: gainCode, kindCode: 1, slopeCode: 0)],
                dropped: [.init(frequencyHz: 4_000, reason: .notchCap)])
        }

        static func v1Profile() throws -> DesiredProfileV1 {
            try .init(
                profileID: "pzm-rew-o1-peq9-qualification", revision: "2026-09-01",
                digest: "sha256/6135022f405de2d172475865d2ecac3479b898eba209c687a0e6e5d92d774ec4",
                document: .object([
                    "target_output": .number(1), "parameter_channel": .number(5), "slot": .number(9),
                    "actions": .array(zip([59, 60, 61, 62], [53, 32, 118, 1]).map { parameter, value in
                        .object(["channel": .number(5), "parameter": .number(Double(parameter)),
                                 "value": .number(Double(value))])
                    }),
                ]))
        }

        static func snapshot(_ target: DCXTargetReference, digest: String) throws -> SnapshotV1 {
            try .init(
                target: target,
                identity: .init(manufacturer: "Behringer", model: "DCX2496", deviceAddress: 0,
                                selectedBaud: 38_400, validSearchResponses: 10),
                capturedAt: Date(timeIntervalSince1970: 0), digest: digest, complete: true,
                sectionDigests: .init(identity: Self.digest("1"), dump0: Self.digest("2"), dump1: Self.digest("3")))
        }

        static func makeAudioUnit() throws -> DCXControlAudioUnit {
            try DCXControlAudioUnit(componentDescription: .init(
                componentType: kAudioUnitType_MIDIProcessor,
                componentSubType: 0x4463_7843, componentManufacturer: 0x546E_4C64,
                componentFlags: 0, componentFlagsMask: 0))
        }

        static func digest(_ digit: Character) -> String {
            "sha256/" + String(repeating: String(digit), count: 64)
        }
    }
}
