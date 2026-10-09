import CryptoKit
import Foundation
import XCTest
@testable import DCXLogicBridge

/// Synthetic bridge v2 contract: the v2 desired profile mirrors
/// `DesiredPeqBankProfileV2`, v2 diffs bind to their profile, and a notch plan
/// reply binds to its request. No socket, helper, child, or device.
final class FeedbackNotchBridgeTests: XCTestCase {
    /// dcxctl's digest for the synthetic three-notch plan staged as
    /// o4-feedback/synthetic-1 (see the helper fixture parity test).
    static let rustProfileDigest = "sha256/1a18205e2d89cf0d094da31337c04ae5328a62b9c4fe387a614c89aef6aaa16c"

    func testSwiftDigestEqualsRustForTheSyntheticThreeNotchPlan() throws {
        let plan = try Self.plan()
        let profile = try Self.profile(plan.expectedActions)
        XCTAssertEqual(profile.digest, Self.rustProfileDigest)
        XCTAssertNoThrow(try plan.requireStaged(by: profile))
        XCTAssertEqual(plan.expectedActions.count, 17)
        XCTAssertEqual(plan.expectedActions.suffix(2), [
            .init(channel: 8, parameter: 0x07, value: 3),
            .init(channel: 8, parameter: 0x06, value: 1),
        ])
    }

    func testV2ProfileRejectsEveryActionOutsideTheReviewedCutOnlyBank() throws {
        let valid = try Self.plan().expectedActions
        let rejected: [[DirectParameterActionV2]] = [
            [.init(channel: 8, parameter: 0x15, value: 151)], // boost
            [.init(channel: 8, parameter: 0x03, value: 0)], // O4 mute
            [.init(channel: 8, parameter: 0x42, value: 0)], // crossover
            [.init(channel: 8, parameter: 0x07, value: 10)], // band count
            [.init(channel: 8, parameter: 0x13, value: 321)], // frequency domain
            [.init(channel: 8, parameter: 0x16, value: 3)], // kind domain
            [.init(channel: 5, parameter: 0x15, value: 100)], // foreign channel
            [valid[0], valid[0]], // duplicate address
            [],
            Array(repeating: valid[0], count: 48),
        ]
        for actions in rejected {
            XCTAssertThrowsError(try Self.profile(actions), "\(actions)")
        }
        XCTAssertThrowsError(try DesiredProfileV2(profileID: "p", revision: "r", bank: .init(
            targetOutput: 4, parameterChannel: 5, actions: valid
        )))
        XCTAssertThrowsError(try DesiredProfileV2(profileID: "p", revision: "r", bank: .init(
            targetOutput: 7, parameterChannel: 11, actions: [.init(channel: 11, parameter: 6, value: 1)]
        )))
        // v2 is O4-only, even for a well-formed bank on another output.
        XCTAssertThrowsError(try DesiredProfileV2(profileID: "p", revision: "r", bank: .init(
            targetOutput: 1, parameterChannel: 5, actions: [.init(channel: 5, parameter: 6, value: 1)]
        )))
        for identity in ["", String(repeating: "x", count: 129), "notch-é"] {
            XCTAssertThrowsError(try DesiredProfileV2(profileID: identity, revision: "r", bank: .init(
                targetOutput: 4, parameterChannel: 8, actions: valid
            )))
        }
        // Unity gain is the cut-only ceiling, not a boost.
        XCTAssertNoThrow(try Self.profile([.init(channel: 8, parameter: 0x15, value: 150)]))
    }

    func testV2ProfileDocumentIsClosedAndDigestBound() throws {
        let profile = try Self.profile(Self.plan().expectedActions)
        guard case var .object(document) = profile.document,
              case var .array(actions)? = document["actions"],
              case var .object(first) = actions[0] else {
            return XCTFail("expected canonical document")
        }
        var extra = document
        extra["slot"] = .number(9)
        XCTAssertThrowsError(try DesiredProfileV2(
            profileID: profile.profileID, revision: profile.revision, digest: profile.digest,
            document: .object(extra)))
        first["field"] = .string("band1.frequency")
        var withField = actions
        withField[0] = .object(first)
        document["actions"] = .array(withField)
        XCTAssertThrowsError(try DesiredProfileV2(
            profileID: profile.profileID, revision: profile.revision, digest: profile.digest,
            document: .object(document)))
        actions[0] = .object(["channel": .number(8), "parameter": .number(19), "value": .number(158)])
        document["actions"] = .array(actions)
        XCTAssertThrowsError(try DesiredProfileV2(
            profileID: profile.profileID, revision: profile.revision, digest: profile.digest,
            document: .object(document))) {
            XCTAssertEqual($0 as? BridgeValidationError, .invalidProfile(
                "desired profile digest does not match its exact v2 document"))
        }
        XCTAssertThrowsError(try DesiredProfileV2(
            profileID: profile.profileID, revision: "other", digest: profile.digest,
            document: profile.document))
    }

