import CryptoKit
import Foundation

/// Swift mirror of the reviewed PEQ allowlist in `crates/dcx-core/src/layout.rs`
/// as admitted by `DesiredPeqBankProfileV2`: PEQ on/off (`0x06`), band count
/// (`0x07`), and the nine bands (`0x13` through `0x3f`) on output channels 5
/// through 10. The O4 mute is reviewed for its own lane but never belongs to a
/// PEQ profile, so it does not classify here.
public struct PeqAddressV2: Equatable, Sendable {
    public enum Field: Equatable, Sendable {
        case eqEnabled
        case eqCount
        case band(UInt8, BandField)
    }

    public enum BandField: UInt8, CaseIterable, Sendable {
        case frequency = 0
        case q = 1
        case gain = 2
        case kind = 3
        case slope = 4

        var name: String {
            switch self {
            case .frequency: "frequency"
            case .q: "q"
            case .gain: "gain"
            case .kind: "kind"
            case .slope: "slope"
            }
        }

        var deviceMaximum: UInt16 {
            switch self {
            case .frequency: 320
            case .q: 40
            case .gain: 300
            case .kind: 2
            case .slope: 1
            }
        }
    }

    public static let firstOutputChannel: UInt8 = 5
    public static let outputCount: UInt8 = 6
    public static let bandCount: UInt8 = 9
    public static let eqEnabledParameter: UInt8 = 0x06
    public static let eqCountParameter: UInt8 = 0x07
    public static let firstBandParameter: UInt8 = 0x13
    public static let bandParameterStride: UInt8 = 5
    /// Gain code 150 is 0 dB; the v2 profile admits cuts and unity only.
    public static let unityGainCode: UInt16 = 150
    public static let bellKindCode: UInt16 = 1

    public let output: UInt8
    public let field: Field

    public init?(channel: UInt8, parameter: UInt8) {
        guard (Self.firstOutputChannel..<Self.firstOutputChannel + Self.outputCount).contains(channel) else {
            return nil
        }
        output = channel - Self.firstOutputChannel + 1
        let lastBandParameter = Self.firstBandParameter + Self.bandCount * Self.bandParameterStride - 1
        switch parameter {
        case Self.eqEnabledParameter:
            field = .eqEnabled
        case Self.eqCountParameter:
            field = .eqCount
        case Self.firstBandParameter...lastBandParameter:
            let relative = parameter - Self.firstBandParameter
            guard let bandField = BandField(rawValue: relative % Self.bandParameterStride) else {
                return nil
            }
            field = .band(relative / Self.bandParameterStride + 1, bandField)
        default:
            return nil
        }
    }

    public static func channel(output: UInt8) -> UInt8 {
        firstOutputChannel + output - 1
    }

    public static func parameter(band: UInt8, field: BandField) -> UInt8 {
        firstBandParameter + (band - 1) * bandParameterStride + field.rawValue
    }

    /// Stable label, identical to `OutputField::label` in Rust.
    public var label: String {
        switch field {
        case .eqEnabled: "eq_enabled"
        case .eqCount: "eq_count"
        case let .band(band, bandField): "band\(band).\(bandField.name)"
        }
    }

    public var deviceMaximum: UInt16 {
        switch field {
        case .eqEnabled: 1
        case .eqCount: UInt16(Self.bandCount)
        case let .band(_, bandField): bandField.deviceMaximum
        }
    }

    public var isGain: Bool {
        if case .band(_, .gain) = field { return true }
        return false
    }

    /// Whether a desired value lies in the device domain and, for a gain, is
    /// a cut or unity.
    public func admits(_ value: UInt16) -> Bool {
        value <= deviceMaximum && (!isGain || value <= Self.unityGainCode)
    }
}

/// One direct-parameter action of a v2 document.
public struct DirectParameterActionV2: Codable, Equatable, Hashable, Sendable {
    public let channel: UInt8
    public let parameter: UInt8
    public let value: UInt16

    public init(channel: UInt8, parameter: UInt8, value: UInt16) {
        self.channel = channel
        self.parameter = parameter
        self.value = value
    }
}

