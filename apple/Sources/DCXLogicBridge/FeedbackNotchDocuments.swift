import CryptoKit
import Foundation

/// Static O4 feedback-notch planning across the bridge. The Audio Unit sends a
/// bounded measurement and a helper-stored snapshot reference; the helper runs
/// the offline `dcxctl feedback import|plan|desired-profile` children and
/// returns a sanitized plan summary plus the staged v2 desired profile. None of
/// these steps opens a device, and the AU never runs a process or reads a
/// helper-owned file.
public enum FeedbackNotchContract {
    public static let targetOutput: UInt8 = 4
    public static let parameterChannel: UInt8 = 8
    public static let measurementSchemaVersion = "dcx.feedback-measurement/v1"
    public static let notchPlanSchemaVersion = "dcx.notch-plan/v1"
    /// `MAX_FREQUENCY_LIST_BYTES` and `MAX_REW_BYTES` in dcx-core.
    public static let maximumSourceBytes = 64 * 1_024
    /// `MAX_FEEDBACK_JSON_BYTES` in dcx-core.
    public static let maximumDocumentBytes = 256 * 1_024
    /// `MAX_MEASUREMENT_PEAKS` in dcx-core.
    public static let maximumPeaks = 64

    public static func sourceDigest(_ text: String) -> String {
        "sha256/" + SHA256.hash(data: Data(text.utf8)).map { String(format: "%02x", $0) }.joined()
    }
}

/// The measurement the helper plans from: ring-out list or REW Generic EQ
/// text for `feedback import`, or an already imported
/// `dcx.feedback-measurement/v1` document.
public struct FeedbackMeasurementInputV1: Codable, Equatable, Sendable {
    public enum Kind: String, Codable, CaseIterable, Sendable {
        case frequencyList = "frequency_list"
        case rewGenericEq = "rew_generic_eq"
        case measurement
    }

    public let kind: Kind
    public let text: String?
    public let document: JSONValue?

    public static func frequencyList(_ text: String) throws -> Self {
        try .init(kind: .frequencyList, text: text, document: nil)
    }

    public static func rewGenericEq(_ text: String) throws -> Self {
        try .init(kind: .rewGenericEq, text: text, document: nil)
    }

    public static func measurement(_ document: JSONValue) throws -> Self {
        try .init(kind: .measurement, text: nil, document: document)
    }

    private init(kind: Kind, text: String?, document: JSONValue?) throws {
        self.kind = kind
        self.text = text
        self.document = document
        try validate()
    }

    public func validate() throws {
        switch kind {
        case .frequencyList, .rewGenericEq:
            guard document == nil, let text,
                  (1...FeedbackNotchContract.maximumSourceBytes).contains(text.utf8.count),
                  !text.utf8.contains(0) else {
                throw BridgeValidationError.invalidMeasurement(
                    "measurement text must be 1...65536 UTF-8 bytes without NUL"
                )
            }
        case .measurement:
            guard text == nil, case let .object(fields)? = document,
                  case let .string(schema)? = fields["schema_version"],
                  schema == FeedbackNotchContract.measurementSchemaVersion,
                  BridgeJSONInteger.uint8(fields["target_output"]) == FeedbackNotchContract.targetOutput,
                  case let .string(digest)? = fields["digest"],
                  (try? BridgeDigest.validate(digest)) != nil,
                  let encoded = try? BridgeJSONCodec.encoder().encode(document),
                  encoded.count <= FeedbackNotchContract.maximumDocumentBytes else {
                throw BridgeValidationError.invalidMeasurement(
                    "measurement document must be a bounded O4 dcx.feedback-measurement/v1"
                )
            }
        }
    }

    /// Digest of an already imported document; text is digested by the child.
    public var documentDigest: String? {
        guard case let .object(fields)? = document, case let .string(digest)? = fields["digest"] else {
            return nil
        }
        return digest
    }
}

