import DCXLogicBridge
import Foundation

public struct StagedProjectStateV1: Codable, Equatable, Sendable {
    public static let schemaVersion = "dcx.logic-project-state/v1"

    public let schemaVersion: String
    public let target: DCXTargetReference
    public let desired: DesiredProfileV1

    public init(target: DCXTargetReference, desired: DesiredProfileV1) {
        schemaVersion = Self.schemaVersion
        self.target = target
        self.desired = desired
    }

    public func validate() throws {
        guard schemaVersion == Self.schemaVersion else {
            throw BridgeValidationError.invalidProfile("unsupported Logic project-state schema")
        }
        try target.validate()
        try desired.validate()
    }
}

/// Complete state persisted in Logic's AU full-state dictionary. Restoring this
/// value is a pure model operation: it never creates an App Group client or
/// contacts the helper, child process, serial device, or render path.
public struct DCXControlPersistedStateV1: Codable, Equatable, Sendable {
    public static let schemaVersion = "dcx.logic-control-state/v1"

    public let schemaVersion: String
    public let projectState: StagedProjectStateV1?
    public let currentSnapshot: SnapshotV1?
    public let diff: SemanticDiffV1?
    public let transactionID: String?
    public let rollbackBaseline: SnapshotV1?
    public let deviceStateUncertain: Bool

    public init(
        projectState: StagedProjectStateV1?,
        currentSnapshot: SnapshotV1?,
        diff: SemanticDiffV1?,
        transactionID: String?,
        rollbackBaseline: SnapshotV1?,
        deviceStateUncertain: Bool
    ) {
        schemaVersion = Self.schemaVersion
        self.projectState = projectState
        self.currentSnapshot = currentSnapshot
        self.diff = diff
        self.transactionID = transactionID
        self.rollbackBaseline = rollbackBaseline
        self.deviceStateUncertain = deviceStateUncertain
    }

    public func validate() throws {
        guard schemaVersion == Self.schemaVersion else {
            throw DCXControlStateError.unsupportedSchema
        }
        guard let projectState else {
            guard currentSnapshot == nil, diff == nil, transactionID == nil,
                  rollbackBaseline == nil, !deviceStateUncertain else {
                throw DCXControlStateError.invalidProjectBinding
            }
            return
        }

        try projectState.validate()
        if let currentSnapshot {
            try currentSnapshot.validate()
            guard currentSnapshot.target == projectState.target else {
                throw DCXControlStateError.invalidSnapshotBinding
            }
        }
        if let rollbackBaseline {
            try rollbackBaseline.validate()
            guard rollbackBaseline.target == projectState.target else {
                throw DCXControlStateError.invalidRollbackBinding
            }
        }
        if let diff {
            try diff.validate()
            guard diff.changes.count <= 1,
                  diff.desiredProfileDigest == projectState.desired.digest,
                  currentSnapshot != nil else {
                throw DCXControlStateError.invalidDiffBinding
            }
            if let rollbackBaseline {
                guard diff.baselineSnapshotDigest == rollbackBaseline.digest else {
                    throw DCXControlStateError.invalidDiffBinding
                }
            } else {
                guard transactionID == nil,
                      diff.baselineSnapshotDigest == currentSnapshot?.digest else {
                    throw DCXControlStateError.invalidDiffBinding
                }
            }
        }

        if let transactionID {
            guard let diff, let rollbackBaseline, currentSnapshot != nil,
                  transactionID == diff.applyPlanDigest,
                  rollbackBaseline.digest == diff.baselineSnapshotDigest else {
                throw DCXControlStateError.invalidTransactionBinding
            }
        } else {
            guard rollbackBaseline == nil, !deviceStateUncertain else {
                throw DCXControlStateError.invalidTransactionBinding
            }
            if let diff {
                guard currentSnapshot?.digest == diff.baselineSnapshotDigest else {
                    throw DCXControlStateError.invalidDiffBinding
                }
            }
        }

        if deviceStateUncertain, transactionID == nil {
            throw DCXControlStateError.invalidTransactionBinding
        }
    }
}

/// In-memory AU project and recovery state. Every mutation preserves the same
/// bindings checked during full-state restoration.
public final class DCXControlState: @unchecked Sendable {
    private let lock = NSLock()
    private var projectState: StagedProjectStateV1?
    private var currentSnapshot: SnapshotV1?
    private var currentDiff: SemanticDiffV1?
    private var lastTransactionID: String?
    private var rollbackBaseline: SnapshotV1?
    private var deviceStateUncertain = false

