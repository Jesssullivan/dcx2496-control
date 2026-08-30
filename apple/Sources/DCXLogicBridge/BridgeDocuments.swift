import CryptoKit
import Foundation

public enum DCXBridgeContract {
    public static let schemaVersion = "dcx.logic-bridge/v1"
    public static let appGroupIdentifier = "QP994XQKNH.io.tinyland.dcx2496"
    // Keep the socket at the App Group root and the leaf deliberately short:
    // Darwin's sockaddr_un path limit includes the full container path.
    public static let socketFileName = "dcx-v1.sock"
    public static let helperConfigurationFileName = "helper-config-v1.json"
    public static let maximumFrameBytes = 1_048_576
}

public struct DCXTargetReference: Codable, Equatable, Sendable {
    public let bindingID: String
    public let expectedDeviceAddress: UInt8

    public init(bindingID: String, expectedDeviceAddress: UInt8) throws {
        guard !bindingID.isEmpty, bindingID.utf8.count <= 128 else {
            throw BridgeValidationError.invalidTarget("bindingID must contain 1...128 UTF-8 bytes")
        }
        guard expectedDeviceAddress <= 15 else {
            throw BridgeValidationError.invalidTarget("DCX device address must be 0...15")
        }
        self.bindingID = bindingID
        self.expectedDeviceAddress = expectedDeviceAddress
    }

    public func validate() throws {
        _ = try Self(bindingID: bindingID, expectedDeviceAddress: expectedDeviceAddress)
    }
}

public struct DeviceIdentityV1: Codable, Equatable, Sendable {
    public static let schemaVersion = "dcx.device-identity/v1"

    public let manufacturer: String
    public let model: String
    public let deviceAddress: UInt8
    public let selectedBaud: UInt32
    public let validSearchResponses: UInt8

    public init(
        manufacturer: String,
        model: String,
        deviceAddress: UInt8,
        selectedBaud: UInt32,
        validSearchResponses: UInt8
    ) {
        self.manufacturer = manufacturer
        self.model = model
        self.deviceAddress = deviceAddress
        self.selectedBaud = selectedBaud
        self.validSearchResponses = validSearchResponses
    }

    public func validate() throws {
        guard manufacturer == "Behringer", model == "DCX2496",
              deviceAddress <= 15, selectedBaud == 38_400,
              (1...10).contains(validSearchResponses) else {
            throw BridgeValidationError.invalidIdentity
        }
    }
}

public struct SnapshotSectionDigestsV1: Codable, Equatable, Sendable {
    public let identity: String
    public let dump0: String
    public let dump1: String

    public init(identity: String, dump0: String, dump1: String) throws {
        try BridgeDigest.validate(identity)
        try BridgeDigest.validate(dump0)
        try BridgeDigest.validate(dump1)
        self.identity = identity
        self.dump0 = dump0
        self.dump1 = dump1
    }

    public func validate() throws {
        try BridgeDigest.validate(identity)
        try BridgeDigest.validate(dump0)
        try BridgeDigest.validate(dump1)
    }
}

/// Sanitized reference to a complete helper-owned raw snapshot. Exact Search,
/// Dump0, and Dump1 frames never cross into the Audio Unit or Logic state.
public struct SnapshotV1: Codable, Equatable, Sendable {
    public static let currentSchemaVersion = "dcx.snapshot/v1"

    public let schemaVersion: String
    public let target: DCXTargetReference
    public let identity: DeviceIdentityV1
    public let capturedAt: Date
    public let digest: String
    public let complete: Bool
    public let sectionDigests: SnapshotSectionDigestsV1

    public init(
        target: DCXTargetReference,
        identity: DeviceIdentityV1,
        capturedAt: Date,
        digest: String,
        complete: Bool,
        sectionDigests: SnapshotSectionDigestsV1
    ) throws {
        guard complete else { throw BridgeValidationError.incompleteSnapshot }
        try BridgeDigest.validate(digest)
        self.schemaVersion = Self.currentSchemaVersion
        self.target = target
        self.identity = identity
        self.capturedAt = capturedAt
        self.digest = digest
        self.complete = true
        self.sectionDigests = sectionDigests
    }

    public func validate() throws {
        guard schemaVersion == Self.currentSchemaVersion, complete else {
            throw BridgeValidationError.incompleteSnapshot
        }
        try target.validate()
        try identity.validate()
        guard identity.deviceAddress == target.expectedDeviceAddress else {
            throw BridgeValidationError.invalidIdentity
        }
        try BridgeDigest.validate(digest)
        try sectionDigests.validate()
    }
}

public struct DesiredProfileV1: Codable, Equatable, Sendable {
    public static let currentSchemaVersion = "dcx.desired-profile/v1"