/// Typed view of a validated v2 document.
public struct DesiredPeqBankV2: Equatable, Sendable {
    public let targetOutput: UInt8
    public let parameterChannel: UInt8
    public let actions: [DirectParameterActionV2]

    public init(targetOutput: UInt8, parameterChannel: UInt8, actions: [DirectParameterActionV2]) {
        self.targetOutput = targetOutput
        self.parameterChannel = parameterChannel
        self.actions = actions
    }
}

/// `dcx.desired-profile/v2`: one output's ordered cut-only PEQ on/off, band
/// count, and band-field actions. Validation and the digest mirror
/// `DesiredPeqBankProfileV2` in `crates/dcx-core/src/peq_bank.rs` exactly; the
/// document stays a closed JSON object so unknown keys fail closed here too.
public struct DesiredProfileV2: Codable, Equatable, Sendable {
    public static let currentSchemaVersion = "dcx.desired-profile/v2"
    /// On/off, count, and all five fields of nine bands.
    public static let maximumActions = 2 + Int(PeqAddressV2.bandCount) * 5

    private static let digestDomain = Data("dcx2496.desired-profile/v2\0".utf8)
    private static let documentKeys = Set(["target_output", "parameter_channel", "actions"])
    private static let actionKeys = Set(["channel", "parameter", "value"])

    public let schemaVersion: String
    public let profileID: String
    public let revision: String
    public let digest: String
    public let document: JSONValue

    public init(profileID: String, revision: String, digest: String, document: JSONValue) throws {
        schemaVersion = Self.currentSchemaVersion
        self.profileID = profileID
        self.revision = revision
        self.digest = digest
        self.document = document
        try validate()
    }

    /// Build the canonical envelope for a document, computing its digest.
    public init(profileID: String, revision: String, bank: DesiredPeqBankV2) throws {
        let document: JSONValue = .object([
            "target_output": .number(Double(bank.targetOutput)),
            "parameter_channel": .number(Double(bank.parameterChannel)),
            "actions": .array(bank.actions.map {
                .object([
                    "channel": .number(Double($0.channel)),
                    "parameter": .number(Double($0.parameter)),
                    "value": .number(Double($0.value)),
                ])
            }),
        ])
        try self.init(
            profileID: profileID,
            revision: revision,
            digest: Self.digest(profileID: profileID, revision: revision, bank: bank),
            document: document
        )
    }

    public func validate() throws {
        guard schemaVersion == Self.currentSchemaVersion else {
            throw BridgeValidationError.invalidProfile("unsupported desired-profile schema")
        }
        try BridgeDigest.validate(digest)
        guard BridgeText.boundedASCII(profileID), BridgeText.boundedASCII(revision) else {
            throw BridgeValidationError.invalidProfile("profile identity must be 1...128 ASCII bytes")
        }
        let bank = try bank()
        guard digest == Self.digest(profileID: profileID, revision: revision, bank: bank) else {
            throw BridgeValidationError.invalidProfile(
                "desired profile digest does not match its exact v2 document"
            )
        }
    }

    /// Parse the closed document. Every rule of `validate_bank_document`.
    public func bank() throws -> DesiredPeqBankV2 {
        guard case let .object(fields) = document,
              Set(fields.keys) == Self.documentKeys,
              let output = BridgeJSONInteger.uint8(fields["target_output"]),
              (1...PeqAddressV2.outputCount).contains(output),
              let channel = BridgeJSONInteger.uint8(fields["parameter_channel"]),
              channel == PeqAddressV2.channel(output: output),
              case let .array(rawActions)? = fields["actions"],
              (1...Self.maximumActions).contains(rawActions.count) else {
            throw BridgeValidationError.invalidProfile(
                "v2 profile must be one output 1...6 with its exact channel and 1...47 actions"
            )
        }
        var seen = Set<UInt8>()
        var actions = [DirectParameterActionV2]()
        actions.reserveCapacity(rawActions.count)
        for raw in rawActions {
            guard case let .object(actionFields) = raw,
                  Set(actionFields.keys) == Self.actionKeys,
                  let actionChannel = BridgeJSONInteger.uint8(actionFields["channel"]),
                  let parameter = BridgeJSONInteger.uint8(actionFields["parameter"]),
                  let value = BridgeJSONInteger.uint16(actionFields["value"]),
                  actionChannel == channel,
                  seen.insert(parameter).inserted,
                  let address = PeqAddressV2(channel: actionChannel, parameter: parameter),
                  address.admits(value) else {
                throw BridgeValidationError.invalidProfile(
                    "v2 profile actions must be distinct cut-only reviewed PEQ fields in their device domain"
                )
            }
            actions.append(.init(channel: actionChannel, parameter: parameter, value: value))
        }
        return .init(targetOutput: output, parameterChannel: channel, actions: actions)
    }