    public init() {}

    public func stage(_ state: StagedProjectStateV1?) throws {
        try state?.validate()
        lock.lock()
        projectState = state
        currentSnapshot = nil
        currentDiff = nil
        lastTransactionID = nil
        rollbackBaseline = nil
        deviceStateUncertain = false
        lock.unlock()
    }

    public func reset() {
        lock.lock()
        projectState = nil
        currentSnapshot = nil
        currentDiff = nil
        lastTransactionID = nil
        rollbackBaseline = nil
        deviceStateUncertain = false
        lock.unlock()
    }

    public func stagedProjectState() -> StagedProjectStateV1? {
        lock.lock()
        defer { lock.unlock() }
        return projectState
    }

    public func accept(snapshot: SnapshotV1, validSearchResponses: UInt8) throws {
        let snapshot = try snapshot.reporting(validSearchResponses: validSearchResponses)
        lock.lock()
        defer { lock.unlock() }
        guard let projectState, snapshot.target == projectState.target else {
            throw DCXControlStateError.invalidSnapshotBinding
        }
        currentSnapshot = snapshot
        if lastTransactionID == nil {
            currentDiff = nil
            rollbackBaseline = nil
        }
        deviceStateUncertain = false
    }

    public func accept(diff: SemanticDiffV1) throws {
        try diff.validate()
        lock.lock()
        defer { lock.unlock() }
        guard let projectState, let currentSnapshot,
              lastTransactionID == nil, rollbackBaseline == nil,
              diff.changes.count <= 1,
              diff.baselineSnapshotDigest == currentSnapshot.digest,
              diff.desiredProfileDigest == projectState.desired.digest else {
            throw DCXControlStateError.invalidDiffBinding
        }
        currentDiff = diff
        lastTransactionID = nil
        rollbackBaseline = nil
        deviceStateUncertain = false
    }

    /// Persist rollback recovery before an apply request crosses IPC. From this
    /// point onward a timeout or lost response is conservatively uncertain.
    public func beginApplyAttempt(transactionID: String, baseline: SnapshotV1) throws {
        try baseline.validate()
        lock.lock()
        defer { lock.unlock() }
        guard let projectState, let currentDiff, let currentSnapshot,
              lastTransactionID == nil, rollbackBaseline == nil,
              baseline.target == projectState.target,
              baseline == currentSnapshot,
              baseline.digest == currentDiff.baselineSnapshotDigest,
              transactionID == currentDiff.applyPlanDigest else {
            throw DCXControlStateError.invalidTransactionBinding
        }
        lastTransactionID = transactionID
        rollbackBaseline = baseline
        deviceStateUncertain = true
    }

    public func acceptApply(
        transactionID: String,
        baseline: SnapshotV1,
        desiredSnapshotDigest: String,
        readback: SnapshotV1?,
        validSearchResponses: UInt8
    ) throws {
        try baseline.validate()
        let readback = try readback?.reporting(validSearchResponses: validSearchResponses)
        lock.lock()
        defer { lock.unlock() }
        guard let projectState, let currentDiff, let currentSnapshot,
              lastTransactionID == transactionID,
              rollbackBaseline == baseline,
              baseline.target == projectState.target,
              baseline == currentSnapshot,
              baseline.digest == currentDiff.baselineSnapshotDigest,
              transactionID == currentDiff.applyPlanDigest,
              desiredSnapshotDigest == currentDiff.desiredSnapshotDigest,
              readback?.target == nil || readback?.target == projectState.target else {
            throw DCXControlStateError.invalidTransactionBinding
        }
        lastTransactionID = transactionID
        rollbackBaseline = baseline
        if let readback { self.currentSnapshot = readback }
        deviceStateUncertain = readback == nil
    }

    public func acceptReadback(
        transactionID: String,
        snapshot: SnapshotV1,
        validSearchResponses: UInt8
    ) throws {
        let snapshot = try snapshot.reporting(validSearchResponses: validSearchResponses)
        lock.lock()
        defer { lock.unlock() }
        guard let projectState, let currentDiff,
              transactionID == lastTransactionID,
              transactionID == currentDiff.applyPlanDigest,
              snapshot.target == projectState.target else {
            throw DCXControlStateError.invalidTransactionBinding
        }
        currentSnapshot = snapshot
        deviceStateUncertain = false
    }