    public let schemaVersion: String
    public let profileID: String
    public let revision: String
    public let digest: String
    public let document: JSONValue

    public init(profileID: String, revision: String, digest: String, document: JSONValue) throws {
        self.schemaVersion = Self.currentSchemaVersion
        self.profileID = profileID
        self.revision = revision
        self.digest = digest
        self.document = document
        try validate()
    }

    public func validate() throws {
        guard schemaVersion == Self.currentSchemaVersion else {
            throw BridgeValidationError.invalidProfile("unsupported desired-profile schema")
        }
        try BridgeDigest.validate(digest)
        try DesiredProfileContract.validate(self)
    }
}

public struct SemanticChangeV1: Codable, Equatable, Sendable {
    public let path: String
    public let before: JSONValue
    public let after: JSONValue

    public init(path: String, before: JSONValue, after: JSONValue) throws {
        guard path == "$.outputs[0].peq[8]" else {
            throw BridgeValidationError.invalidDiff("the MVP diff path must be exact O1/PEQ9")
        }
        self.path = path
        self.before = before
        self.after = after
    }

    public func validate() throws {
        _ = try Self(path: path, before: before, after: after)
    }
}

public struct SemanticDiffV1: Codable, Equatable, Sendable {
    public static let currentSchemaVersion = "dcx.semantic-diff/v1"

    public let schemaVersion: String
    public let baselineSnapshotDigest: String
    public let desiredProfileDigest: String
    public let desiredSnapshotDigest: String
    public let applyPlanDigest: String
    public let rollbackPlanDigest: String
    public let changes: [SemanticChangeV1]

    public init(
        baselineSnapshotDigest: String,
        desiredProfileDigest: String,
        desiredSnapshotDigest: String,
        applyPlanDigest: String,
        rollbackPlanDigest: String,
        changes: [SemanticChangeV1]
    ) throws {
        try BridgeDigest.validate(baselineSnapshotDigest)
        try BridgeDigest.validate(desiredProfileDigest)
        try BridgeDigest.validate(desiredSnapshotDigest)
        try BridgeDigest.validate(applyPlanDigest)
        try BridgeDigest.validate(rollbackPlanDigest)
        guard changes.count <= 1 else {
            throw BridgeValidationError.invalidDiff("the MVP diff contains at most one PEQ slot change")
        }
        self.schemaVersion = Self.currentSchemaVersion
        self.baselineSnapshotDigest = baselineSnapshotDigest
        self.desiredProfileDigest = desiredProfileDigest
        self.desiredSnapshotDigest = desiredSnapshotDigest
        self.applyPlanDigest = applyPlanDigest
        self.rollbackPlanDigest = rollbackPlanDigest
        self.changes = changes
    }

    public func validate() throws {
        guard schemaVersion == Self.currentSchemaVersion, changes.count <= 1 else {
            throw BridgeValidationError.invalidDiff("unexpected schema or too many changes")
        }
        try BridgeDigest.validate(baselineSnapshotDigest)
        try BridgeDigest.validate(desiredProfileDigest)
        try BridgeDigest.validate(desiredSnapshotDigest)
        try BridgeDigest.validate(applyPlanDigest)
        try BridgeDigest.validate(rollbackPlanDigest)
        try changes.forEach { try $0.validate() }
    }
}

public struct ApplyPlanV1: Codable, Equatable, Sendable {
    public static let currentSchemaVersion = "dcx.apply-plan/v1"

    public let schemaVersion: String
    public let baseline: SnapshotV1
    public let desired: DesiredProfileV1
    public let diff: SemanticDiffV1

    public init(
        baseline: SnapshotV1,
        desired: DesiredProfileV1,
        diff: SemanticDiffV1
    ) throws {
        guard baseline.complete,
              diff.baselineSnapshotDigest == baseline.digest,
              diff.desiredProfileDigest == desired.digest else {
            throw BridgeValidationError.invalidApplyBinding
        }
        self.schemaVersion = Self.currentSchemaVersion
        self.baseline = baseline
        self.desired = desired
        self.diff = diff
    }

    public func validate() throws {
        guard schemaVersion == Self.currentSchemaVersion else {
            throw BridgeValidationError.invalidApplyBinding
        }
        try baseline.validate()
        try desired.validate()
        try diff.validate()
        guard diff.baselineSnapshotDigest == baseline.digest,
              diff.desiredProfileDigest == desired.digest else {
            throw BridgeValidationError.invalidApplyBinding
        }
    }
}

public struct RollbackPlanV1: Codable, Equatable, Sendable {
    public static let currentSchemaVersion = "dcx.rollback-plan/v1"