    static func digest(profileID: String, revision: String, bank: DesiredPeqBankV2) -> String {
        var hasher = SHA256()
        hasher.update(data: digestDomain)
        for text in [profileID, revision] {
            let bytes = Array(text.utf8)
            hasher.update(data: Data([UInt8(truncatingIfNeeded: bytes.count)]))
            hasher.update(data: Data(bytes))
        }
        hasher.update(data: Data([
            bank.targetOutput,
            bank.parameterChannel,
            UInt8(truncatingIfNeeded: bank.actions.count),
        ]))
        for action in bank.actions {
            hasher.update(data: Data([
                action.channel,
                action.parameter,
                UInt8(action.value >> 8),
                UInt8(action.value & 0xff),
            ]))
        }
        return "sha256/" + hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }
}

/// Either supported desired-profile envelope. Encoding is transparent, so a
/// v1 profile keeps its exact v1 wire and Logic project bytes.
public enum DesiredProfile: Codable, Equatable, Sendable {
    case v1(DesiredProfileV1)
    case v2(DesiredProfileV2)

    private enum PeekKeys: String, CodingKey { case schemaVersion }

    public init(from decoder: Decoder) throws {
        let peek = try decoder.container(keyedBy: PeekKeys.self)
        switch try peek.decode(String.self, forKey: .schemaVersion) {
        case DesiredProfileV1.currentSchemaVersion:
            self = .v1(try DesiredProfileV1(from: decoder))
        case DesiredProfileV2.currentSchemaVersion:
            self = .v2(try DesiredProfileV2(from: decoder))
        default:
            throw BridgeValidationError.invalidProfile("unsupported desired-profile schema")
        }
    }

    public func encode(to encoder: Encoder) throws {
        switch self {
        case let .v1(profile): try profile.encode(to: encoder)
        case let .v2(profile): try profile.encode(to: encoder)
        }
    }

    public func validate() throws {
        switch self {
        case let .v1(profile): try profile.validate()
        case let .v2(profile): try profile.validate()
        }
    }

    public var schemaVersion: String {
        switch self {
        case let .v1(profile): profile.schemaVersion
        case let .v2(profile): profile.schemaVersion
        }
    }

    public var digest: String {
        switch self {
        case let .v1(profile): profile.digest
        case let .v2(profile): profile.digest
        }
    }

    public var profileID: String {
        switch self {
        case let .v1(profile): profile.profileID
        case let .v2(profile): profile.profileID
        }
    }

    public var revision: String {
        switch self {
        case let .v1(profile): profile.revision
        case let .v2(profile): profile.revision
        }
    }
}

enum BridgeText {
    static func boundedASCII(_ value: String) -> Bool {
        !value.isEmpty && value.utf8.count <= 128 && value.utf8.allSatisfy { $0 < 0x80 }
    }
}

enum BridgeJSONInteger {
    static func uint16(_ value: JSONValue?) -> UInt16? {
        guard case let .number(number)? = value, number.isFinite,
              number.rounded(.towardZero) == number,
              (0...Double(UInt16.max)).contains(number) else {
            return nil
        }
        return UInt16(number)
    }

    static func uint8(_ value: JSONValue?) -> UInt8? {
        uint16(value).flatMap { $0 <= UInt16(UInt8.max) ? UInt8($0) : nil }
    }
}
