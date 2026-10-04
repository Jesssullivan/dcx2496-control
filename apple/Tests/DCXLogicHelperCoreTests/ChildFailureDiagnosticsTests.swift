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

    func testIdentitySearchUsesItsActualRustFailureFormat() throws {
        // live_discovery.rs emits this distinct Debug format, rather than
        // snapshot.rs's SearchQualificationIncomplete variant.
        for count in [0, 7, 9] {
            let value = result("Error: LiveSearchFailed { phase: \"qualification_incomplete\", detail: \"\(count) of 10 required Search identities were valid\" }\n")
            let payload = try XCTUnwrap(value.failurePayload())
            try payload.validate()
            XCTAssertEqual(payload.failureDiagnostic?.kind, .searchIncomplete)
            XCTAssertEqual(payload.failureDiagnostic?.stage, .search)
            XCTAssertEqual(payload.failureDiagnostic?.validSearchCount, count)
            XCTAssertEqual(payload.failureDiagnostic?.searchAttempts, 20)
            XCTAssertFalse(payload.retryable)
            XCTAssertFalse(payload.failureDiagnostic?.cleanupFailureReported ?? true)
            let response = BridgeResponse(requestID: "identity", operation: .identitySearch, error: payload)
            let data = try BridgeJSONCodec.encoder().encode(response)
            let decoded = try BridgeJSONCodec.decoder().decode(BridgeResponse.self, from: data)
            XCTAssertEqual(decoded.error, payload)
            XCTAssertFalse(String(decoding: data, as: UTF8.self).contains("private stdout"))
        }
    }

    func testTransactionCaptureEnvelopePreservesSnapshotDiagnostic() throws {
        let cleanup = "Cleanup { primary: None, input_failed: false, termios_failed: true, control_lines_failed: false }"
        let cases = [
            "Timeout { operation: Dump0, received: 0, finish_error: None }",
            "SearchQualificationIncomplete { valid: 7, required: 10, attempts: 20, finish_error: None }",
            "Timeout { operation: Dump1, received: 0, finish_error: Some(\(cleanup)) }",
            "Finish { source: \(cleanup) }",
            "Validation { operation: Dump1, source: InvalidDumpLength { part: Part1, expected: 911, actual: 910 }, finish_error: None }",
            "RemoteMode { mode: ReceiveDirect, source: ShortWrite { written: 3, expected: 10 }, finish_error: None }",
        ]
        for error in cases {
            let bare = ChildFailureClassifier.classify(result("Error: \(error)\n"))
            for newline in ["", "\n"] {
                let value = result("Error: Capture(\(error))\(newline)")
                let wrapped = ChildFailureClassifier.classify(value)
                try wrapped.validate()
                XCTAssertNotEqual(wrapped.kind, .unknown, error)
                XCTAssertEqual(wrapped.kind, bare.kind, error)
                XCTAssertEqual(wrapped.stage, bare.stage, error)
                XCTAssertEqual(wrapped.validSearchCount, bare.validSearchCount, error)
                XCTAssertEqual(wrapped.searchAttempts, bare.searchAttempts, error)
                XCTAssertEqual(wrapped.cleanupFailureReported, bare.cleanupFailureReported, error)
                XCTAssertEqual(wrapped.message, bare.message, error)
                // Envelope removal affects inspection only, never the receipt.
                let digest = SHA256.hash(data: value.stderr).map { String(format: "%02x", $0) }.joined()
                XCTAssertEqual(wrapped.stderrDigest, "sha256/\(digest)")
                XCTAssertEqual(wrapped.stderrByteCount, value.stderr.count)
                XCTAssertNotEqual(wrapped.stderrDigest, bare.stderrDigest)
            }
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
            "Error: Capture(Capture(Timeout { operation: Dump0, received: 0, finish_error: None }))",
            "Error: Capture(Timeout { operation: Dump0, received: 0, finish_error: None }) /private/secret",
            "Error: Capture(Timeout { operation: Dump0, received: 0, finish_error: None }\n/private/secret)",
            "Error: Capture(System { stage: OpenExclusive, errno: 13 })",
            "Error: Capture(Transport { operation: Dump0, source: Binding(\"/dev/cu.usbserial-PRIVATE\"), finish_error: None })",
            "Error: LiveSearchFailed { phase: \"qualification_incomplete\", detail: \"10 of 10 required Search identities were valid\" }",
            "Error: LiveSearchFailed { phase: \"qualification_incomplete\", detail: \"07 of 10 required Search identities were valid\" }",
            "Error: LiveSearchFailed { phase: \"qualification_search\", detail: \"7 of 10 required Search identities were valid\" }",
            "Error: LiveSearchFailed { phase: \"qualification_incomplete\", detail: \"7 of 10 required Search identities were valid /private/secret\" }",
            "Error: Capture(LiveSearchFailed { phase: \"qualification_incomplete\", detail: \"7 of 10 required Search identities were valid\" })",
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
