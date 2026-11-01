import CryptoKit
import DCXLogicBridge
@testable import DCXLogicHelperCore
import Foundation
import XCTest

final class ApprovedRoomRefreshTests: XCTestCase {
    private let at = Date(timeIntervalSince1970: 1_000)

    func testFullBulkEnvelopeRetainsActualCaptureTimeAndMeasuredAttempts() throws {
        let value = try decode(fixture())
        let date = try value.validate(expectedDevice: 0, terminationStatus: 0, startedAt: at.addingTimeInterval(-10), now: at.addingTimeInterval(2))
        XCTAssertEqual(date, at)
        XCTAssertEqual(value.metrics.searchAttempts, 2)
        XCTAssertEqual(value.metrics.searchEmptyTimeouts, 1)
        XCTAssertTrue(value.metrics.closeVerified)
    }

    func testOldFutureUnclosedOrInconsistentEnvelopeCannotBecomeReadiness() throws {
        let changes: [(inout [String: Any]) -> Void] = [
            { $0["captureCompletedAtUnixMilliseconds"] = 1_003_000 },
            { $0["captureCompletedAtUnixMilliseconds"] = 900_000 },
            { $0["closeVerified"] = false },
            { $0["searchAttempts"] = 4 },
            { $0["searchEmptyTimeouts"] = UInt32.max },
            { $0["validSearchCount"] = 0 },
            { $0["captureElapsedMilliseconds"] = 20_001 },
            { $0["status"] = "drift" },
            { $0["identityPolicy"] = "unknown" },
        ]
        for change in changes {
            var object = try fixture(); change(&object)
            XCTAssertThrowsError(try decode(object).validate(expectedDevice: 0, terminationStatus: 0,
                startedAt: at.addingTimeInterval(-10), now: at.addingTimeInterval(2)))
        }
        var failed = try fixture(); failed["snapshot"] = NSNull(); failed["status"] = "failed"
        XCTAssertThrowsError(try decode(failed))
    }

