import CryptoKit
import DCXLogicBridge
@testable import DCXLogicHelperCore
import Foundation
import XCTest

final class RoomVerificationTests: XCTestCase {
    func testHeartbeatNeverExtendsActualCaptureLifetime() throws {
        let capture = ReceiptCapture()
        let publisher = RoomVerificationPublisher(startHeartbeats: false, submit: capture.append)
        let expectation = Data("SYNTHETIC approved expectation".utf8)
        let at = Date()
        let receipt = try publisher.install(comparison(expectation), expectation: expectation, capturedAt: at, ttyPath: "/dev/cu.usbserial-SYNTHETIC")
        for seconds in stride(from: 0.0, to: 60.0, by: 0.5) {
            let now = at.addingTimeInterval(seconds)
            let renewed = try XCTUnwrap(receipt.renewed(at: now, capturedAt: at, ttyPresent: true))
            let formatter = ISO8601DateFormatter()
            formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            let expiry = try XCTUnwrap(formatter.date(from: renewed.expiresAt))
            XCTAssertLessThanOrEqual(expiry.timeIntervalSince(at), 60.001)
            XCTAssertLessThanOrEqual(expiry.timeIntervalSince(now), 2.001)
            XCTAssertEqual(renewed.verifiedAt, receipt.verifiedAt)
        }
        XCTAssertNil(receipt.renewed(at: at.addingTimeInterval(60), capturedAt: at, ttyPresent: true))
        XCTAssertNil(receipt.renewed(at: at.addingTimeInterval(-1), capturedAt: at, ttyPresent: true))
        XCTAssertNil(receipt.renewed(at: at, capturedAt: at, ttyPresent: false))
        publisher.invalidate("helper-left-foreground")
        let invalidated = try XCTUnwrap(capture.values.last)
        XCTAssertEqual(invalidated.status, "invalidated")
        XCTAssertFalse(invalidated.matches)
        XCTAssertEqual(invalidated.invalidationReason, "helper-left-foreground")
    }

    func testWrongExpectationOrSnapshotBindingCannotPublishMatch() throws {
        let expectation = Data("SYNTHETIC approved expectation".utf8)
        let capture = ReceiptCapture()
        let publisher = RoomVerificationPublisher(startHeartbeats: false, submit: capture.append)
        XCTAssertThrowsError(try publisher.install(comparison(expectation), expectation: Data("different".utf8), capturedAt: Date(), ttyPath: "/dev/cu.usbserial-SYNTHETIC"))
        let wrong = comparison(expectation, expectedSnapshot: "sha256/" + String(repeating: "b", count: 64))
        XCTAssertThrowsError(try publisher.install(wrong, expectation: expectation, capturedAt: Date(), ttyPath: "/dev/cu.usbserial-SYNTHETIC"))
        XCTAssertTrue(capture.values.isEmpty)
    }

    func testExplicitForegroundActionCapturesThenVerifiesAndExitInvalidates() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let capture = ReceiptCapture()
        let publisher = RoomVerificationPublisher(startHeartbeats: false, submit: capture.append)
        let expectation = Data("SYNTHETIC approved expectation".utf8)
        let snapshotURL = try XCTUnwrap(Bundle.module.url(forResource: "SYNTHETIC-blank-snapshot", withExtension: "json", subdirectory: "feedback"))
        let snapshot = try Data(contentsOf: snapshotURL)
        let carrier = try BridgeJSONCodec.decoder().decode(CoreSnapshotCarrierV1.self, from: snapshot)
        let output = comparison(expectation, snapshot: carrier.snapshotDigest)
        let backend = RoomBackend(snapshot: snapshot, comparison: try JSONEncoder().encode(output))
        let configuration = try HelperConfigurationV1(bindingID: "synthetic-room", expectedDeviceAddress: 0, ttyPath: "/dev/cu.usbserial-SYNTHETIC", enabledFeatures: [.snapshotCapture])
        let coordinator = DCXHelperCoordinator(locations: .init(fixtureRoot: root), configuration: .success(configuration), coreMIDI: .init(), runner: backend, roomVerification: publisher)
        XCTAssertThrowsError(try coordinator.verifyRoom(expectation: expectation))
        XCTAssertTrue(backend.arguments.isEmpty)
        coordinator.setForeground(true)
        let receipt = try coordinator.verifyRoom(expectation: expectation)
        XCTAssertTrue(receipt.matches)
        XCTAssertEqual(backend.arguments.count, 3)
        XCTAssertEqual(Array(backend.arguments[0].prefix(2)), ["profile", "room-validate"])
        XCTAssertFalse(backend.arguments[0].contains("--tty"))
        XCTAssertEqual(Array(backend.arguments[1].prefix(2)), ["control", "snapshot"])
        XCTAssertTrue(backend.arguments[1].contains("--tty"))
        XCTAssertEqual(Array(backend.arguments[2].prefix(2)), ["profile", "room-verify"])
        XCTAssertFalse(backend.arguments[2].contains("--tty"))
        coordinator.setForeground(false)
        XCTAssertEqual(capture.values.last?.status, "invalidated")
        XCTAssertFalse(capture.values.last?.matches ?? true)
    }

    private func comparison(_ expectation: Data, snapshot: String = "sha256/" + String(repeating: "a", count: 64), expectedSnapshot: String? = nil) -> RoomComparison {
        .init(schemaVersion: "dcx.room-comparison/v1", sceneId: "practice-ready", sceneRevision: 1,
              compiledDigest: "sha256/" + String(repeating: "c", count: 64),
              presetDigest: "sha256/" + String(repeating: "d", count: 64),
              expectationDigest: "sha256/" + SHA256.hash(data: expectation).map { String(format: "%02x", $0) }.joined(),
              snapshotDigest: snapshot, expectedSnapshotDigest: expectedSnapshot ?? snapshot,
              matches: true, readable: true, comparedFields: 67)
    }
}

private final class ReceiptCapture: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [RoomVerificationReceipt] = []
    var values: [RoomVerificationReceipt] { lock.lock(); defer { lock.unlock() }; return storage }
    func append(_ value: RoomVerificationReceipt) { lock.lock(); storage.append(value); lock.unlock() }
}

private final class RoomBackend: DCXCTLCommandExecuting, @unchecked Sendable {
    private let snapshot: Data
    private let comparison: Data
    var arguments: [[String]] = []
    init(snapshot: Data, comparison: Data) { self.snapshot = snapshot; self.comparison = comparison }
    func resolveExecutable(for configuration: HelperConfigurationV1) throws -> URL { URL(fileURLWithPath: "/SYNTHETIC/dcxctl") }
    func run(_ invocation: DCXCTLInvocation, timeoutSeconds: UInt8, mutationLock: MutationRecoveryProcessLock?) throws -> DCXCTLProcessResult {
        XCTAssertNotNil(mutationLock)
        arguments.append(invocation.arguments)
        let stdout = arguments.count == 1 ? Data("{}".utf8) : arguments.count == 2 ? snapshot : comparison
        return .init(terminationStatus: 0, stdout: stdout, stderr: Data(), durationMilliseconds: 1)
    }
}
