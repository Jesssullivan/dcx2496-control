import Foundation
import XCTest
@testable import DCXLogicBridge

final class BridgeResponseBindingTests: XCTestCase {
    func testGeneratedOperationPairsAcceptOnlyTheRequestedOperation() throws {
        let fixture = try Fixture()
        for requested in BridgeOperation.allCases {
            let request = try fixture.request(requested)
            for returned in BridgeOperation.allCases {
                let response = try fixture.wire(fixture.response(returned))
                if requested == returned {
                    XCTAssertNoThrow(try response.validate(for: request))
                } else {
                    XCTAssertThrowsError(try response.validate(for: request))
                }
            }
        }
    }

    func testGeneratedErrorRepliesRequireExactRequestAndOperation() throws {
        let fixture = try Fixture()
        for requested in BridgeOperation.allCases {
            let request = try fixture.request(requested)
            for returned in BridgeOperation.allCases {
                let response = try fixture.wire(BridgeResponse(
                    requestID: request.requestID, operation: returned,
                    error: .init(code: .operationUnavailable, message: "fixture denial", retryable: false)
                ))
                if requested == returned {
                    XCTAssertNoThrow(try response.validate(for: request))
                } else {
                    XCTAssertThrowsError(try response.validate(for: request))
                }
            }
            for responseID in ["different-request", ""] {
                let response = BridgeResponse(requestID: responseID, operation: requested,
                    error: .init(code: .operationUnavailable, message: "fixture denial", retryable: false))
                XCTAssertThrowsError(try response.validate(for: request))
            }
            let noOperation = BridgeResponse(requestID: request.requestID, operation: nil,
                error: .init(code: .operationUnavailable, message: "fixture denial", retryable: false))
            XCTAssertThrowsError(try noOperation.validate(for: request))
        }
    }

    func testReadbackEqualityIsBoundToRequestedDigestInBothDirections() throws {
        let fixture = try Fixture()
        for expected in [fixture.baseline.digest, fixture.desired.digest] {
            let request = try BridgeRequest(requestID: Fixture.requestID, body: .readback(.init(
                target: fixture.target, transactionID: fixture.diff.applyPlanDigest,
                expectedDesiredDigest: expected
            )))
            for actual in [fixture.baseline.digest, fixture.desired.digest] {
                for claimed in [false, true] {
                    let response = try fixture.wire(fixture.response(
                        .readback, snapshotDigest: actual, matchesDesired: claimed
                    ))
                    if claimed == (actual == expected) {
                        XCTAssertNoThrow(try response.validate(for: request))
                    } else {
                        XCTAssertThrowsError(try response.validate(for: request))
                    }
                }
            }
        }
    }

    func testRepliesCannotBorrowAnotherTargetBindingOrDeviceAddress() throws {
        let fixture = try Fixture()
        let otherTargets = [
            try DCXTargetReference(bindingID: "different-fixture", expectedDeviceAddress: 0),
            try DCXTargetReference(bindingID: fixture.target.bindingID, expectedDeviceAddress: 1),
        ]
        for operation in [BridgeOperation.snapshotCapture, .apply, .readback, .rollback] {
            let request = try fixture.request(operation)
            for otherTarget in otherTargets {
                let response = try fixture.wire(fixture.response(operation, target: otherTarget))
                XCTAssertThrowsError(try response.validate(for: request))
            }
        }
        let identityRequest = try fixture.request(.identitySearch)
        let wrongIdentity = try fixture.wire(fixture.response(.identitySearch, target: otherTargets[1]))
        XCTAssertThrowsError(try wrongIdentity.validate(for: identityRequest))
    }

    func testGeneratedForeignTransactionAndDigestBindingsFailClosed() throws {
        let fixture = try Fixture()
        for digit in "0123456789abcdef" {
            let digest = Fixture.digest(digit)
            for operation in [BridgeOperation.apply, .readback, .rollback] where digit != "a" {
                let request = try fixture.request(operation)
                let response = try fixture.wire(fixture.response(operation, transactionID: digest))
                XCTAssertThrowsError(try response.validate(for: request))
            }
            for operation in [BridgeOperation.apply, .rollback, .diffPreview] where digit != "0" {
                let request = try fixture.request(operation)
                let response = try fixture.wire(fixture.response(operation, baselineDigest: digest))
                XCTAssertThrowsError(try response.validate(for: request))
            }
            if digit != "4" {
                let request = try fixture.request(.apply)
                let response = try fixture.wire(fixture.response(.apply, desiredSnapshotDigest: digest))
                XCTAssertThrowsError(try response.validate(for: request))
            }
            let request = try fixture.request(.diffPreview)
            let response = try fixture.wire(fixture.response(.diffPreview, desiredProfileDigest: digest))
            XCTAssertThrowsError(try response.validate(for: request))
        }
    }

