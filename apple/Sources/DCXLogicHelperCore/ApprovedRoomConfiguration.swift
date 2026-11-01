import CryptoKit
import Darwin
import DCXLogicBridge
import Foundation

/// Opt-in read-only startup authority. It pins both the saved manifest and the
/// existing helper's exact target and callout. It never grants parameter writes.
public struct ApprovedRoomConfiguration: Codable, Equatable, Sendable {
    public let schemaVersion: String
    public let target: DCXTargetReference
    public let ttyPath: String
    public let expectation: Data
    public let expectationDigest: String
    public let verifyOnForegroundLaunch: Bool
    public let allowExplicitRefreshRequests: Bool

    public init(configuration: HelperConfigurationV1, expectation: Data,
                verifyOnForegroundLaunch: Bool = false, allowExplicitRefreshRequests: Bool = false) throws {
        guard !expectation.isEmpty, expectation.count <= 256 * 1024 else { throw RoomVerificationError.invalidExpectation }
        schemaVersion = "dcx.approved-room-configuration/v1"
        target = configuration.target
        ttyPath = configuration.ttyPath
        self.expectation = expectation
        expectationDigest = Self.digest(expectation)
        self.verifyOnForegroundLaunch = verifyOnForegroundLaunch
        self.allowExplicitRefreshRequests = allowExplicitRefreshRequests
    }

    public func requireBinding(to configuration: HelperConfigurationV1) throws {
        guard schemaVersion == "dcx.approved-room-configuration/v1",
              target == configuration.target, ttyPath == configuration.ttyPath,
              !expectation.isEmpty, expectation.count <= 256 * 1024,
              expectationDigest == Self.digest(expectation) else { throw RoomVerificationError.invalidExpectation }
        try configuration.require(.snapshotCapture, target: target)
    }

    func save(to url: URL) throws {
        let manager = FileManager.default
        let directory = url.deletingLastPathComponent()
        try manager.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        try manager.setAttributes([.posixPermissions: 0o700], ofItemAtPath: directory.path)
        let data = try JSONEncoder().encode(self)
        guard data.count <= 400 * 1024 else { throw RoomVerificationError.invalidExpectation }
        try data.write(to: url, options: [.atomic, .completeFileProtection])
        try manager.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }

    static func load(from url: URL) throws -> Self {
        let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
        let resource = try url.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey])
        guard resource.isRegularFile == true, resource.isSymbolicLink != true,
              let count = resource.fileSize, count <= 400 * 1024,
              let mode = attributes[.posixPermissions] as? NSNumber, mode.intValue & 0o077 == 0,
              let owner = attributes[.ownerAccountID] as? NSNumber, owner.uint32Value == geteuid() else {
            throw RoomVerificationError.invalidExpectation
        }
        let value = try JSONDecoder().decode(Self.self, from: Data(contentsOf: url))
        guard value.schemaVersion == "dcx.approved-room-configuration/v1",
              !value.expectation.isEmpty, value.expectation.count <= 256 * 1024,
              value.expectationDigest == digest(value.expectation) else { throw RoomVerificationError.invalidExpectation }
        return value
    }

    public static func acceptsRefreshURL(_ url: URL) -> Bool {
        url.absoluteString == "tinyland-dcx://verify-approved-room"
    }

    private static func digest(_ data: Data) -> String {
        "sha256/" + SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }
}