/// Sanitized measurement identity: digests and counts, not the peaks.
public struct FeedbackMeasurementSummaryV1: Codable, Equatable, Sendable {
    public enum Source: String, Codable, Sendable {
        case frequencyList = "frequency_list"
        case rewGenericEq = "rew_generic_eq"
    }

    public let digest: String
    public let source: Source
    public let sourceDigest: String
    public let targetOutput: UInt8
    public let peakCount: Int

    public init(
        digest: String,
        source: Source,
        sourceDigest: String,
        targetOutput: UInt8,
        peakCount: Int
    ) throws {
        self.digest = digest
        self.source = source
        self.sourceDigest = sourceDigest
        self.targetOutput = targetOutput
        self.peakCount = peakCount
        try validate()
    }

    public func validate() throws {
        try BridgeDigest.validate(digest)
        try BridgeDigest.validate(sourceDigest)
        guard targetOutput == FeedbackNotchContract.targetOutput,
              (1...FeedbackNotchContract.maximumPeaks).contains(peakCount) else {
            throw BridgeValidationError.invalidMeasurement("measurement must carry 1...64 O4 peaks")
        }
    }
}

/// One planned static notch, by its exact device codes.
public struct PlannedNotchV1: Codable, Equatable, Sendable {
    public let band: UInt8
    public let frequencyHz: Double
    public let occurrences: UInt8
    public let levelDb: Double?
    public let frequencyCode: UInt16
    public let qCode: UInt16
    public let gainCode: UInt16
    public let kindCode: UInt16
    public let slopeCode: UInt16

    public init(
        band: UInt8,
        frequencyHz: Double,
        occurrences: UInt8,
        levelDb: Double?,
        frequencyCode: UInt16,
        qCode: UInt16,
        gainCode: UInt16,
        kindCode: UInt16,
        slopeCode: UInt16
    ) {
        self.band = band
        self.frequencyHz = frequencyHz
        self.occurrences = occurrences
        self.levelDb = levelDb
        self.frequencyCode = frequencyCode
        self.qCode = qCode
        self.gainCode = gainCode
        self.kindCode = kindCode
        self.slopeCode = slopeCode
    }

    /// `20 * 2^(code / 32)` Hz, as `peq_bank::frequency_hz`.
    public var encodedFrequencyHz: Double { 20 * pow(2, Double(frequencyCode) / 32) }
    /// `0.1 * 10^(code / 20)`, as `peq_bank::q_value`.
    public var encodedQ: Double { 0.1 * pow(10, Double(qCode) / 20) }
    /// `code / 10 - 15` dB, as `peq_bank::gain_db`.
    public var encodedGainDb: Double { Double(gainCode) / 10 - 15 }

    /// Ordered five-field band actions, as `BandCodes::actions`.
    public func actions(channel: UInt8) -> [DirectParameterActionV2] {
        zip(PeqAddressV2.BandField.allCases, [frequencyCode, qCode, gainCode, kindCode, slopeCode])
            .map { field, value in
                .init(channel: channel, parameter: PeqAddressV2.parameter(band: band, field: field), value: value)
            }
    }
}

public struct DroppedPeakV1: Codable, Equatable, Sendable {
    public enum Reason: String, Codable, Sendable {
        case notchCap = "notch_cap"
        case perOctaveLimit = "per_octave_limit"
        case duplicateCode = "duplicate_code"
    }

    public let frequencyHz: Double
    public let reason: Reason

    public init(frequencyHz: Double, reason: Reason) {
        self.frequencyHz = frequencyHz
        self.reason = reason
    }
}

/// Sanitized `dcx.notch-plan/v1`. The raw plan stays in the helper's store,
/// addressed by `planDigest`, so a later round can name it as the prior plan.
public struct NotchPlanSummaryV1: Codable, Equatable, Sendable {
    public let schemaVersion: String
    public let planDigest: String
    public let baselineSnapshotDigest: String
    public let measurementDigest: String
    public let targetOutput: UInt8
    public let parameterChannel: UInt8
    public let eqEnabledBefore: Bool
    public let eqCountBefore: UInt8
    public let operatorBandCount: UInt8
    public let notches: [PlannedNotchV1]
    public let dropped: [DroppedPeakV1]