    public let schemaVersion: String
    public let transactionID: String
    public let baseline: SnapshotV1
    public let rollbackPlanDigest: String

    public init(
        transactionID: String,
        baseline: SnapshotV1,
        rollbackPlanDigest: String
    ) throws {
        guard !transactionID.isEmpty, transactionID.utf8.count <= 128, baseline.complete else {
            throw BridgeValidationError.invalidRollbackBinding
        }
        try BridgeDigest.validate(rollbackPlanDigest)
        self.schemaVersion = Self.currentSchemaVersion
        self.transactionID = transactionID
        self.baseline = baseline
        self.rollbackPlanDigest = rollbackPlanDigest
    }

    public func validate() throws {
        guard schemaVersion == Self.currentSchemaVersion,
              !transactionID.isEmpty, transactionID.utf8.count <= 128 else {
            throw BridgeValidationError.invalidRollbackBinding
        }
        try baseline.validate()
        try BridgeDigest.validate(rollbackPlanDigest)
    }
}

public enum BridgeDigest {
    public static func validate(_ value: String) throws {
        let bytes = value.utf8
        guard value.hasPrefix("sha256/"), bytes.count == 71,
              bytes.dropFirst(7).allSatisfy({ byte in
                  (48...57).contains(byte) || (97...102).contains(byte)
              }) else {
            throw BridgeValidationError.invalidDigest
        }
    }
}

private enum DesiredProfileContract {
    private static let documentKeys = Set(["target_output", "parameter_channel", "slot", "actions"])
    private static let actionKeys = Set(["channel", "parameter", "value"])
    private static let expectedParameters: [UInt16] = [59, 60, 61, 62]
    private static let maximumValues: [UInt16] = [320, 40, 150, 1]

    static func validate(_ profile: DesiredProfileV1) throws {
        guard boundedASCII(profile.profileID), boundedASCII(profile.revision),
              case let .object(document) = profile.document,
              Set(document.keys) == documentKeys,
              integer(document["target_output"]) == 1,
              integer(document["parameter_channel"]) == 5,
              integer(document["slot"]) == 9,
              case let .array(actions)? = document["actions"],
              actions.count == expectedParameters.count else {
            throw BridgeValidationError.invalidProfile(
                "desired profile must be exact O1/channel-5/PEQ9 v1"
            )
        }

        var values = [UInt16]()
        for (index, action) in actions.enumerated() {
            guard case let .object(fields) = action,
                  Set(fields.keys) == actionKeys,
                  integer(fields["channel"]) == 5,
                  integer(fields["parameter"]) == expectedParameters[index],
                  let value = integer(fields["value"]),
                  value <= maximumValues[index],
                  index != 3 || value == 1 else {
                throw BridgeValidationError.invalidProfile(
                    "desired profile actions must be parameters 59...62 within MVP bounds and Peak kind 1"
                )
            }
            values.append(value)
        }

        guard profile.digest == digest(profile: profile, actionValues: values) else {
            throw BridgeValidationError.invalidProfile(
                "desired profile digest does not match its exact MVP document"
            )
        }
    }

    private static func boundedASCII(_ value: String) -> Bool {
        !value.isEmpty && value.utf8.count <= 128 && value.utf8.allSatisfy { $0 < 0x80 }
    }

    private static func integer(_ value: JSONValue?) -> UInt16? {
        guard case let .number(number)? = value, number.isFinite,
              number.rounded(.towardZero) == number,
              (0...Double(UInt16.max)).contains(number) else {
            return nil
        }
        return UInt16(number)
    }

    private static func digest(profile: DesiredProfileV1, actionValues: [UInt16]) -> String {
        var hasher = SHA256()
        hasher.update(data: Data("dcx2496.desired-profile/v1\0".utf8))
        append(profile.profileID, to: &hasher)
        append(profile.revision, to: &hasher)
        hasher.update(data: Data([1, 5, 9, UInt8(actionValues.count)]))
        for (index, value) in actionValues.enumerated() {
            hasher.update(data: Data([
                5,
                UInt8(expectedParameters[index]),
                UInt8(value >> 8),
                UInt8(value & 0xff),
            ]))
        }
        return "sha256/" + hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }

    private static func append(_ value: String, to hasher: inout SHA256) {
        let bytes = Array(value.utf8)
        hasher.update(data: Data([UInt8(bytes.count)]))
        hasher.update(data: Data(bytes))
    }
}

public enum BridgeValidationError: Error, Equatable, Sendable {
    case invalidTarget(String)
    case invalidProfile(String)
    case invalidDiff(String)
    case invalidDigest
    case invalidIdentity
    case incompleteSnapshot
    case invalidApplyBinding
    case invalidRollbackBinding
}