    func testDesiredProfileEnvelopeKeepsV1BytesAndRejectsUnknownSchemas() throws {
        let v1 = try Self.v1Profile()
        let encoder = BridgeJSONCodec.encoder()
        XCTAssertEqual(try encoder.encode(DesiredProfile.v1(v1)), try encoder.encode(v1))
        let decoder = BridgeJSONCodec.decoder()
        XCTAssertEqual(try decoder.decode(DesiredProfile.self, from: encoder.encode(v1)), .v1(v1))
        let v2 = try Self.profile(Self.plan().expectedActions)
        XCTAssertEqual(try decoder.decode(DesiredProfile.self, from: encoder.encode(v2)), .v2(v2))
        let unknown = String(decoding: try encoder.encode(v2), as: UTF8.self)
            .replacingOccurrences(of: "dcx.desired-profile/v2", with: "dcx.desired-profile/v9")
        XCTAssertThrowsError(try decoder.decode(DesiredProfile.self, from: Data(unknown.utf8)))
    }

    func testV2DiffBindsEveryChangeToTheStagedProfile() throws {
        let profile = DesiredProfile.v2(try Self.profile(Self.plan().expectedActions))
        let gain = try FieldChangeV2(output: 4, field: "band1.gain", channel: 8, parameter: 0x15,
                                     before: 150, after: 90)
        let diff = try Self.diff(profile.digest, [gain])
        XCTAssertNoThrow(try diff.validate(against: profile))
        let decoded = try BridgeJSONCodec.decoder().decode(
            SemanticDiff.self, from: BridgeJSONCodec.encoder().encode(diff))
        XCTAssertEqual(decoded, diff)

        let wrongAfter = try FieldChangeV2(output: 4, field: "band1.gain", channel: 8, parameter: 0x15,
                                           before: 150, after: 80)
        let notDesired = try FieldChangeV2(output: 4, field: "band9.gain", channel: 8, parameter: 0x3d,
                                           before: 150, after: 90)
        for change in [wrongAfter, notDesired] {
            XCTAssertThrowsError(try Self.diff(profile.digest, [change]).validate(against: profile))
        }
        let v1Diff = try SemanticDiff(
            baselineSnapshotDigest: Self.digest("0"), desiredProfileDigest: profile.digest,
            desiredSnapshotDigest: Self.digest("4"), applyPlanDigest: Self.digest("a"),
            rollbackPlanDigest: Self.digest("b"), changes: [])
        XCTAssertThrowsError(try v1Diff.validate(against: profile))
        let v1Profile = DesiredProfile.v1(try Self.v1Profile())
        XCTAssertThrowsError(try Self.diff(v1Profile.digest, []).validate(against: v1Profile))
        XCTAssertThrowsError(try Self.diff(Self.digest("9"), [gain]).validate(against: profile))
    }

    func testFieldChangesRejectUnreviewedLabelsBoostsAndNoOps() {
        let cases: [(UInt8, String, UInt8, UInt8, UInt16, UInt16)] = [
            (4, "band1.q", 8, 0x15, 150, 90), // label mismatch
            (3, "band1.gain", 8, 0x15, 150, 90), // output mismatch
            (4, "band1.gain", 8, 0x15, 150, 151), // boost
            (4, "band1.gain", 8, 0x15, 90, 90), // no-op
            (4, "mute", 8, 0x03, 1, 0), // mute is not a PEQ field
            (4, "band1.gain", 8, 0x15, 301, 90), // baseline outside device domain
        ]
        for (output, field, channel, parameter, before, after) in cases {
            XCTAssertThrowsError(try FieldChangeV2(output: output, field: field, channel: channel,
                                                   parameter: parameter, before: before, after: after))
        }
        // A boost present in the baseline may be cut back.
        XCTAssertNoThrow(try FieldChangeV2(output: 4, field: "band1.gain", channel: 8, parameter: 0x15,
                                           before: 200, after: 150))
    }

