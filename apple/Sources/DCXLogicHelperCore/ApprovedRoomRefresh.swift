import Foundation
import DCXLogicBridge

/// TIN-6964: stale Search replies the carrier discarded ahead of each dump
/// frame. Producers before TIN-6964, including the build 14 helper at 9ff6e73,
/// omit the field, so it is optional.
/// The carrier discards at most one per dump window.
public struct StaleSearchRepliesDiscarded: Codable, Equatable, Sendable {
    public static let perDumpLimit: UInt32 = 1
    public let dump0: UInt32
    public let dump1: UInt32

    var isWithinBound: Bool {
        dump0 <= Self.perDumpLimit && dump1 <= Self.perDumpLimit
    }
}

public struct RoomCaptureMetrics: Codable, Equatable, Sendable {
    public let identityPolicy: String
    public let searchAttempts: UInt32
    public let searchEmptyTimeouts: UInt32
    public let validSearchCount: UInt32
    public let captureElapsedMilliseconds: UInt64
    public let comparisonElapsedMilliseconds: UInt64
    public let closeVerified: Bool
    public let staleSearchRepliesDiscarded: StaleSearchRepliesDiscarded?
}

/// Produced only by the protected approved-target CLI. A failed/null carrier
/// cannot decode as a complete fresh capture.
struct ApprovedRoomRefresh: Decodable {
    let schemaVersion: String
    let status: String
    let snapshot: CoreSnapshotCarrierV1
    let comparison: RoomComparison
    let captureCompletedAtUnixMilliseconds: UInt64
    let captureElapsedMilliseconds: UInt64
    let comparisonElapsedMilliseconds: UInt64
    let searchAttempts: UInt32
    let searchEmptyTimeouts: UInt32
    let validSearchCount: UInt32
    let closeVerified: Bool
    let identityPolicy: String
    let staleSearchRepliesDiscarded: StaleSearchRepliesDiscarded?

    func validate(expectedDevice: UInt8, terminationStatus: Int32, startedAt: Date, now: Date) throws -> Date {
        try snapshot.validate(expectedDevice: expectedDevice)
        let capturedAt = Date(timeIntervalSince1970: Double(captureCompletedAtUnixMilliseconds) / 1000)
        guard schemaVersion == "dcx.approved-room-refresh/v1",
              identityPolicy == "approved-exact-target-one-identity-three-attempts",
              closeVerified, validSearchCount == 1, (1...3).contains(searchAttempts),
              searchEmptyTimeouts <= 2, searchEmptyTimeouts + 1 == searchAttempts,
              staleSearchRepliesDiscarded?.isWithinBound ?? true,
              captureElapsedMilliseconds <= 20_000,
              comparisonElapsedMilliseconds <= 45_000,
              capturedAt >= startedAt.addingTimeInterval(-0.001), capturedAt <= now,
              now.timeIntervalSince(capturedAt) < 60,
              comparison.snapshotDigest == snapshot.snapshotDigest,
              comparison.matches == (terminationStatus == 0),
              (terminationStatus == 0 && status == "match") || (terminationStatus == 1 && status == "drift") else {
            throw RoomVerificationError.invalidComparison
        }
        return capturedAt
    }

    var metrics: RoomCaptureMetrics {
        .init(identityPolicy: identityPolicy, searchAttempts: searchAttempts,
            searchEmptyTimeouts: searchEmptyTimeouts, validSearchCount: validSearchCount,
            captureElapsedMilliseconds: captureElapsedMilliseconds,
            comparisonElapsedMilliseconds: comparisonElapsedMilliseconds, closeVerified: closeVerified,
            staleSearchRepliesDiscarded: staleSearchRepliesDiscarded)
    }
}