    public init(
        planDigest: String,
        baselineSnapshotDigest: String,
        measurementDigest: String,
        eqEnabledBefore: Bool,
        eqCountBefore: UInt8,
        operatorBandCount: UInt8,
        notches: [PlannedNotchV1],
        dropped: [DroppedPeakV1]
    ) throws {
        schemaVersion = FeedbackNotchContract.notchPlanSchemaVersion
        self.planDigest = planDigest
        self.baselineSnapshotDigest = baselineSnapshotDigest
        self.measurementDigest = measurementDigest
        targetOutput = FeedbackNotchContract.targetOutput
        parameterChannel = FeedbackNotchContract.parameterChannel
        self.eqEnabledBefore = eqEnabledBefore
        self.eqCountBefore = eqCountBefore
        self.operatorBandCount = operatorBandCount
        self.notches = notches
        self.dropped = dropped
        try validate()
    }

    /// The structural rules of `NotchPlanV1::verify`: O4 only, consecutive
    /// bands directly above the operator bands, cut-only bells, bounded lists.
    public func validate() throws {
        try BridgeDigest.validate(planDigest)
        try BridgeDigest.validate(baselineSnapshotDigest)
        try BridgeDigest.validate(measurementDigest)
        let bands = PeqAddressV2.bandCount
        guard schemaVersion == FeedbackNotchContract.notchPlanSchemaVersion,
              targetOutput == FeedbackNotchContract.targetOutput,
              parameterChannel == FeedbackNotchContract.parameterChannel,
              eqCountBefore <= bands,
              operatorBandCount < bands,
              !notches.isEmpty,
              Int(operatorBandCount) + notches.count <= Int(bands),
              dropped.count <= FeedbackNotchContract.maximumPeaks else {
            throw BridgeValidationError.invalidNotchPlan("plan header")
        }
        for (offset, notch) in notches.enumerated() {
            guard Int(notch.band) == Int(operatorBandCount) + offset + 1,
                  notch.kindCode == PeqAddressV2.bellKindCode,
                  notch.slopeCode == 0,
                  notch.qCode <= 40,
                  notch.gainCode < PeqAddressV2.unityGainCode,
                  notch.frequencyCode <= 320,
                  notch.occurrences >= 1,
                  Self.inMeasuredRange(notch.frequencyHz),
                  notch.levelDb.map({ $0.isFinite && (-200...200).contains($0) }) ?? true else {
                throw BridgeValidationError.invalidNotchPlan("notch placement or shape")
            }
        }
        guard dropped.allSatisfy({ Self.inMeasuredRange($0.frequencyHz) }) else {
            throw BridgeValidationError.invalidNotchPlan("dropped peak frequency")
        }
    }

    /// Exact ordered actions the planner writes: every notch band, then the
    /// band count, then PEQ enable.
    public var expectedActions: [DirectParameterActionV2] {
        notches.flatMap { $0.actions(channel: parameterChannel) } + [
            .init(
                channel: parameterChannel,
                parameter: PeqAddressV2.eqCountParameter,
                value: UInt16(Int(operatorBandCount) + notches.count)
            ),
            .init(channel: parameterChannel, parameter: PeqAddressV2.eqEnabledParameter, value: 1),
        ]
    }

    /// A staged v2 profile shows this plan only when it carries exactly the
    /// plan's actions on O4.
    public func requireStaged(by desired: DesiredProfileV2) throws {
        try validate()
        let bank = try desired.bank()
        guard bank.targetOutput == targetOutput,
              bank.parameterChannel == parameterChannel,
              bank.actions == expectedActions else {
            throw BridgeValidationError.invalidNotchPlan("staged profile does not carry exactly this plan")
        }
    }