    func testApplyPlanRejectsMixedProfileAndDiffGenerations() throws {
        let baseline = try Self.snapshot()
        let v2 = DesiredProfile.v2(try Self.profile(Self.plan().expectedActions))
        let v1Diff = try SemanticDiff(
            baselineSnapshotDigest: baseline.digest, desiredProfileDigest: v2.digest,
            desiredSnapshotDigest: Self.digest("4"), applyPlanDigest: Self.digest("a"),
            rollbackPlanDigest: Self.digest("b"), changes: [])
        XCTAssertThrowsError(try ApplyPlanV1(baseline: baseline, desired: v2, diff: v1Diff))
        let v2Diff = try Self.diff(v2.digest, [])
        XCTAssertNoThrow(try ApplyPlanV1(baseline: baseline, desired: v2, diff: v2Diff))
    }

    func testPlanSummaryEnforcesPlacementAndExactStaging() throws {
        let plan = try Self.plan()
        let notch = plan.notches[0]
        func with(_ notches: [PlannedNotchV1], operatorBands: UInt8 = 0) throws -> NotchPlanSummaryV1 {
            try .init(planDigest: Self.digest("e"), baselineSnapshotDigest: Self.digest("0"),
                      measurementDigest: Self.digest("d"), eqEnabledBefore: false, eqCountBefore: 0,
                      operatorBandCount: operatorBands, notches: notches, dropped: [])
        }
        func notchAt(_ band: UInt8, gain: UInt16 = 90, kind: UInt16 = 1) -> PlannedNotchV1 {
            .init(band: band, frequencyHz: notch.frequencyHz, occurrences: 1, levelDb: nil,
                  frequencyCode: notch.frequencyCode, qCode: 40, gainCode: gain, kindCode: kind, slopeCode: 0)
        }
        XCTAssertThrowsError(try with([notchAt(2)])) // not directly above operator bands
        XCTAssertThrowsError(try with([notchAt(1, gain: 150)])) // not a cut
        XCTAssertThrowsError(try with([notchAt(1, kind: 0)])) // shelf
        XCTAssertThrowsError(try with([])) // empty
        XCTAssertThrowsError(try with([notchAt(9), notchAt(10)], operatorBands: 8))
        XCTAssertNoThrow(try with([notchAt(4)], operatorBands: 3))

        var reordered = plan.expectedActions
        reordered.swapAt(0, 1)
        XCTAssertThrowsError(try plan.requireStaged(by: Self.profile(reordered)))
        XCTAssertThrowsError(try plan.requireStaged(by: Self.profile(Array(plan.expectedActions.dropLast()))))
    }

    func testPlanReplyBindsToTheExactRequest() throws {
        let baseline = try Self.snapshot()
        let text = "frequency_hz,level_db\n630,4.0\n"
        let request = try FeedbackPlanRequest(
            target: baseline.target, baseline: baseline, measurement: .frequencyList(text),
            profileID: "o4-feedback", revision: "synthetic-1")
        let reply = try Self.reply(baseline: baseline.digest, sourceText: text)
        XCTAssertNoThrow(try reply.validate(for: request))

        let otherBaseline = try Self.reply(baseline: Self.digest("7"), sourceText: text)
        let otherSource = try Self.reply(baseline: baseline.digest, sourceText: text + "1260\n")
        let otherProfile = try Self.reply(baseline: baseline.digest, sourceText: text, profileID: "other")
        let noImport = try Self.reply(baseline: baseline.digest, sourceText: text, importReceipt: false)
        for candidate in [otherBaseline, otherSource, otherProfile, noImport] {
            XCTAssertThrowsError(try candidate.validate(for: request))
        }
        let rew = try FeedbackPlanRequest(
            target: baseline.target, baseline: baseline, measurement: .rewGenericEq(text),
            profileID: "o4-feedback", revision: "synthetic-1")
        XCTAssertThrowsError(try reply.validate(for: rew)) // source kind differs

        let document: JSONValue = .object([
            "schema_version": .string("dcx.feedback-measurement/v1"),
            "target_output": .number(4), "digest": .string(Self.digest("d")),
        ])
        let imported = try FeedbackPlanRequest(
            target: baseline.target, baseline: baseline, measurement: .measurement(document),
            profileID: "o4-feedback", revision: "synthetic-1")
        XCTAssertThrowsError(try reply.validate(for: imported)) // an import ran
        XCTAssertNoThrow(try Self.reply(baseline: baseline.digest, sourceText: text, importReceipt: false)
            .validate(for: imported))
        XCTAssertThrowsError(try Self.reply(baseline: baseline.digest, measurementDigest: Self.digest("8"),
                                            sourceText: text, importReceipt: false).validate(for: imported))
    }