/// A completed failed child remains failure data, even when its close succeeded.
/// Legacy failed envelopes have no typed authority and retain unknown close.
struct ApprovedRoomFailureEnvelope: Decodable {
    let failure: RoomCaptureFailureV1
    let searchAttempts: UInt32
    let searchEmptyTimeouts: UInt32
    let captureElapsedMilliseconds: UInt64
    let staleSearchRepliesDiscarded: StaleSearchRepliesDiscarded?
    private enum Keys: String, CodingKey {
        case schemaVersion, status, snapshot, comparison, failure, closeVerified
        case searchAttempts, searchEmptyTimeouts, captureElapsedMilliseconds
        case staleSearchRepliesDiscarded
    }
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: Keys.self)
        guard try values.decode(String.self, forKey: .schemaVersion) == "dcx.approved-room-refresh/v1",
              try values.decode(String.self, forKey: .status) == "failed",
              try values.decodeNil(forKey: .snapshot), try values.decodeNil(forKey: .comparison) else {
            throw RoomVerificationError.invalidComparison
        }
        failure = try values.decode(RoomCaptureFailureV1.self, forKey: .failure)
        searchAttempts = try values.decode(UInt32.self, forKey: .searchAttempts)
        searchEmptyTimeouts = try values.decode(UInt32.self, forKey: .searchEmptyTimeouts)
        captureElapsedMilliseconds = try values.decode(UInt64.self, forKey: .captureElapsedMilliseconds)
        staleSearchRepliesDiscarded = try values.decodeIfPresent(StaleSearchRepliesDiscarded.self,
            forKey: .staleSearchRepliesDiscarded)
        let closed = try values.decode(Bool.self, forKey: .closeVerified)
        let carrierKinds: Set<String> = ["unexpected_trailing_frame", "input_recovery_required", "deadline", "overflow", "end_of_file", "preexisting_input", "short_write", "system", "cleanup", "binding", "other"]
        guard failure.schemaVersion == "dcx.approved-room-capture-failure/v1",
              searchAttempts <= 3, searchEmptyTimeouts <= searchAttempts,
              staleSearchRepliesDiscarded?.isWithinBound ?? true,
              captureElapsedMilliseconds <= 30_000,
              closed == failure.sessionClosedVerified,
              (closed && failure.closeFailure == nil) || (!closed && failure.closeFailure != nil),
              failure.closeFailure.map({ carrierKinds.contains($0) }) ?? true,
              failure.received.map({ $0 <= 2048 }) ?? true,
              failure.category != .timeout || (failure.operation != nil && failure.received != nil),
              failure.category != .finish || !closed else {
            throw RoomVerificationError.invalidComparison
        }
    }
    static func decodeIfFailed(_ data: Data, terminationStatus: Int32) throws -> Self? {
        struct Header: Decodable {
            let status: String
            let hasFailure: Bool
            private enum Keys: String, CodingKey { case status, failure }
            init(from decoder: Decoder) throws {
                let values = try decoder.container(keyedBy: Keys.self)
                status = try values.decode(String.self, forKey: .status)
                hasFailure = values.contains(.failure)
            }
        }
        let header = try BridgeJSONCodec.decoder().decode(Header.self, from: data)
        guard header.status == "failed" else {
            guard !header.hasFailure else { throw RoomVerificationError.invalidComparison }
            return nil
        }
        guard terminationStatus == 1 else { throw RoomVerificationError.invalidComparison }
        return try BridgeJSONCodec.decoder().decode(Self.self, from: data)
    }
}

struct ApprovedRoomCaptureFailed: Error {
    let failure: RoomCaptureFailureV1
    /// TIN-6964, ruling 70: the capture time of the receipt the coordinator
    /// kept after this failure, or nil when it invalidated the receipt. Only a
    /// periodic read sets it, and only when `keepsPriorReceipt` holds.
    var keptReceiptCapturedAt: Date? = nil

