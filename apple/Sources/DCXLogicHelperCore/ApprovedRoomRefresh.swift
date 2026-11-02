import Foundation
import DCXLogicBridge

public struct RoomCaptureMetrics: Codable, Equatable, Sendable {
    public let identityPolicy: String
    public let searchAttempts: UInt32
    public let searchEmptyTimeouts: UInt32
    public let validSearchCount: UInt32
    public let captureElapsedMilliseconds: UInt64
    public let comparisonElapsedMilliseconds: UInt64
    public let closeVerified: Bool
    public var ignoredDump0SearchReplays: UInt32? = nil
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
    let ignoredDump0SearchReplays: UInt32?

    func validate(expectedDevice: UInt8, terminationStatus: Int32, startedAt: Date, now: Date) throws -> Date {
        try snapshot.validate(expectedDevice: expectedDevice)
        let capturedAt = Date(timeIntervalSince1970: Double(captureCompletedAtUnixMilliseconds) / 1000)
        guard schemaVersion == "dcx.approved-room-refresh/v1",
              identityPolicy == "approved-exact-target-one-identity-three-attempts",
              closeVerified, validSearchCount == 1, (1...3).contains(searchAttempts),
              searchEmptyTimeouts <= 2, searchEmptyTimeouts + 1 == searchAttempts,
              (ignoredDump0SearchReplays ?? 0) <= 1,
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
            ignoredDump0SearchReplays: ignoredDump0SearchReplays)
    }
}


/// A completed failed child remains failure data, even when its close succeeded.
/// Legacy failed envelopes have no typed authority and retain unknown close.
struct ApprovedRoomFailureEnvelope: Decodable {
    let failure: RoomCaptureFailureV1
    let searchAttempts: UInt32
    let searchEmptyTimeouts: UInt32
    let captureElapsedMilliseconds: UInt64
    private enum Keys: String, CodingKey {
        case schemaVersion, status, snapshot, comparison, failure, closeVerified
        case searchAttempts, searchEmptyTimeouts, captureElapsedMilliseconds
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
        let closed = try values.decode(Bool.self, forKey: .closeVerified)
        let carrierKinds: Set<String> = ["unexpected_trailing_frame", "input_recovery_required", "deadline", "overflow", "end_of_file", "preexisting_input", "short_write", "system", "cleanup", "binding", "other"]
        guard failure.schemaVersion == "dcx.approved-room-capture-failure/v1",
              searchAttempts <= 3, searchEmptyTimeouts <= searchAttempts,
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
    /// TIN-6964: only a completed, verified-close empty Dump0 or Dump1 timeout may rearm.
    var permitsPeriodicRetry: Bool {
        failure.schemaVersion == "dcx.approved-room-capture-failure/v1" &&
        failure.category == .timeout && (failure.operation == .dump0 || failure.operation == .dump1) &&
        failure.received == 0 && failure.sessionClosedVerified && failure.closeFailure == nil
    }
}
