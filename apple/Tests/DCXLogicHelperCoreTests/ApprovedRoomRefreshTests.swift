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

    func testPeriodicReadRetainsOriginalFreshProofUntilAtomicReplacement() throws {
        try exerciseRetention { coordinator, publisher, gate, backend, manifest, configuration, first in
            backend.beforeRefresh = {
                XCTAssertTrue(gate.isInFlight)
                XCTAssertEqual(publisher.current(expectation: manifest)?.verifiedAt, first.verifiedAt)
            }
            let second = try coordinator.verifyBoundRoom(expectation: manifest, forceFresh: true,
                expectedConfiguration: configuration, approvedTargetRefresh: true, retainFreshDuringRead: true,
                bindingIsCurrent: { true })
            XCTAssertEqual(publisher.current(expectation: manifest)?.verifiedAt, second.verifiedAt)
            XCTAssertTrue(second.matches)
        }
    }

    func testPeriodicReadFailureImmediatelyInvalidatesRetainedProof() throws {
        try exerciseRetention { coordinator, publisher, _, backend, manifest, configuration, first in
            backend.beforeRefresh = {
                XCTAssertEqual(publisher.current(expectation: manifest)?.verifiedAt, first.verifiedAt)
                throw RoomVerificationError.unavailable
            }
            XCTAssertThrowsError(try coordinator.verifyBoundRoom(expectation: manifest, forceFresh: true,
                expectedConfiguration: configuration, approvedTargetRefresh: true, retainFreshDuringRead: true,
                bindingIsCurrent: { true }))
            XCTAssertNil(publisher.current(expectation: manifest))
        }
    }

    func testPeriodicReadCannotRetainExpiredProofOrDifferentBinding() throws {
        let publisher = RoomVerificationPublisher(startHeartbeats: false, submit: { _ in }, ttyPresent: { _ in true })
        let manifest = Data("SYNTHETIC retention age".utf8)
        let value = try decode(fixture(expectation: manifest))
        let captured = Date()
        let receipt = try publisher.install(value.comparison, expectation: manifest, capturedAt: captured,
            ttyPath: "/SYNTHETIC", captureBinding: Data([1]))
        XCTAssertTrue(publisher.canRetainDuringRead(binding: Data([1]), now: captured.addingTimeInterval(59)))
        XCTAssertFalse(publisher.canRetainDuringRead(binding: Data([2]), now: captured))
        XCTAssertFalse(publisher.canRetainDuringRead(binding: Data([1]), now: captured.addingTimeInterval(60)))
        XCTAssertNil(publisher.current(expectation: manifest, now: captured.addingTimeInterval(60)))
        XCTAssertEqual(publisher.current(expectation: manifest, now: captured)?.verifiedAt, receipt.verifiedAt)
    }

    func testExplicitFreshReadStillInvalidatesBeforeCapture() throws {
        try exerciseRetention { coordinator, publisher, _, backend, manifest, configuration, _ in
            backend.beforeRefresh = { XCTAssertNil(publisher.current(expectation: manifest)) }
            _ = try coordinator.verifyBoundRoom(expectation: manifest, forceFresh: true,
                expectedConfiguration: configuration, approvedTargetRefresh: true, bindingIsCurrent: { true })
        }
    }

    func testForegroundCancellationDuringRetainedReadCannotRepublish() throws {
        try exerciseRetention { coordinator, publisher, _, backend, manifest, configuration, _ in
            backend.beforeRefresh = {
                coordinator.setForeground(false)
                XCTAssertNil(publisher.current(expectation: manifest))
            }
            XCTAssertThrowsError(try coordinator.verifyBoundRoom(expectation: manifest, forceFresh: true,
                expectedConfiguration: configuration, approvedTargetRefresh: true, retainFreshDuringRead: true,
                bindingIsCurrent: { true }))
            XCTAssertNil(publisher.current(expectation: manifest))
        }
    }

    func testBindingChangeDuringRetainedReadPreventsPublicationAndHeartbeat() throws {
        let allowed = RetentionFlag()
        try exerciseRetention(initialBinding: { allowed.value }) { coordinator, publisher, _, backend, manifest, configuration, _ in
            backend.beforeRefresh = {
                allowed.disable()
                XCTAssertNil(publisher.current(expectation: manifest))
            }
            XCTAssertThrowsError(try coordinator.verifyBoundRoom(expectation: manifest, forceFresh: true,
                expectedConfiguration: configuration, approvedTargetRefresh: true, retainFreshDuringRead: true,
                bindingIsCurrent: { allowed.value }))
            XCTAssertNil(publisher.current(expectation: manifest))
        }
    }

    func testMutatingAuthorityInvalidationDuringReadCannotRestoreOldProof() throws {
        try exerciseRetention { coordinator, publisher, _, backend, manifest, configuration, _ in
            backend.beforeRefresh = {
                coordinator.invalidateApprovedRoomBinding()
                XCTAssertNil(publisher.current(expectation: manifest))
            }
            XCTAssertThrowsError(try coordinator.verifyBoundRoom(expectation: manifest, forceFresh: true,
                expectedConfiguration: configuration, approvedTargetRefresh: true, retainFreshDuringRead: true,
                bindingIsCurrent: { true }))
            XCTAssertNil(publisher.current(expectation: manifest))
        }
    }

    func testDriftAtomicallyReplacesPriorMatchWithUnavailableReadiness() throws {
        let publisher = RoomVerificationPublisher(startHeartbeats: false, submit: { _ in }, ttyPresent: { _ in true })
        let manifest = Data("SYNTHETIC drift replacement".utf8)
        var object = try fixture(expectation: manifest, capturedAt: Date())
        let good = try decode(object)
        _ = try publisher.install(good.comparison, expectation: manifest, capturedAt: Date(),
            ttyPath: "/SYNTHETIC", captureBinding: Data([1]))
        var comparison = try XCTUnwrap(object["comparison"] as? [String: Any])
        comparison["matches"] = false
        object["comparison"] = comparison
        let drift = try decode(object)
        let receipt = try publisher.install(drift.comparison, expectation: manifest, capturedAt: Date(),
            ttyPath: "/SYNTHETIC", captureBinding: Data([1]))
        XCTAssertEqual(receipt.status, "drift")
        XCTAssertFalse(try XCTUnwrap(publisher.current(expectation: manifest)).matches)
        XCTAssertFalse(publisher.canRetainDuringRead(binding: Data([1])))
    }

    private func exerciseRetention(initialBinding: @escaping @Sendable () -> Bool = { true },
        _ body: (DCXHelperCoordinator, RoomVerificationPublisher, RoomVerificationPreparation, FastBackend,
                 Data, HelperConfigurationV1, RoomVerificationReceipt) throws -> Void) throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let manifest = Data("SYNTHETIC retained periodic approval".utf8)
        let backend = FastBackend { try self.fixture(expectation: manifest, capturedAt: Date()) }
        let publisher = RoomVerificationPublisher(startHeartbeats: false, submit: { _ in }, ttyPresent: { _ in true })
        let gate = RoomVerificationPreparation()
        let configuration = try HelperConfigurationV1(bindingID: "synthetic-room", expectedDeviceAddress: 0,
            ttyPath: "/dev/cu.usbserial-SYNTHETIC", enabledFeatures: [.snapshotCapture])
        let coordinator = DCXHelperCoordinator(locations: .init(fixtureRoot: root), configuration: .success(configuration),
            coreMIDI: .init(), runner: backend, roomVerification: publisher, roomPreparation: gate)
        coordinator.setForeground(true)
        let first = try coordinator.verifyBoundRoom(expectation: manifest, forceFresh: true,
            expectedConfiguration: configuration, approvedTargetRefresh: true, bindingIsCurrent: initialBinding)
        try body(coordinator, publisher, gate, backend, manifest, configuration, first)
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
    var beforeRefresh: (() throws -> Void)?
    let envelope: () throws -> [String: Any]
    init(_ envelope: @escaping () throws -> [String: Any]) { self.envelope = envelope }
    func resolveExecutable(for configuration: HelperConfigurationV1) throws -> URL { URL(fileURLWithPath: "/SYNTHETIC/dcxctl") }
    func run(_ invocation: DCXCTLInvocation, timeoutSeconds: UInt8, mutationLock: MutationRecoveryProcessLock?) throws -> DCXCTLProcessResult {
        XCTAssertNotNil(mutationLock)
        arguments.append(invocation.arguments)
        let object: Any
        if invocation.arguments.starts(with: ["profile", "room-refresh"]) { try beforeRefresh?(); object = try envelope() }
        else if invocation.arguments.starts(with: ["control", "snapshot"]) { object = try XCTUnwrap(envelope()["snapshot"]) }
        else if invocation.arguments.starts(with: ["profile", "room-verify"]) { object = try XCTUnwrap(envelope()["comparison"]) }
        else { object = [String: Any]() }
        return .init(terminationStatus: 0, stdout: try JSONSerialization.data(withJSONObject: object), stderr: Data(), durationMilliseconds: 1)
    }
}

private final class RetentionFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var enabled = true
    var value: Bool { lock.lock(); defer { lock.unlock() }; return enabled }
    func disable() { lock.lock(); enabled = false; lock.unlock() }
}