    /// TIN-6964, ruling 70: a periodic read that failed this way may keep the
    /// last good receipt until its own sixty-second capture limit. It covers
    /// the three kinds the periodic reader retries without a write: an empty
    /// or partial Dump0 or Dump1 timeout and a Search timeout or transport
    /// failure, each with a verified close. A validation failure is left out,
    /// because the record does not say whether the frame named another device
    /// or another state. An unknown or failed close is left out too. The
    /// protected `room-refresh` child writes no parameter, and every bridge
    /// device operation invalidates the receipt before it starts.
    var keepsPriorReceipt: Bool {
        permitsPeriodicRetry || permitsBoundedPartialRetry || permitsBoundedSearchRetry
    }
    /// TIN-6964: a completed, verified-close empty Dump0 or Dump1 timeout
    /// rearms with backoff, or after `PeriodicRoomVerification.keptRetrySpacing`
    /// while a kept receipt holds under ruling 70, at most
    /// `PeriodicRoomVerification.keptEmptyRetryCap` times per kept receipt.
    var permitsPeriodicRetry: Bool {
        isVerifiedClosedDumpTimeout && failure.received == 0
    }

    /// TIN-6964: a completed, verified-close Dump0 or Dump1 timeout after some
    /// bytes. The periodic reader may retry it on its next period, or after
    /// the retry spacing while a kept receipt holds under ruling 70, up to
    /// `PeriodicRoomVerification.partialReadCap` in a row. Dump0 and Dump1 are
    /// reads, so no write is replayed. Unknown close or count still latches.
    var permitsBoundedPartialRetry: Bool {
        guard let received = failure.received else { return false }
        return isVerifiedClosedDumpTimeout && received > 0
    }

    /// TIN-6964: a completed, verified-close Dump0 or Dump1 read whose complete
    /// frame failed validation. On November 15 a 26-byte frame arrived where the
    /// 1015-byte Dump0 belongs. The periodic reader may retry it on its next
    /// period, up to `PeriodicRoomVerification.validationReadCap` in a row.
    /// Dump0 and Dump1 are reads, so no write is replayed. The producer reports
    /// no byte count for a complete frame, so a validation record that carries
    /// one is not the producer's form and still latches. Unknown close latches.
    var permitsBoundedValidationRetry: Bool {
        isVerifiedClosedDump(.validation) && failure.received == nil
    }

    /// TIN-6964: a completed, verified-close Search that timed out or failed in
    /// transport. A timeout qualifies with or without received bytes. On
    /// November 15 one such Search timeout latched the reader for 16 hours. The
    /// periodic reader may retry it on its next period, or after the retry
    /// spacing while a kept receipt holds under ruling 70, up to
    /// `PeriodicRoomVerification.searchReadCap` in a row. Search is a read, so
    /// no write is replayed, and the retry opens a new session. Search
    /// validation, such as a wrong device, and an unknown or failed close still
    /// latch.
    var permitsBoundedSearchRetry: Bool {
        failure.schemaVersion == "dcx.approved-room-capture-failure/v1" &&
        (failure.category == .timeout || failure.category == .transport) &&
        failure.operation == .search &&
        failure.sessionClosedVerified && failure.closeFailure == nil
    }

    /// The periodic outcome for this failure. A failure of a read that is no
    /// longer current, or of a kind no rule above admits, latches as `.failed`.
    func periodicOutcome(current: Bool) -> PeriodicRoomVerification.Outcome {
        guard current else { return .failed }
        if permitsPeriodicRetry { return .retryableReadFailure }
        if permitsBoundedPartialRetry, let received = failure.received {
            return .partialReadFailure(received: received)
        }
        if permitsBoundedValidationRetry { return .validationReadFailure }
        if permitsBoundedSearchRetry { return .searchReadFailure }
        return .failed
    }

    private var isVerifiedClosedDumpTimeout: Bool {
        isVerifiedClosedDump(.timeout)
    }

    private func isVerifiedClosedDump(_ category: RoomCaptureFailureV1.Category) -> Bool {
        failure.schemaVersion == "dcx.approved-room-capture-failure/v1" &&
        failure.category == category && (failure.operation == .dump0 || failure.operation == .dump1) &&
        failure.sessionClosedVerified && failure.closeFailure == nil
    }
}
