import Foundation

/// Metadata only. Private child bytes are never included in status or MIDI.
public struct RoomFailureDiagnosticV1: Codable, Equatable, Sendable {
    public enum Stage: String, Codable, Sendable {
        case admission, validationChild, refreshChild, refreshDecode
        case snapshotChild, snapshotDecode, comparisonChild, comparisonDecode, publication
    }
    public enum Failure: String, Codable, Sendable {
        case childExit, launchFailed, timedOut, outputReadFailed, outputTooLarge
        case terminationUnconfirmed, invalidExpectation, invalidComparison
        case unavailable, busy, decoding, other
    }
    public enum CloseKnown: String, Codable, Sendable { case notApplicable, unknown, verified }
    public let schemaVersion: String
    public let attemptID: UUID
    public let observedAt: Date
    public let stage: Stage
    public let failure: Failure
    public let childExitCode: Int32?
    public let childDurationMilliseconds: UInt64?
    public let stdoutDigest: String?
    public let stdoutByteCount: Int?
    public let stderrDigest: String?
    public let stderrByteCount: Int?
    public let stderrRetainedByteCount: Int
    public let closeKnown: CloseKnown
    public let privateRecord: String?
    public let persistenceUnavailable: Bool

    public init(attemptID: UUID, observedAt: Date, stage: Stage, failure: Failure,
                childExitCode: Int32?, childDurationMilliseconds: UInt64?, stdoutDigest: String?,
                stdoutByteCount: Int?, stderrDigest: String?, stderrByteCount: Int?,
                stderrRetainedByteCount: Int, closeKnown: CloseKnown,
                privateRecord: String?, persistenceUnavailable: Bool) {
        schemaVersion = "dcx.room-failure-diagnostic/v1"
        self.attemptID = attemptID; self.observedAt = observedAt; self.stage = stage
        self.failure = failure; self.childExitCode = childExitCode
        self.childDurationMilliseconds = childDurationMilliseconds; self.stdoutDigest = stdoutDigest
        self.stdoutByteCount = stdoutByteCount; self.stderrDigest = stderrDigest
        self.stderrByteCount = stderrByteCount; self.stderrRetainedByteCount = stderrRetainedByteCount
        self.closeKnown = closeKnown; self.privateRecord = privateRecord
        self.persistenceUnavailable = persistenceUnavailable
    }
}