    func testMissingCaptureRemainsValidUncertaintyWithoutFabricatedEquality() throws {
        let fixture = try Fixture()
        let apply = BridgeResponse(requestID: Fixture.requestID, body: .apply(.init(
            transactionID: fixture.diff.applyPlanDigest, baselineDigest: fixture.baseline.digest,
            desiredSnapshotDigest: fixture.desired.digest, readback: nil,
            readbackMatchesDesired: false, rollbackRequired: true, rollbackAvailable: true,
            receipt: Fixture.receipt(.apply)
        )))
        let rollback = BridgeResponse(requestID: Fixture.requestID, body: .rollback(.init(
            transactionID: fixture.diff.applyPlanDigest, baselineDigest: fixture.baseline.digest,
            restored: nil, equalsBaseline: false, receipt: Fixture.receipt(.rollback)
        )))
        XCTAssertNoThrow(try fixture.wire(apply).validate(for: fixture.request(.apply)))
        XCTAssertNoThrow(try fixture.wire(rollback).validate(for: fixture.request(.rollback)))
    }

    func testSuccessReplyRequiresTheOriginalRequestID() throws {
        let fixture = try Fixture()
        for operation in BridgeOperation.allCases {
            let request = try fixture.request(operation, requestID: "different-request")
            let response = try fixture.wire(fixture.response(operation))
            XCTAssertThrowsError(try response.validate(for: request))
        }
    }

    /// Synthetic, sanitized references only; these values establish correlation
    /// behavior, not protocol decoding or named-device snapshot evidence.
    private struct Fixture {
        static let requestID = "synthetic-request"
        let target: DCXTargetReference
        let profile: DesiredProfile
        let feedback: FeedbackPlanRequest
        let baseline: SnapshotV1
        let desired: SnapshotV1
        let diff: SemanticDiff

        init() throws {
            target = try .init(bindingID: "synthetic-fixture", expectedDeviceAddress: 0)
            profile = .v1(try .init(
                profileID: "pzm-rew-o1-peq9-qualification", revision: "2026-09-01",
                digest: "sha256/6135022f405de2d172475865d2ecac3479b898eba209c687a0e6e5d92d774ec4",
                document: .object([
                    "target_output": .number(1), "parameter_channel": .number(5), "slot": .number(9),
                    "actions": .array(zip([59, 60, 61, 62], [53, 32, 118, 1]).map { parameter, value in
                        .object(["channel": .number(5), "parameter": .number(Double(parameter)),
                                 "value": .number(Double(value))])
                    }),
                ])
            ))
            baseline = try Self.snapshot(target: target, digest: Self.digest("0"))
            feedback = try .init(target: target, baseline: baseline,
                                 measurement: .frequencyList(Self.ringOut),
                                 profileID: "o4-feedback", revision: "synthetic-1")
            desired = try Self.snapshot(target: target, digest: Self.digest("4"))
            diff = try .init(
                baselineSnapshotDigest: baseline.digest, desiredProfileDigest: profile.digest,
                desiredSnapshotDigest: desired.digest, applyPlanDigest: Self.digest("a"),
                rollbackPlanDigest: Self.digest("b"), changes: []
            )
        }

        func request(_ operation: BridgeOperation, requestID: String = Self.requestID) throws -> BridgeRequest {
            let body: BridgeRequestBody
            switch operation {
            case .helperStatus: body = .helperStatus(.init())
            case .identitySearch: body = .identitySearch(.init(target: target))
            case .snapshotCapture: body = .snapshotCapture(.init(target: target))
            case .diffPreview: body = .diffPreview(.init(target: target, baseline: baseline, desired: profile))
            case .apply: body = .apply(.init(target: target, plan: try .init(baseline: baseline, desired: profile, diff: diff)))
            case .readback: body = .readback(try .init(target: target, transactionID: diff.applyPlanDigest,
                                                    expectedDesiredDigest: desired.digest))
            case .rollback: body = .rollback(.init(target: target, plan: try .init(
                transactionID: diff.applyPlanDigest, baseline: baseline, rollbackPlanDigest: diff.rollbackPlanDigest
            )))
            case .feedbackPlan: body = .feedbackPlan(feedback)
            }
            return try .init(requestID: requestID, body: body)
        }

