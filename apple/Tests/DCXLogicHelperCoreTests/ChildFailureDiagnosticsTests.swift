import CryptoKit
import DCXLogicBridge
@testable import DCXLogicHelperCore
import Foundation
import XCTest

final class ChildFailureDiagnosticsTests: XCTestCase {
    private func result(_ stderr: String, exit: Int32 = 1) -> DCXCTLProcessResult {
        .init(
            terminationStatus: exit,
            stdout: Data("private stdout /dev/cu.usbserial-PRIVATE".utf8),
            stderr: Data(stderr.utf8), durationMilliseconds: 105_123
        )
    }

    func testSearchQualificationSeparatesZeroAndPartialIdentities() throws {
        for count in [0, 7] {
            let value = result("Error: SearchQualificationIncomplete { valid: \(count), required: 10, attempts: 20, finish_error: None }\n")
            let payload = try XCTUnwrap(value.failurePayload())
            try payload.validate()
            XCTAssertFalse(payload.retryable)
            XCTAssertEqual(payload.failureDiagnostic?.kind, .searchIncomplete)
            XCTAssertEqual(payload.failureDiagnostic?.validSearchCount, count)
            XCTAssertEqual(payload.failureDiagnostic?.searchAttempts, 20)
            XCTAssertTrue(payload.message.contains("\(count)/10 valid identities"))
            XCTAssertTrue(payload.message.contains("exit 1; 105123 ms"))
        }
    }

    func testDumpAndSearchStagesRemainDistinct() throws {
        let cases: [(String, ChildFailureDiagnosticV1.Kind, ChildFailureDiagnosticV1.Stage)] = [
            ("Timeout { operation: Dump0, received: 0, finish_error: None }", .timeout, .dump0),
            ("Validation { operation: Dump1, source: InvalidDumpLength { part: Part1, expected: 911, actual: 910 }, finish_error: None }", .invalidResponse, .dump1),
            ("Transport { operation: Dump0, source: System { stage: Read, errno: 5 }, finish_error: None }", .transport, .dump0),
            ("Validation { operation: Search { sequence: 2 }, source: Protocol(MissingStart(7)), finish_error: None }", .invalidResponse, .search),
            ("BudgetExceeded { operation: Dump1, finish_error: None }", .sessionBudget, .dump1),
            ("ResponseLimit { operation: Dump0, received: 1024, limit: 1015, finish_error: None }", .responseLimit, .dump0),
            ("RemoteMode { mode: ReceiveDirect, source: ShortWrite { written: 3, expected: 10 }, finish_error: None }", .transport, .remoteMode),
            ("System { stage: OpenExclusive, errno: 13 }", .serialSystem, .serial),
            ("Deadline { stage: Configure }", .serialSystem, .serial),
        ]
        for (error, kind, stage) in cases {
            let diagnostic = ChildFailureClassifier.classify(result("Error: \(error)\n"))
            try diagnostic.validate()
            XCTAssertEqual(diagnostic.kind, kind, error)
            XCTAssertEqual(diagnostic.stage, stage, error)
            XCTAssertFalse(diagnostic.cleanupFailureReported, error)
        }
    }

    func testCleanupFailureDoesNotErasePrimaryFailure() throws {
        let cleanup = "Cleanup { primary: None, input_failed: false, termios_failed: true, control_lines_failed: false }"
        let primary = ChildFailureClassifier.classify(result("Error: Timeout { operation: Dump1, received: 0, finish_error: Some(\(cleanup)) }"))
        XCTAssertEqual(primary.kind, .timeout)
        XCTAssertEqual(primary.stage, .dump1)
        XCTAssertTrue(primary.cleanupFailureReported)
        XCTAssertTrue(primary.message.contains("Cleanup not verified"))
        let final = ChildFailureClassifier.classify(result("Error: Finish { source: \(cleanup) }"))
        XCTAssertEqual(final.kind, .sessionCleanup)
        XCTAssertTrue(final.cleanupFailureReported)
    }

    func testUnknownPrivateTextNeverCrossesTheBridge() throws {
        let privateErrors = [
            "Error: /dev/cu.usbserial-PRIVATE /Users/private/secret.json",
            "Error: Timeout { operation: Dump0, received: 0, finish_error: None }\n/private/secret",
            "Error: Transport { operation: Dump0, source: Binding(\"/dev/cu.usbserial-PRIVATE\"), finish_error: None }",
            "Error: Timeout { operation: Dump0, source: \"serial-PRIVATE\", finish_error: None }",
            "Error: SearchQualificationIncomplete { valid: 10, required: 10, attempts: 20, finish_error: None }",
            "Error: SearchQualificationIncomplete { valid: 9999999999999, required: 10, attempts: 20, finish_error: None }",
            "Error: persistent snapshot timed out with 0 bytes at Dump0",
        ]
        for stderr in privateErrors {
            let payload = try XCTUnwrap(result(stderr).failurePayload())
            XCTAssertEqual(payload.failureDiagnostic?.kind, .unknown, stderr)
            let response = BridgeResponse(requestID: "test", operation: .snapshotCapture, error: payload)
            let encoded = String(decoding: try BridgeJSONCodec.encoder().encode(response), as: UTF8.self)
            for secret in ["PRIVATE", "/dev/", "/Users/", "/private/", "private stdout"] {
                XCTAssertFalse(encoded.contains(secret), encoded)
            }
            let decoded = try BridgeJSONCodec.decoder().decode(BridgeResponse.self, from: Data(encoded.utf8))
            XCTAssertEqual(decoded.error, payload)
        }
    }