    private static func inMeasuredRange(_ value: Double) -> Bool {
        value.isFinite && (20...20_000).contains(value)
    }
}

public struct FeedbackPlanRequest: Codable, Equatable, Sendable {
    public let target: DCXTargetReference
    /// Complete helper-stored snapshot the plan is computed against.
    public let baseline: SnapshotV1
    public let measurement: FeedbackMeasurementInputV1
    /// Plan most recently applied to O4, for recurrence; the helper resolves
    /// it from its own store.
    public let priorPlanDigest: String?
    public let profileID: String
    public let revision: String

    public init(
        target: DCXTargetReference,
        baseline: SnapshotV1,
        measurement: FeedbackMeasurementInputV1,
        priorPlanDigest: String? = nil,
        profileID: String,
        revision: String
    ) throws {
        self.target = target
        self.baseline = baseline
        self.measurement = measurement
        self.priorPlanDigest = priorPlanDigest
        self.profileID = profileID
        self.revision = revision
        try validate()
    }

    public func validate() throws {
        try target.validate()
        try baseline.validate()
        guard baseline.target == target else {
            throw BridgeValidationError.invalidTarget("baseline target does not match request")
        }
        try measurement.validate()
        if let priorPlanDigest { try BridgeDigest.validate(priorPlanDigest) }
        guard BridgeText.boundedASCII(profileID), BridgeText.boundedASCII(revision) else {
            throw BridgeValidationError.invalidProfile("profile identity must be 1...128 ASCII bytes")
        }
    }
}

public struct FeedbackPlanResponse: Codable, Equatable, Sendable {
    public let measurement: FeedbackMeasurementSummaryV1
    public let plan: NotchPlanSummaryV1
    public let desired: DesiredProfileV2
    /// Present exactly when the helper ran `feedback import` on source text.
    public let importReceipt: CommandReceiptV1?
    public let planReceipt: CommandReceiptV1
    public let profileReceipt: CommandReceiptV1

    public init(
        measurement: FeedbackMeasurementSummaryV1,
        plan: NotchPlanSummaryV1,
        desired: DesiredProfileV2,
        importReceipt: CommandReceiptV1?,
        planReceipt: CommandReceiptV1,
        profileReceipt: CommandReceiptV1
    ) {
        self.measurement = measurement
        self.plan = plan
        self.desired = desired
        self.importReceipt = importReceipt
        self.planReceipt = planReceipt
        self.profileReceipt = profileReceipt
    }

    public func validate() throws {
        try measurement.validate()
        try plan.requireStaged(by: desired)
        guard plan.measurementDigest == measurement.digest else {
            throw BridgeValidationError.invalidNotchPlan("plan is not bound to the measurement")
        }
    }

    /// Bind the reply to the exact request: baseline, measurement identity,
    /// profile identity, and whether an import ran.
    public func validate(for request: FeedbackPlanRequest) throws {
        try validate()
        guard plan.baselineSnapshotDigest == request.baseline.digest,
              desired.profileID == request.profileID,
              desired.revision == request.revision,
              (importReceipt != nil) == (request.measurement.kind != .measurement) else {
            throw BridgeValidationError.invalidNotchPlan("plan reply does not answer this request")
        }
        switch request.measurement.kind {
        case .measurement:
            guard measurement.digest == request.measurement.documentDigest else {
                throw BridgeValidationError.invalidNotchPlan("measurement digest differs from the request")
            }
        case .frequencyList, .rewGenericEq:
            guard let text = request.measurement.text,
                  measurement.sourceDigest == FeedbackNotchContract.sourceDigest(text),
                  measurement.source.rawValue == request.measurement.kind.rawValue else {
                throw BridgeValidationError.invalidNotchPlan("measurement source differs from the request")
            }
        }
    }
}