    func testProtectedHelperPathSuppliesExactExpectationDigestAndNeverGenericSnapshotOrWrites() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let manifest = Data("SYNTHETIC fast saved approval".utf8)
        let backend = FastBackend { try self.fixture(expectation: manifest, capturedAt: Date()) }
        let configuration = try HelperConfigurationV1(bindingID: "synthetic-room", expectedDeviceAddress: 0,
            ttyPath: "/dev/cu.usbserial-SYNTHETIC", enabledFeatures: [.snapshotCapture])
        let coordinator = DCXHelperCoordinator(locations: .init(fixtureRoot: root), configuration: .success(configuration),
            coreMIDI: .init(), runner: backend, roomVerification: .init(startHeartbeats: false, submit: { _ in }))
        coordinator.setForeground(true)
        let value = try coordinator.verifyBoundRoom(expectation: manifest, forceFresh: true,
            expectedConfiguration: configuration, approvedTargetRefresh: true, bindingIsCurrent: { true })
        XCTAssertTrue(value.matches)
        XCTAssertNotNil(value.captureMetrics)
        XCTAssertEqual(backend.arguments.count, 2)
        let argv = try XCTUnwrap(backend.arguments.last)
        XCTAssertEqual(Array(argv.prefix(2)), ["profile", "room-refresh"])
        let index = try XCTUnwrap(argv.firstIndex(of: "--expectation-digest"))
        XCTAssertEqual(argv[index + 1], "sha256/" + SHA256.hash(data: manifest).map { String(format: "%02x", $0) }.joined())
        XCTAssertFalse(backend.arguments.contains { $0.contains("snapshot") || $0.contains("apply") || $0.contains("rollback") })
    }

    func testDifferentIdentityPoliciesCannotReuseEachOthersCompletedCapture() throws {
        for fastFirst in [true, false] {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            defer { try? FileManager.default.removeItem(at: root) }
            let locations = DCXHelperStorageLocations(fixtureRoot: root)
            let manifest = Data("SYNTHETIC policy separation".utf8)
            let backend = FastBackend { try self.fixture(expectation: manifest, capturedAt: Date()) }
            let configuration = try HelperConfigurationV1(bindingID: "synthetic-room", expectedDeviceAddress: 0,
                ttyPath: "/dev/cu.usbserial-SYNTHETIC", enabledFeatures: [.snapshotCapture])
            try configuration.save(to: locations.helperConfigurationURL)
            let coordinator = DCXHelperCoordinator(locations: locations, configuration: .success(configuration),
                coreMIDI: .init(), runner: backend,
                roomVerification: .init(startHeartbeats: false, submit: { _ in }, ttyPresent: { _ in true }))
            coordinator.setForeground(true)
            for fast in [fastFirst, !fastFirst] {
                let value = try coordinator.verifyBoundRoom(expectation: manifest, forceFresh: false,
                    expectedConfiguration: configuration, approvedTargetRefresh: fast, bindingIsCurrent: { true })
                XCTAssertEqual(value.captureMetrics != nil, fast)
            }
            XCTAssertEqual(backend.arguments.filter { $0.starts(with: ["control", "snapshot"]) }.count, 1)
            XCTAssertEqual(backend.arguments.filter { $0.starts(with: ["profile", "room-refresh"]) }.count, 1)
        }
    }

    private func decode(_ object: [String: Any]) throws -> ApprovedRoomRefresh {
        try BridgeJSONCodec.decoder().decode(ApprovedRoomRefresh.self, from: JSONSerialization.data(withJSONObject: object))
    }

    private func fixture(expectation: Data = Data("SYNTHETIC fast approved".utf8), capturedAt: Date? = nil) throws -> [String: Any] {
        let url = try XCTUnwrap(Bundle.module.url(forResource: "SYNTHETIC-blank-snapshot", withExtension: "json", subdirectory: "feedback"))
        let data = try Data(contentsOf: url)
        let snapshot = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let digest = try XCTUnwrap(snapshot["snapshot_digest"] as? String)
        return ["schemaVersion": "dcx.approved-room-refresh/v1", "status": "match", "snapshot": snapshot,
            "comparison": ["schemaVersion": "dcx.room-comparison/v1", "sceneId": "practice-ready", "sceneRevision": 1,
                "compiledDigest": "sha256/" + String(repeating: "b", count: 64),
                "presetDigest": "sha256/" + String(repeating: "c", count: 64),
                "expectationDigest": "sha256/" + SHA256.hash(data: expectation).map { String(format: "%02x", $0) }.joined(),
                "snapshotDigest": digest, "expectedSnapshotDigest": digest, "matches": true, "readable": true, "comparedFields": 67],
            "captureCompletedAtUnixMilliseconds": UInt64(((capturedAt ?? at).timeIntervalSince1970 * 1000).rounded(.down)),
            "captureElapsedMilliseconds": 5_100, "comparisonElapsedMilliseconds": 1,
            "searchAttempts": 2, "searchEmptyTimeouts": 1, "validSearchCount": 1,
            "closeVerified": true, "identityPolicy": "approved-exact-target-one-identity-three-attempts"]
    }
}

private final class FastBackend: DCXCTLCommandExecuting, @unchecked Sendable {
    var arguments: [[String]] = []
    let envelope: () throws -> [String: Any]
    init(_ envelope: @escaping () throws -> [String: Any]) { self.envelope = envelope }
    func resolveExecutable(for configuration: HelperConfigurationV1) throws -> URL { URL(fileURLWithPath: "/SYNTHETIC/dcxctl") }
    func run(_ invocation: DCXCTLInvocation, timeoutSeconds: UInt8, mutationLock: MutationRecoveryProcessLock?) throws -> DCXCTLProcessResult {
        XCTAssertNotNil(mutationLock)
        arguments.append(invocation.arguments)
        let object: Any
        if invocation.arguments.starts(with: ["profile", "room-refresh"]) { object = try envelope() }
        else if invocation.arguments.starts(with: ["control", "snapshot"]) { object = try XCTUnwrap(envelope()["snapshot"]) }
        else if invocation.arguments.starts(with: ["profile", "room-verify"]) { object = try XCTUnwrap(envelope()["comparison"]) }
        else { object = [String: Any]() }
        return .init(terminationStatus: 0, stdout: try JSONSerialization.data(withJSONObject: object), stderr: Data(), durationMilliseconds: 1)
    }
}
