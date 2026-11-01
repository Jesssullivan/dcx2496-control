import Foundation

public struct RoomCaptureMetrics: Codable, Equatable, Sendable {
    public let identityPolicy: String
    public let searchAttempts: UInt32
    public let searchEmptyTimeouts: UInt32
    public let validSearchCount: UInt32
    public let captureElapsedMilliseconds: UInt64
    public let comparisonElapsedMilliseconds: UInt64
    public let closeVerified: Bool
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

    func validate(expectedDevice: UInt8, terminationStatus: Int32, startedAt: Date, now: Date) throws -> Date {
        try snapshot.validate(expectedDevice: expectedDevice)
        let capturedAt = Date(timeIntervalSince1970: Double(captureCompletedAtUnixMilliseconds) / 1000)
        guard schemaVersion == "dcx.approved-room-refresh/v1",
              identityPolicy == "approved-exact-target-one-identity-three-attempts",
              closeVerified, validSearchCount == 1, (1...3).contains(searchAttempts),
              searchEmptyTimeouts <= 2, searchEmptyTimeouts + 1 == searchAttempts,
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
            comparisonElapsedMilliseconds: comparisonElapsedMilliseconds, closeVerified: closeVerified)
    }
}