    func testMeasurementInputAndRequestBounds() throws {
        XCTAssertThrowsError(try FeedbackMeasurementInputV1.frequencyList(""))
        XCTAssertThrowsError(try FeedbackMeasurementInputV1.frequencyList(String(repeating: "1", count: 65_537)))
        XCTAssertThrowsError(try FeedbackMeasurementInputV1.rewGenericEq("Filter 1\u{0}"))
        XCTAssertNoThrow(try FeedbackMeasurementInputV1.frequencyList(String(repeating: "1", count: 65_536)))
        let good: [String: JSONValue] = [
            "schema_version": .string("dcx.feedback-measurement/v1"),
            "target_output": .number(4), "digest": .string(Self.digest("d")),
        ]
        var o1 = good
        o1["target_output"] = .number(1)
        var schema = good
        schema["schema_version"] = .string("dcx.notch-plan/v1")
        var digest = good
        digest["digest"] = .string("sha256/short")
        for document in [o1, schema, digest] {
            XCTAssertThrowsError(try FeedbackMeasurementInputV1.measurement(.object(document)))
        }
        XCTAssertThrowsError(try FeedbackMeasurementInputV1.measurement(.array([])))

        let baseline = try Self.snapshot()
        let other = try DCXTargetReference(bindingID: "other-fixture", expectedDeviceAddress: 0)
        XCTAssertThrowsError(try FeedbackPlanRequest(
            target: other, baseline: baseline, measurement: .frequencyList("630"),
            profileID: "o4-feedback", revision: "r"))
        XCTAssertThrowsError(try FeedbackPlanRequest(
            target: baseline.target, baseline: baseline, measurement: .frequencyList("630"),
            priorPlanDigest: "sha256/nope", profileID: "o4-feedback", revision: "r"))
    }

    func testSelectedFileTravelsByteForByteSoTheSourceDigestIsTheFileDigest() throws {
        let bytes = Data([0xEF, 0xBB, 0xBF]) + Data("frequency_hz,level_db\r\n630,4.0\r\n".utf8)
        let fileDigest = "sha256/" + SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined()
        // Foundation's UTF-8 initializer drops the BOM; the file path must not.
        XCTAssertNotEqual(Data(try XCTUnwrap(String(data: bytes, encoding: .utf8)).utf8), bytes)
        let input = try FeedbackMeasurementInputV1.file(.frequencyList, bytes: bytes)
        XCTAssertEqual(Data(try XCTUnwrap(input.text).utf8), bytes)

        let baseline = try Self.snapshot()
        let request = try BridgeRequest(requestID: "synthetic-bom", body: .feedbackPlan(.init(
            target: baseline.target, baseline: baseline, measurement: input,
            profileID: "o4-feedback", revision: "synthetic-1")))
        let decoded = try BridgeJSONCodec.decoder().decode(
            BridgeRequest.self, from: BridgeJSONCodec.encoder().encode(request))
        guard case let .feedbackPlan(carried) = decoded.body else { return XCTFail("expected plan request") }
        let text = try XCTUnwrap(carried.measurement.text)
        XCTAssertEqual(Data(text.utf8), bytes)
        XCTAssertEqual(FeedbackNotchContract.sourceDigest(text), fileDigest)

        XCTAssertThrowsError(try FeedbackMeasurementInputV1.file(.frequencyList, bytes: Data([0x36, 0xFF, 0x0A])))
        XCTAssertThrowsError(try FeedbackMeasurementInputV1.file(.rewGenericEq, bytes: Data()))
        XCTAssertThrowsError(try FeedbackMeasurementInputV1.file(.measurement, bytes: Data("630".utf8)))
    }