        func response(
            _ operation: BridgeOperation, target: DCXTargetReference? = nil,
            snapshotDigest: String? = nil, transactionID: String? = nil,
            baselineDigest: String? = nil, desiredSnapshotDigest: String? = nil,
            desiredProfileDigest: String? = nil, matchesDesired: Bool? = nil
        ) throws -> BridgeResponse {
            let target = target ?? self.target
            let transactionID = transactionID ?? diff.applyPlanDigest
            let baselineDigest = baselineDigest ?? baseline.digest
            let desiredSnapshotDigest = desiredSnapshotDigest ?? desired.digest
            let body: BridgeResponseBody
            switch operation {
            case .helperStatus:
                body = .helperStatus(.init(foreground: true, configured: true, target: target,
                    capabilities: [.helperStatus], coreMIDI: .init(
                        commandsName: "Tinyland DCX Commands", commandsUniqueID: 0x4443_5843,
                        statusName: "Tinyland DCX Status", statusUniqueID: 0x4443_5853, online: false
                    ), activeTransactionID: nil))
            case .identitySearch:
                body = .identitySearch(.init(identity: Self.identity(target), receipt: Self.receipt(operation)))
            case .snapshotCapture:
                body = .snapshotCapture(.init(snapshot: try Self.snapshot(target: target, digest: baseline.digest),
                                              receipt: Self.receipt(operation)))
            case .diffPreview:
                body = .diffPreview(.init(diff: try .init(
                    baselineSnapshotDigest: baselineDigest,
                    desiredProfileDigest: desiredProfileDigest ?? profile.digest,
                    desiredSnapshotDigest: desired.digest, applyPlanDigest: diff.applyPlanDigest,
                    rollbackPlanDigest: diff.rollbackPlanDigest, changes: []
                ), receipt: Self.receipt(operation)))
            case .apply:
                let snapshot = try Self.snapshot(target: target, digest: snapshotDigest ?? desired.digest)
                let exact = snapshot.digest == desiredSnapshotDigest
                body = .apply(.init(transactionID: transactionID, baselineDigest: baselineDigest,
                    desiredSnapshotDigest: desiredSnapshotDigest, readback: snapshot,
                    readbackMatchesDesired: exact, rollbackRequired: !exact, rollbackAvailable: true,
                    receipt: Self.receipt(operation)))
            case .readback:
                body = .readback(.init(transactionID: transactionID, matchesDesired: matchesDesired ?? true,
                    snapshot: try Self.snapshot(target: target, digest: snapshotDigest ?? desired.digest),
                    receipt: Self.receipt(operation)))
            case .rollback:
                let snapshot = try Self.snapshot(target: target, digest: snapshotDigest ?? baseline.digest)
                body = .rollback(.init(transactionID: transactionID, baselineDigest: baselineDigest,
                    restored: snapshot, equalsBaseline: snapshot.digest == baselineDigest,
                    receipt: Self.receipt(operation)))
            case .feedbackPlan:
                body = .feedbackPlan(try Self.feedbackResponse(baselineDigest: baselineDigest))
            }
            return .init(requestID: Self.requestID, body: body)
        }

        func wire(_ response: BridgeResponse) throws -> BridgeResponse {
            try BridgeJSONCodec.decoder().decode(BridgeResponse.self, from: BridgeJSONCodec.encoder().encode(response))
        }

        static func snapshot(target: DCXTargetReference, digest: String) throws -> SnapshotV1 {
            try .init(target: target, identity: identity(target), capturedAt: Date(timeIntervalSince1970: 0),
                digest: digest, complete: true, sectionDigests: .init(
                    identity: Self.digest("1"), dump0: Self.digest("2"), dump1: Self.digest("3")))
        }

        static func identity(_ target: DCXTargetReference) -> DeviceIdentityV1 {
            .init(manufacturer: "Behringer", model: "DCX2496", deviceAddress: target.expectedDeviceAddress,
                  selectedBaud: 38_400, validSearchResponses: 10)
        }

        static func receipt(_ operation: BridgeOperation) -> CommandReceiptV1 {
            .init(operation: operation, exitCode: 0, durationMilliseconds: 1, stdoutDigest: digest("c"))
        }

        static let ringOut = "frequency_hz,level_db\n630,4.0\n"

        /// Synthetic one-notch O4 plan whose v2 profile carries exactly its actions.
        static func feedbackResponse(
            baselineDigest: String, measurementDigest: String = digest("d"),
            profileID: String = "o4-feedback", sourceText: String = ringOut
        ) throws -> FeedbackPlanResponse {
            let plan = try NotchPlanSummaryV1(
                planDigest: digest("e"), baselineSnapshotDigest: baselineDigest,
                measurementDigest: measurementDigest, eqEnabledBefore: false, eqCountBefore: 0,
                operatorBandCount: 0,
                notches: [.init(band: 1, frequencyHz: 630, occurrences: 1, levelDb: 4,
                                frequencyCode: 159, qCode: 40, gainCode: 90, kindCode: 1, slopeCode: 0)],
                dropped: [])
            return .init(
                measurement: try .init(digest: measurementDigest, source: .frequencyList,
                    sourceDigest: FeedbackNotchContract.sourceDigest(sourceText), targetOutput: 4, peakCount: 1),
                plan: plan,
                desired: try .init(profileID: profileID, revision: "synthetic-1", bank: .init(
                    targetOutput: 4, parameterChannel: 8, actions: plan.expectedActions)),
                importReceipt: receipt(.feedbackPlan), planReceipt: receipt(.feedbackPlan),
                profileReceipt: receipt(.feedbackPlan))
        }

        static func digest(_ digit: Character) -> String {
            "sha256/" + String(repeating: String(digit), count: 64)
        }
    }
}