    /// A rollback can write before its response crosses IPC. Persist the
    /// unresolved state while retaining the original transaction and baseline.
    public func beginRollbackAttempt(transactionID: String, baseline: SnapshotV1) throws {
        try baseline.validate()
        lock.lock()
        defer { lock.unlock() }
        guard let projectState, let currentDiff,
              transactionID == lastTransactionID,
              rollbackBaseline == baseline,
              baseline.target == projectState.target,
              baseline.digest == currentDiff.baselineSnapshotDigest,
              transactionID == currentDiff.applyPlanDigest else {
            throw DCXControlStateError.invalidRollbackBinding
        }
        deviceStateUncertain = true
    }

    public func acceptRollback(
        transactionID: String,
        baselineDigest: String,
        restored: SnapshotV1?,
        equalsBaseline: Bool,
        validSearchResponses: UInt8
    ) throws {
        let restored = try restored?.reporting(validSearchResponses: validSearchResponses)
        lock.lock()
        defer { lock.unlock() }
        guard let projectState, let currentDiff, let lastTransactionID, let rollbackBaseline,
              transactionID == lastTransactionID,
              transactionID == currentDiff.applyPlanDigest,
              baselineDigest == rollbackBaseline.digest,
              rollbackBaseline.digest == currentDiff.baselineSnapshotDigest,
              restored?.target == nil || restored?.target == projectState.target,
              equalsBaseline == (restored?.digest == rollbackBaseline.digest) else {
            throw DCXControlStateError.invalidRollbackBinding
        }
        if let restored { currentSnapshot = restored }
        deviceStateUncertain = restored == nil
        if equalsBaseline {
            self.currentDiff = nil
            self.lastTransactionID = nil
            self.rollbackBaseline = nil
            deviceStateUncertain = false
        }
    }

    public func persistedState() throws -> DCXControlPersistedStateV1 {
        let view = view()
        let state = DCXControlPersistedStateV1(
            projectState: view.projectState,
            currentSnapshot: view.currentSnapshot,
            diff: view.diff,
            transactionID: view.transactionID,
            rollbackBaseline: view.rollbackBaseline,
            deviceStateUncertain: view.deviceStateUncertain
        )
        try state.validate()
        return state
    }

    public func restore(_ state: DCXControlPersistedStateV1) throws {
        try state.validate()
        lock.lock()
        projectState = state.projectState
        currentSnapshot = state.currentSnapshot
        currentDiff = state.diff
        lastTransactionID = state.transactionID
        rollbackBaseline = state.rollbackBaseline
        deviceStateUncertain = state.deviceStateUncertain
        lock.unlock()
    }

    public func view() -> DCXControlStateView {
        lock.lock()
        defer { lock.unlock() }
        return .init(
            projectState: projectState,
            currentSnapshot: currentSnapshot,
            diff: currentDiff,
            transactionID: lastTransactionID,
            rollbackBaseline: rollbackBaseline,
            deviceStateUncertain: deviceStateUncertain
        )
    }
}

public struct DCXControlStateView: Sendable {
    public let projectState: StagedProjectStateV1?
    public let currentSnapshot: SnapshotV1?
    public let diff: SemanticDiffV1?
    public let transactionID: String?
    public let rollbackBaseline: SnapshotV1?
    public let deviceStateUncertain: Bool
}

public enum DCXControlStateError: Error, Equatable, Sendable {
    case unsupportedSchema
    case invalidProjectBinding
    case invalidSnapshotBinding
    case invalidDiffBinding
    case invalidTransactionBinding
    case invalidRollbackBinding
}

private extension SnapshotV1 {
    func reporting(validSearchResponses: UInt8) throws -> SnapshotV1 {
        let identity = DeviceIdentityV1(
            manufacturer: identity.manufacturer,
            model: identity.model,
            deviceAddress: identity.deviceAddress,
            selectedBaud: identity.selectedBaud,
            validSearchResponses: validSearchResponses
        )
        try identity.validate()
        return try SnapshotV1(
            target: target,
            identity: identity,
            capturedAt: capturedAt,
            digest: digest,
            complete: complete,
            sectionDigests: sectionDigests
        )
    }
}
