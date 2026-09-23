import Foundation

/// Sanitized classification of bounded child stderr, not a raw error transcript.
/// Unknown text remains unknown; no path, serial identifier, or captured bytes cross IPC.
public struct ChildFailureDiagnosticV1: Codable, Equatable, Sendable {
    public enum Kind: String, Codable, Sendable {
        case searchIncomplete, timeout, transport, invalidResponse, responseLimit
        case sessionBudget, sessionCleanup, serialSystem, unknown
    }

    public enum Stage: String, Codable, Sendable {
        case search, dump0, dump1, serial, remoteMode, cleanup, unknown
    }

    public let kind: Kind
    public let stage: Stage
    public let exitCode: Int32
    public let durationMilliseconds: UInt64
    public let stderrDigest: String
    public let stderrByteCount: Int
    public let inspectionTruncated: Bool
    public let cleanupFailureReported: Bool
    public let validSearchCount: Int?
    public let searchAttempts: Int?

    public init(
        kind: Kind, stage: Stage, exitCode: Int32, durationMilliseconds: UInt64,
        stderrDigest: String, stderrByteCount: Int, inspectionTruncated: Bool,
        cleanupFailureReported: Bool = false,
        validSearchCount: Int? = nil, searchAttempts: Int? = nil
    ) {
        self.kind = kind
        self.stage = stage
        self.exitCode = exitCode
        self.durationMilliseconds = durationMilliseconds
        self.stderrDigest = stderrDigest
        self.stderrByteCount = stderrByteCount
        self.inspectionTruncated = inspectionTruncated
        self.cleanupFailureReported = cleanupFailureReported
        self.validSearchCount = validSearchCount
        self.searchAttempts = searchAttempts
    }

    public func validate() throws {
        try BridgeDigest.validate(stderrDigest)
        guard exitCode != 0, (0...1_048_576).contains(stderrByteCount),
              inspectionTruncated == (stderrByteCount > 4_096) else {
            throw BridgeMessageError.invalidResponse
        }
        let validStage: Bool
        switch kind {
        case .searchIncomplete: validStage = stage == .search
        case .timeout, .invalidResponse, .responseLimit, .sessionBudget:
            validStage = [Stage.search, .dump0, .dump1].contains(stage)
        case .transport:
            validStage = [Stage.search, .dump0, .dump1, .remoteMode].contains(stage)
        case .sessionCleanup: validStage = stage == .cleanup && cleanupFailureReported
        case .serialSystem: validStage = stage == .serial
        case .unknown: validStage = stage == .unknown && !cleanupFailureReported
        }
        guard validStage, !inspectionTruncated || kind == .unknown else {
            throw BridgeMessageError.invalidResponse
        }
        if kind == .searchIncomplete {
            guard stage == .search, let validSearchCount, let searchAttempts,
                  (0..<10).contains(validSearchCount), searchAttempts == 20 else {
                throw BridgeMessageError.invalidResponse
            }
        } else if validSearchCount != nil || searchAttempts != nil {
            throw BridgeMessageError.invalidResponse
        }
    }

    /// Closed vocabulary only. The AU already displays BridgeErrorPayload.message.
    public var message: String {
        let detail: String
        switch kind {
        case .searchIncomplete:
            detail = "Search: \(validSearchCount ?? 0)/10 valid identities in \(searchAttempts ?? 0) attempts"
        case .timeout: detail = "\(stageLabel): response timed out"
        case .transport: detail = "\(stageLabel): transport failed"
        case .invalidResponse: detail = "\(stageLabel): response validation failed"
        case .responseLimit: detail = "\(stageLabel): response exceeded its limit"
        case .sessionBudget: detail = "\(stageLabel): session time budget exceeded"
        case .sessionCleanup: detail = "Serial cleanup was not verified"
        case .serialSystem: detail = "Serial system operation failed"
        case .unknown: detail = "Unclassified child failure"
        }
        let cleanup = cleanupFailureReported && kind != .sessionCleanup
            ? " Cleanup not verified." : ""
        return "\(detail); exit \(exitCode); \(durationMilliseconds) ms.\(cleanup)"
    }

    private var stageLabel: String {
        switch stage {
        case .search: "Search"
        case .dump0: "Dump0"
        case .dump1: "Dump1"
        case .serial: "Serial"
        case .remoteMode: "Remote mode"
        case .cleanup: "Cleanup"
        case .unknown: "Unknown stage"
        }
    }
}
