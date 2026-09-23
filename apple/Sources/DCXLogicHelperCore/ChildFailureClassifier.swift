import CryptoKit
import DCXLogicBridge
import Foundation

/// Allowlist the Rust Debug output emitted by main's Result termination at
/// source 77a0a2d. Require one complete bounded line; never return matched text.
enum ChildFailureClassifier {
    static let inspectionLimit = 4_096

    static func classify(_ result: DCXCTLProcessResult) -> ChildFailureDiagnosticV1 {
        let digest = SHA256.hash(data: result.stderr).map { String(format: "%02x", $0) }.joined()
        // Truncated, malformed, multiline, or changed formats remain unknown.
        let text = result.stderr.count <= inspectionLimit
            ? String(data: result.stderr, encoding: .utf8) ?? "" : ""
        var kind = ChildFailureDiagnosticV1.Kind.unknown
        var stage = ChildFailureDiagnosticV1.Stage.unknown
        var valid: Int?
        var attempts: Int?
        var cleanupFailed = false

        if matches("Finish \\{ source: \(carrier) \\}", text)
            || matches(cleanup, text) {
            kind = .sessionCleanup
            stage = .cleanup
            cleanupFailed = true
        } else if let fields = captures(
            "SearchQualificationIncomplete \\{ valid: ([0-9]{1,2}), required: 10, attempts: (20), finish_error: (\(finish)) \\}",
            text
        ), let count = Int(fields[0]), let trials = Int(fields[1]),
           (0..<10).contains(count), (1...20).contains(trials), count <= trials {
            kind = .searchIncomplete
            stage = .search
            valid = count
            attempts = trials
            cleanupFailed = fields[2] != "None"
        } else if let fields = captures(
            "(BudgetExceeded|Transport|Timeout|ResponseLimit|Validation) \\{ operation: (\(operation)), (\(operationDetail))finish_error: (\(finish)) \\}",
            text
        ), detailMatches(kind: fields[0], detail: fields[2]) {
            switch fields[0] {
            case "BudgetExceeded": kind = .sessionBudget
            case "Transport": kind = .transport
            case "Timeout": kind = .timeout
            case "ResponseLimit": kind = .responseLimit
            case "Validation": kind = .invalidResponse
            default: break
            }
            stage = fields[1] == "Dump0" ? .dump0 : fields[1] == "Dump1" ? .dump1 : .search
            cleanupFailed = fields[3] != "None"
        } else if let fields = captures(
            "RemoteMode \\{ mode: (?:Transmit|ReceiveDirect), source: \(carrier), finish_error: (\(finish)) \\}", text
        ) {
            kind = .transport
            stage = .remoteMode
            cleanupFailed = fields[0] != "None"
        } else if matches(carrier, text) {
            kind = .serialSystem
            stage = .serial
        }

        return .init(
            kind: kind, stage: stage, exitCode: result.terminationStatus,
            durationMilliseconds: result.durationMilliseconds,
            stderrDigest: "sha256/\(digest)", stderrByteCount: result.stderr.count,
            inspectionTruncated: result.stderr.count > inspectionLimit,
            cleanupFailureReported: cleanupFailed,
            validSearchCount: valid, searchAttempts: attempts
        )
    }

    private static let number = #"[0-9]{1,7}"#
    private static let stage = #"(?:VerifyOperation|VerifyBinding|OpenExclusive|SnapshotTermios|SnapshotControlLines|Configure|CheckPreexistingInput|Write|BytesAvailable|WaitReadable|Read|DiscardInput|RestoreTermios|VerifyTermiosRestore|RestoreControlLines|VerifyControlLinesRestore|Close)"#
    private static let cleanup = #"Cleanup \{ primary: (?:None|Some\((?:Operation|Binding|Deadline|System|ShortWrite|PreexistingInput|Overflow|EndOfFile|ReadInvariant)\)), input_failed: (?:true|false), termios_failed: (?:true|false), control_lines_failed: (?:true|false) \}"#
    private static let carrier = "(?:System \\{ stage: \(stage), errno: -?[0-9]{1,10} \\}|Deadline \\{ stage: \(stage) \\}|PreexistingInput \\{ queued: \(number) \\}|ShortWrite \\{ written: \(number), expected: \(number) \\}|Overflow \\{ received: \(number), queued: \(number) \\}|(?:EndOfFile|UnexpectedTrailingFrame) \\{ received: \(number) \\}|InputRecoveryRequired|\(cleanup))"
    private static let finish = "(?:None|Some\\(\(carrier)\\))"
    private static let operation = #"(?:Search \{ sequence: (?:[1-9]|1[0-9]|20) \}|Dump0|Dump1)"#
    private static let validation = "(?:InvalidDumpLength \\{ part: Part[01], expected: \(number), actual: \(number) \\}|WrongDumpPart \\{ expected: Part[01], actual: Part[01] \\}|DeviceMismatch \\{ section: (?:Identity|Dump0|Dump1), expected: [0-9]{1,2}, actual: [0-9]{1,2} \\}|UnexpectedMessage \\{ section: (?:Identity|Dump0|Dump1), kind: \"non_dump_response\" \\}|Protocol\\(\(protocolError)\\))"
    private static let protocolError = "(?:(?:FrameTooShort|FrameTooLong|MissingStart|MissingTerminator|WrongModel|InvalidDeviceId|InvalidSearchResponseLength|UnexpectedSearchResponseFunction|MalformedDump|InvalidDumpPart)\\(\(number)\\)|BroadcastSearchResponse|WrongManufacturer\\(\\[\(number), \(number), \(number)\\]\\)|NonSevenBitData \\{ index: \(number), value: \(number) \\})"
    private static let operationDetail = "(?:source: (?:\(carrier)|\(validation)), |received: \(number)(?:, limit: \(number))?, )?"

    private static func detailMatches(kind: String, detail: String) -> Bool {
        switch kind {
        case "BudgetExceeded": detail.isEmpty
        case "Transport": wholeMatch("source: \(carrier), ", detail)
        case "Validation": wholeMatch("source: \(validation), ", detail)
        case "Timeout": wholeMatch("received: \(number), ", detail)
        case "ResponseLimit": wholeMatch("received: \(number), limit: \(number), ", detail)
        default: false
        }
    }

    private static func wholeMatch(_ pattern: String, _ text: String) -> Bool {
        text.range(of: "\\A\(pattern)\\z", options: .regularExpression) != nil
    }

    private static func matches(_ pattern: String, _ text: String) -> Bool {
        captures(pattern, text) != nil
    }

    private static func captures(_ pattern: String, _ text: String) -> [String]? {
        guard let expression = try? NSRegularExpression(pattern: "\\AError: \(pattern)\\n?\\z"),
              let match = expression.firstMatch(in: text, range: NSRange(text.startIndex..., in: text)) else {
            return nil
        }
        return (1..<match.numberOfRanges).compactMap {
            Range(match.range(at: $0), in: text).map { String(text[$0]) }
        }
    }
}

extension DCXCTLProcessResult {
    /// Nil preserves the existing successful decode/receipt path exactly.
    func failurePayload() -> BridgeErrorPayload? {
        guard terminationStatus != 0 else { return nil }
        let diagnostic = ChildFailureClassifier.classify(self)
        return .init(
            code: .childFailed, message: diagnostic.message, retryable: false,
            failureDiagnostic: diagnostic
        )
    }
}