    func testTruncationAndInvalidUTF8RemainUnknownWithWholeInputDigest() throws {
        let stderr = Data(("Error: Timeout { operation: Dump0, received: 0, finish_error: None }" + String(repeating: "x", count: 4_096)).utf8)
        let value = DCXCTLProcessResult(terminationStatus: 1, stdout: Data(), stderr: stderr, durationMilliseconds: 3)
        let diagnostic = ChildFailureClassifier.classify(value)
        XCTAssertEqual(diagnostic.kind, .unknown)
        XCTAssertTrue(diagnostic.inspectionTruncated)
        XCTAssertEqual(diagnostic.stderrByteCount, stderr.count)
        let digest = SHA256.hash(data: stderr).map { String(format: "%02x", $0) }.joined()
        XCTAssertEqual(diagnostic.stderrDigest, "sha256/\(digest)")
        try diagnostic.validate()
        let invalid = DCXCTLProcessResult(terminationStatus: 1, stdout: Data(), stderr: Data([0xff]), durationMilliseconds: 3)
        XCTAssertEqual(ChildFailureClassifier.classify(invalid).kind, .unknown)
    }

    func testSuccessKeepsExistingReceiptAndDoesNotClassifyStderr() {
        let success = result("Error: Timeout { operation: Dump0, received: 0, finish_error: None }", exit: 0)
        XCTAssertNil(success.failurePayload())
        let receipt = success.receipt(for: .snapshotCapture)
        XCTAssertEqual(receipt.exitCode, 0)
        XCTAssertEqual(receipt.durationMilliseconds, 105_123)
        XCTAssertEqual(receipt.operation, .snapshotCapture)
        let digest = SHA256.hash(data: success.stdout).map { String(format: "%02x", $0) }.joined()
        XCTAssertEqual(receipt.stdoutDigest, "sha256/\(digest)")
    }

    func testPriorErrorResponseStillDecodesWithoutDiagnostic() throws {
        let json = #"{"schemaVersion":"dcx.logic-bridge/v1","requestID":"old","operation":"device.snapshot.capture","status":"error","error":{"code":"child_failed","message":"dcxctl exited with status 1","retryable":false}}"#
        let response = try BridgeJSONCodec.decoder().decode(BridgeResponse.self, from: Data(json.utf8))
        XCTAssertNil(response.error?.failureDiagnostic)
    }

    func testInvalidDiagnosticIsRejectedAtBridgeBoundary() throws {
        let digest = "sha256/" + String(repeating: "0", count: 64)
        let invalid = ChildFailureDiagnosticV1(
            kind: .searchIncomplete, stage: .dump0, exitCode: 1,
            durationMilliseconds: 2, stderrDigest: digest,
            stderrByteCount: 0, inspectionTruncated: false,
            validSearchCount: 0, searchAttempts: 20
        )
        let response = BridgeResponse(
            requestID: "invalid", operation: .snapshotCapture,
            error: .init(code: .childFailed, message: "bounded", retryable: false, failureDiagnostic: invalid)
        )
        let data = try BridgeJSONCodec.encoder().encode(response)
        XCTAssertThrowsError(try BridgeJSONCodec.decoder().decode(BridgeResponse.self, from: data))
        let incomplete = ChildFailureDiagnosticV1(
            kind: .searchIncomplete, stage: .search, exitCode: 1,
            durationMilliseconds: 2, stderrDigest: digest,
            stderrByteCount: 0, inspectionTruncated: false,
            validSearchCount: 9, searchAttempts: 1
        )
        XCTAssertThrowsError(try incomplete.validate())
        let valid = ChildFailureClassifier.classify(result("Error: System { stage: Read, errno: 5 }"))
        let diagnosticJSON = String(decoding: try BridgeJSONCodec.encoder().encode(valid), as: UTF8.self)
        let overflowJSON = diagnosticJSON.replacingOccurrences(
            of: "\"durationMilliseconds\":105123",
            with: "\"durationMilliseconds\":18446744073709551616"
        )
        XCTAssertNotEqual(overflowJSON, diagnosticJSON)
        XCTAssertThrowsError(try BridgeJSONCodec.decoder().decode(
            ChildFailureDiagnosticV1.self, from: Data(overflowJSON.utf8)
        ))
    }
}