    func testFeedbackPlanCrossesTheWireAndBindsByRequestID() throws {
        let baseline = try Self.snapshot()
        let text = "frequency_hz,level_db\n630,4.0\n"
        let request = try BridgeRequest(requestID: "synthetic-feedback", body: .feedbackPlan(.init(
            target: baseline.target, baseline: baseline, measurement: .frequencyList(text),
            priorPlanDigest: Self.digest("e"), profileID: "o4-feedback", revision: "synthetic-1")))
        let decodedRequest = try BridgeJSONCodec.decoder().decode(
            BridgeRequest.self, from: BridgeJSONCodec.encoder().encode(request))
        XCTAssertEqual(decodedRequest, request)
        XCTAssertEqual(decodedRequest.schemaVersion, "dcx.logic-bridge/v2")

        let response = BridgeResponse(requestID: request.requestID, body: .feedbackPlan(
            try Self.reply(baseline: baseline.digest, sourceText: text)))
        let decoded = try BridgeJSONCodec.decoder().decode(
            BridgeResponse.self, from: BridgeJSONCodec.encoder().encode(response))
        XCTAssertNoThrow(try decoded.validate(for: request))
        let foreign = BridgeResponse(requestID: "other", body: decoded.body!)
        XCTAssertThrowsError(try foreign.validate(for: request))
        let failedReceipt = BridgeResponseBody.feedbackPlan(try Self.reply(
            baseline: baseline.digest, sourceText: text, planReceipt: .init(
                operation: .diffPreview, exitCode: 0, durationMilliseconds: 1, stdoutDigest: Self.digest("c"))))
        XCTAssertThrowsError(try failedReceipt.validate())
    }

    // MARK: - Synthetic fixtures

    /// The synthetic ring-out's plan against the blank snapshot, transcribed
    /// from dcxctl: 630 Hz and 1260 Hz at -6 dB, 2500 Hz recurring at -9 dB.
    static func plan(baseline: String = digest("0"), measurement: String = digest("d")) throws -> NotchPlanSummaryV1 {
        try .init(
            planDigest: digest("e"), baselineSnapshotDigest: baseline, measurementDigest: measurement,
            eqEnabledBefore: false, eqCountBefore: 0, operatorBandCount: 0,
            notches: [
                .init(band: 1, frequencyHz: 630, occurrences: 1, levelDb: 4,
                      frequencyCode: 159, qCode: 40, gainCode: 90, kindCode: 1, slopeCode: 0),
                .init(band: 2, frequencyHz: 1260, occurrences: 1, levelDb: 3,
                      frequencyCode: 191, qCode: 40, gainCode: 90, kindCode: 1, slopeCode: 0),
                .init(band: 3, frequencyHz: 2500, occurrences: 2, levelDb: 9,
                      frequencyCode: 223, qCode: 40, gainCode: 60, kindCode: 1, slopeCode: 0),
            ],
            dropped: [])
    }

    static func profile(_ actions: [DirectParameterActionV2], profileID: String = "o4-feedback") throws
        -> DesiredProfileV2 {
        try .init(profileID: profileID, revision: "synthetic-1",
                  bank: .init(targetOutput: 4, parameterChannel: 8, actions: actions))
    }

    static func reply(
        baseline: String, measurementDigest: String = digest("d"), sourceText: String,
        profileID: String = "o4-feedback", importReceipt: Bool = true,
        planReceipt: CommandReceiptV1? = nil
    ) throws -> FeedbackPlanResponse {
        let plan = try plan(baseline: baseline, measurement: measurementDigest)
        return .init(
            measurement: try .init(digest: measurementDigest, source: .frequencyList,
                                   sourceDigest: FeedbackNotchContract.sourceDigest(sourceText),
                                   targetOutput: 4, peakCount: 4),
            plan: plan,
            desired: try profile(plan.expectedActions, profileID: profileID),
            importReceipt: importReceipt ? receipt() : nil,
            planReceipt: planReceipt ?? receipt(),
            profileReceipt: receipt())
    }

    static func diff(_ profileDigest: String, _ changes: [FieldChangeV2]) throws -> SemanticDiff {
        try .init(baselineSnapshotDigest: digest("0"), desiredProfileDigest: profileDigest,
                  desiredSnapshotDigest: digest("4"), applyPlanDigest: digest("a"),
                  rollbackPlanDigest: digest("b"), fieldChanges: changes)
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

    static func snapshot() throws -> SnapshotV1 {
        let target = try DCXTargetReference(bindingID: "synthetic-fixture", expectedDeviceAddress: 0)
        return try .init(
            target: target,
            identity: .init(manufacturer: "Behringer", model: "DCX2496", deviceAddress: 0,
                            selectedBaud: 38_400, validSearchResponses: 10),
            capturedAt: Date(timeIntervalSince1970: 0), digest: digest("0"), complete: true,
            sectionDigests: .init(identity: digest("1"), dump0: digest("2"), dump1: digest("3")))
    }

    static func receipt() -> CommandReceiptV1 {
        .init(operation: .feedbackPlan, exitCode: 0, durationMilliseconds: 1, stdoutDigest: digest("c"))
    }

    static func digest(_ digit: Character) -> String {
        "sha256/" + String(repeating: String(digit), count: 64)
    }
}
