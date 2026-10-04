import DCXLogicBridge
import Foundation

public final class DCXHelperCoordinator: @unchecked Sendable {
    private let locations: AppGroupLocations
    private var configuration: HelperConfigurationV1?
    private var configurationError: Error?
    private let coreMIDI: CoreMIDIPresentation
    private let runner = DCXCTLProcessRunner()
    private let snapshotStore: RawSnapshotStore
    private let planStore: RawPlanStore
    private let recoveryLeaseStore: MutationRecoveryLeaseStore
    private let stateLock = NSLock()
    private var foreground = false
    private var activeTransactionID: String?
    private var recoveryLease: MutationRecoveryLeaseV1?
    private var recoveryCompletion: MutationRecoveryCompletionV1?
    private var recoveryLeaseError: Error?

    public init(
        locations: AppGroupLocations,
        configuration: Result<HelperConfigurationV1, Error>,
        coreMIDI: CoreMIDIPresentation
    ) {
        self.locations = locations
        snapshotStore = .init(root: locations.snapshotRootURL)
        planStore = .init(root: locations.planRootURL)
        let recoveryLeaseStore = MutationRecoveryLeaseStore(root: locations.planRootURL)
        self.recoveryLeaseStore = recoveryLeaseStore
        switch configuration {
        case let .success(value):
            self.configuration = value
            configurationError = nil
        case let .failure(error):
            self.configuration = nil
            configurationError = error
        }
        self.coreMIDI = coreMIDI
        recoveryLease = nil
        recoveryCompletion = nil
        recoveryLeaseError = nil
        do {
            let processLock = try recoveryLeaseStore.acquireExclusiveLock()
            defer { processLock.release() }
            try reloadRecoveryStateFromStore()
        } catch {
            recoveryLease = nil
            recoveryCompletion = nil
            recoveryLeaseError = error
        }
    }

    public func setForeground(_ value: Bool) {
        stateLock.lock()
        foreground = value
        stateLock.unlock()
    }

    @discardableResult
    func replaceConfiguration(
        load: () -> Result<HelperConfigurationV1, Error>
    ) throws -> HelperConfigurationV1 {
        let processLock = try recoveryLeaseStore.acquireExclusiveLock()
        defer { processLock.release() }
        stateLock.lock()
        defer { stateLock.unlock() }
        try reloadRecoveryStateFromStore()
        let result = load()
        try requireConfigurationReplacementAllowed(result)
        switch result {
        case let .success(value):
            configuration = value
            configurationError = nil
        case let .failure(error):
            configuration = nil
            configurationError = error
        }
        return try result.get()
    }

    /// Serialize persistent configuration replacement with Apply admission.
    /// The closure runs while the coordinator lock prevents a lease from being
    /// created against a configuration that is concurrently being replaced.
    func installConfiguration(
        _ candidate: HelperConfigurationV1,
        persistAndReload: () throws -> HelperConfigurationV1
    ) throws -> HelperConfigurationV1 {
        let processLock = try recoveryLeaseStore.acquireExclusiveLock()
        defer { processLock.release() }
        stateLock.lock()
        defer { stateLock.unlock() }
        try reloadRecoveryStateFromStore()
        try requireConfigurationReplacementAllowed(.success(candidate))
        let loaded = try persistAndReload()
        guard loaded == candidate else {
            throw HelperConfigurationError.invalidConfigurationFile
        }
        configuration = loaded
        configurationError = nil
        return loaded
    }

    func currentConfiguration() -> HelperConfigurationV1? {
        stateLock.lock()
        defer { stateLock.unlock() }
        return configuration
    }

    public func handle(_ request: BridgeRequest) -> BridgeResponse {
        do {
            try request.body.validate()
        } catch {
            return preAdmissionFailure(
                request,
                .invalidRequest,
                "request failed bridge validation",
                false
            )
        }
        if case .helperStatus = request.body {
            return BridgeResponse(requestID: request.requestID, body: .helperStatus(status()))
        }
        guard isForeground else {
            return preAdmissionFailure(
                request,
                .helperNotForeground,
                "helper must be open and foreground",
                true
            )
        }

        let target: DCXTargetReference
        switch request.body {
        case .helperStatus:
            return preAdmissionFailure(
                request,
                .invalidRequest,
                "status request reached command dispatch",
                false
            )
        case let .identitySearch(value): target = value.target
        case let .snapshotCapture(value): target = value.target
        case let .diffPreview(value): target = value.target
        case let .apply(value): target = value.target
        case let .readback(value): target = value.target
        case let .rollback(value): target = value.target
        }

        var processLock: MutationRecoveryProcessLock?
        do {
            processLock = try recoveryLeaseStore.acquireExclusiveLock()
            stateLock.lock()
            do {
                try reloadRecoveryStateFromStore()
                if recoveryLease == nil {
                    reloadCurrentConfigurationFromStore()
                }
                stateLock.unlock()
            } catch {
                stateLock.unlock()
                throw error
            }
        } catch MutationRecoveryLeaseError.lockBusy {
            return preAdmissionFailure(
                request,
                .operationInFlight,
                "another helper or child owns the device operation lock",
                true
            )
        } catch {
            return preAdmissionFailure(
                request,
                .helperNotConfigured,
                "durable mutation recovery state is unavailable",
                false
            )
        }
        defer { processLock?.release() }

        let configuration: HelperConfigurationV1
        do {
            configuration = try executionConfiguration(for: request)
            try configuration.require(request.operation, target: target)
        } catch MutationRecoveryStateError.applyAlreadyActive {
            return preAdmissionFailure(
                request,
                .operationUnavailable,
                "the durable mutation recovery must complete before another Apply",
                false
            )
        } catch MutationRecoveryStateError.recoveryUnavailable {
            return preAdmissionFailure(
                request,
                .operationUnavailable,
                "durable mutation recovery authority is unavailable",
                false
            )
        } catch MutationRecoveryStateError.recoveryInProgress {
            return preAdmissionFailure(
                request,
                .operationUnavailable,
                "only recovery-bound Readback or Rollback is available",
                false
            )
        } catch HelperConfigurationError.targetNotAllowlisted {
            return preAdmissionFailure(
                request,
                .targetNotAllowlisted,
                "requested target is not configured",
                false
            )
        } catch HelperConfigurationError.operationUnavailable {
            return preAdmissionFailure(
                request,
                .operationUnavailable,
                "operation is not enabled",
                false
            )
        } catch {
            return preAdmissionFailure(
                request,
                .helperNotConfigured,
                "helper configuration is unavailable",
                false
            )
        }
        if case .diffPreview = request.body {
            processLock?.release()
            processLock = nil
        }

        let activeID = UUID().uuidString
        stateLock.lock()
        guard activeTransactionID == nil else {
            stateLock.unlock()
            return preAdmissionFailure(
                request,
                .operationInFlight,
                "one helper transaction is already active",
                true
            )
        }
        activeTransactionID = activeID
        stateLock.unlock()
        defer {
            stateLock.lock()
            activeTransactionID = nil
            stateLock.unlock()
        }

        var applyAdmitted = false
        do {
            let workspace = try TransactionWorkspace(root: locations.transactionRootURL)
            defer { workspace.remove() }
            let invocation = try buildInvocation(
                request: request,
                configuration: configuration,
                workspace: workspace
            )
            try admitMutation(
                request,
                configuration: configuration,
                applyAdmitted: &applyAdmitted
            )
            let result = try runner.run(
                invocation,
                timeoutSeconds: configuration.childTimeoutSeconds,
                mutationLock: processLock
            )
            if let failure = result.failurePayload() {
                return BridgeResponse(
                    requestID: request.requestID,
                    operation: request.operation,
                    error: failure
                )
            }
            let response = try decode(request: request, result: result)
            try finishMutation(request, response: response)
            return response
        } catch DCXCTLRunnerError.timedOut {
            return failure(request, .childTimedOut, "dcxctl exceeded its configured deadline", true)
        } catch DCXCTLRunnerError.launchFailed {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .childFailed,
                "bundled dcxctl could not be launched",
                false
            )
        } catch DCXCTLRunnerError.outputTooLarge {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .childFailed,
                "dcxctl exceeded its bounded output limit; device state is not verified",
                false
            )
        } catch MutationRecoveryStateError.applyAlreadyActive {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .operationUnavailable,
                "the durable mutation recovery must complete before another Apply",
                false
            )
        } catch MutationRecoveryStateError.bindingMismatch {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .invalidRequest,
                "request does not match the durable mutation recovery binding",
                false
            )
        } catch MutationRecoveryStateError.configurationChanged {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .operationUnavailable,
                "helper configuration changed before mutation admission; retry",
                true
            )
        } catch is MutationRecoveryLeaseError {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .helperNotConfigured,
                "durable mutation recovery state is unavailable",
                false
            )
        } catch ChildResponseError.requestPlanBinding {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .invalidRequest,
                "stored plan does not match the request bindings",
                false
            )
        } catch HelperConfigurationError.executableUnavailable {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .helperNotConfigured,
                "bundled dcxctl is unavailable",
                false
            )
        } catch {
            return commandFailure(
                request,
                applyAdmitted: applyAdmitted,
                .malformedChildResponse,
                "bounded command did not return its expected schema",
                false
            )
        }
    }

    private var isForeground: Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        return foreground
    }

    /// Reload only while the caller owns the process-shared operation lock.
    /// A valid active lease suppresses terminal history, including a stale or
    /// partially replaced completion file from an earlier transaction.
    private func reloadRecoveryStateFromStore() throws {
        do {
            let durableLease = try recoveryLeaseStore.load()
            recoveryLease = durableLease
            if durableLease != nil {
                recoveryCompletion = nil
            } else {
                recoveryCompletion = try recoveryLeaseStore.loadCompletion()
            }
            recoveryLeaseError = nil
        } catch {
            recoveryLease = nil
            recoveryCompletion = nil
            recoveryLeaseError = error
            throw error
        }
    }

    /// With no active lease, the fixed App Group file is the cross-process
    /// configuration authority. Recovery execution never calls this path and
    /// remains pinned to the lease rather than overwriting mutable config.
    private func reloadCurrentConfigurationFromStore() {
        do {
            configuration = try HelperConfigurationV1.load(
                from: locations.helperConfigurationURL
            )
            configurationError = nil
        } catch {
            configuration = nil
            configurationError = error
        }
    }

    private func requireConfigurationReplacementAllowed(
        _ result: Result<HelperConfigurationV1, Error>
    ) throws {
        guard recoveryLeaseError == nil else {
            throw HelperConfigurationError.recoveryConfigurationPinned
        }
        guard let recoveryLease else { return }
        guard case let .success(candidate) = result,
              candidate == (try recoveryLease.pinnedConfiguration()) else {
            throw HelperConfigurationError.recoveryConfigurationPinned
        }
    }

    private func executionConfiguration(
        for request: BridgeRequest
    ) throws -> HelperConfigurationV1 {
        stateLock.lock()
        defer { stateLock.unlock() }
        if recoveryLeaseError != nil {
            throw MutationRecoveryStateError.recoveryUnavailable
        }
        switch request.body {
        case .helperStatus:
            throw HelperConfigurationError.operationUnavailable
        case .apply:
            guard recoveryLease == nil else {
                throw MutationRecoveryStateError.applyAlreadyActive
            }
            guard let configuration else {
                throw HelperConfigurationError.invalidConfigurationFile
            }
            return configuration
        case .readback, .rollback:
            guard let recoveryLease else {
                throw MutationRecoveryStateError.recoveryUnavailable
            }
            return try recoveryLease.pinnedConfiguration()
        case .identitySearch, .snapshotCapture, .diffPreview:
            guard recoveryLease == nil else {
                throw MutationRecoveryStateError.recoveryInProgress
            }
            guard let configuration else {
                throw HelperConfigurationError.invalidConfigurationFile
            }
            return configuration
        }
    }

    /// Persist mutation authority under the same lock used by configuration
    /// replacement, immediately before the child process can start.
    private func admitMutation(
        _ request: BridgeRequest,
        configuration capturedConfiguration: HelperConfigurationV1,
        applyAdmitted: inout Bool
    ) throws {
        guard BridgeOperation.mutationCapabilities.contains(request.operation) else { return }
        stateLock.lock()
        defer { stateLock.unlock() }
        guard recoveryLeaseError == nil else {
            throw MutationRecoveryStateError.recoveryUnavailable
        }
        switch request.body {
        case let .apply(value):
            guard recoveryLease == nil else {
                throw MutationRecoveryStateError.applyAlreadyActive
            }
            guard configuration == capturedConfiguration else {
                throw MutationRecoveryStateError.configurationChanged
            }
            let lease = try MutationRecoveryLeaseV1(
                configuration: capturedConfiguration,
                apply: value
            )
            do {
                try recoveryLeaseStore.persistNew(lease)
            } catch MutationRecoveryLeaseError.activeLease {
                do {
                    guard let durable = try recoveryLeaseStore.load() else {
                        throw MutationRecoveryLeaseError.invalidLease
                    }
                    recoveryLease = durable
                    recoveryCompletion = nil
                    recoveryLeaseError = nil
                } catch {
                    recoveryLease = nil
                    recoveryCompletion = nil
                    recoveryLeaseError = error
                }
                throw MutationRecoveryStateError.applyAlreadyActive
            } catch {
                let publicationError = error
                do {
                    if let durable = try recoveryLeaseStore.load() {
                        recoveryLease = durable
                        recoveryCompletion = nil
                        recoveryLeaseError = nil
                        applyAdmitted = durable == lease
                    } else {
                        recoveryLease = nil
                        applyAdmitted = false
                    }
                } catch {
                    // Once exclusive publication may have happened, an
                    // unreadable pathname cannot prove non-admission.
                    recoveryLease = nil
                    recoveryCompletion = nil
                    recoveryLeaseError = error
                    applyAdmitted = true
                }
                throw publicationError
            }
            recoveryLease = lease
            recoveryCompletion = nil
            recoveryLeaseError = nil
            applyAdmitted = true
        case let .readback(value):
            guard let recoveryLease,
                  recoveryLease.matches(readback: value),
                  capturedConfiguration == (try recoveryLease.pinnedConfiguration()) else {
                throw MutationRecoveryStateError.bindingMismatch
            }
        case let .rollback(value):
            guard let recoveryLease,
                  recoveryLease.matches(rollback: value),
                  capturedConfiguration == (try recoveryLease.pinnedConfiguration()) else {
                throw MutationRecoveryStateError.bindingMismatch
            }
        case .helperStatus, .identitySearch, .snapshotCapture, .diffPreview:
            return
        }
    }

    /// A transaction-bound readback or rollback is terminal only when its
    /// decoded snapshot equals the immutable baseline. All other outcomes keep
    /// the durable lease.
    private func finishMutation(
        _ request: BridgeRequest,
        response: BridgeResponse
    ) throws {
        guard let body = response.body else { return }
        stateLock.lock()
        defer { stateLock.unlock() }
        guard let recoveryLease else { return }
        let verifiedBaseline: SnapshotV1
        switch (request.body, body) {
        case let (.readback(requestValue), .readback(result)):
            guard recoveryLease.matches(readback: requestValue),
                  result.transactionID == recoveryLease.transactionID else {
                throw MutationRecoveryStateError.bindingMismatch
            }
            guard result.snapshot.digest == recoveryLease.baselineDigest else { return }
            verifiedBaseline = result.snapshot
        case let (.rollback(requestValue), .rollback(result)):
            guard recoveryLease.matches(rollback: requestValue),
                  result.transactionID == recoveryLease.transactionID,
                  result.baselineDigest == recoveryLease.baselineDigest else {
                throw MutationRecoveryStateError.bindingMismatch
            }
            guard result.equalsBaseline,
                  let restored = result.restored,
                  restored.digest == recoveryLease.baselineDigest else { return }
            verifiedBaseline = restored
        default:
            return
        }
        let completion = try recoveryLeaseStore.complete(
            recoveryLease,
            verifiedBaseline: verifiedBaseline
        )
        self.recoveryLease = nil
        recoveryCompletion = completion
        recoveryLeaseError = nil
    }

    private func status() -> HelperStatusResponse {
        var recoveryUnavailable = false
        do {
            let processLock = try recoveryLeaseStore.acquireExclusiveLock()
            defer { processLock.release() }
            stateLock.lock()
            do {
                try reloadRecoveryStateFromStore()
                if recoveryLease == nil {
                    reloadCurrentConfigurationFromStore()
                }
                stateLock.unlock()
            } catch {
                stateLock.unlock()
                throw error
            }
        } catch MutationRecoveryLeaseError.lockBusy {
            recoveryUnavailable = true
        } catch {
            stateLock.lock()
            recoveryLease = nil
            recoveryCompletion = nil
            recoveryLeaseError = error
            stateLock.unlock()
            recoveryUnavailable = true
        }

        stateLock.lock()
        let foreground = foreground
        let active = activeTransactionID
        let configuration = configuration
        let unavailable = recoveryUnavailable || recoveryLeaseError != nil
        let recovery = unavailable ? nil : recoveryLease.map {
            HelperRecoveryStatusV1(
                transactionID: $0.transactionID,
                target: $0.target,
                capabilities: [.readback, .rollback],
                baseline: $0.baseline,
                desiredSnapshotDigest: $0.desiredSnapshotDigest,
                rollbackPlanDigest: $0.rollbackPlanDigest
            )
        }
        let completion = unavailable || recoveryLease != nil ? nil : recoveryCompletion.map {
            HelperRecoveryCompletionStatusV1(
                transactionID: $0.transactionID,
                target: $0.target,
                baseline: $0.baseline,
                verifiedBaseline: $0.verifiedBaseline,
                desiredSnapshotDigest: $0.desiredSnapshotDigest,
                rollbackPlanDigest: $0.rollbackPlanDigest
            )
        }
        let recoveryBlocksOperations = recoveryLease != nil || unavailable
        let enabledOperations = recoveryBlocksOperations
            ? []
            : configuration?.enabledOperations ?? []
        _ = configurationError
        _ = recoveryLeaseError
        stateLock.unlock()
        return .init(
            foreground: foreground,
            configured: configuration != nil,
            target: configuration?.target,
            capabilities: [.helperStatus] + enabledOperations,
            coreMIDI: coreMIDI.bridgeStatus(),
            activeTransactionID: active,
            recovery: recovery,
            completion: completion,
            recoveryUnavailable: unavailable
        )
    }

    private func buildInvocation(
        request: BridgeRequest,
        configuration: HelperConfigurationV1,
        workspace: TransactionWorkspace
    ) throws -> DCXCTLInvocation {
        let executable = try configuration.resolveExecutable()
        let targetArguments = [
            "--tty", configuration.ttyPath,
            "--expected-device", String(configuration.target.expectedDeviceAddress),
        ]
        let arguments: [String]
        switch request.body {
        case .helperStatus:
            throw HelperConfigurationError.operationUnavailable
        case .identitySearch:
            arguments = ["discovery", "live-search"] + targetArguments
        case .snapshotCapture:
            arguments = ["control", "snapshot"] + targetArguments
        case let .diffPreview(value):
            let (snapshot, _) = try snapshotStore.load(
                digest: value.baseline.digest,
                expectedDevice: value.target.expectedDeviceAddress
            )
            // dcxctl validates the versioned profile envelope and its strict
            // O1/PEQ9 document. The document remains unchanged inside it.
            let profile = try workspace.write(value.desired, named: "profile.json")
            arguments = [
                "control", "diff",
                "--snapshot", snapshot.path,
                "--profile", profile.path,
            ]
        case let .apply(value):
            let applyPlanDigest = value.plan.diff.applyPlanDigest
            let plan: RawPlanReference
            do {
                plan = try planStore.loadApply(
                    applyPlanDigest: applyPlanDigest,
                    transactionID: applyPlanDigest,
                    expectedDevice: value.target.expectedDeviceAddress,
                    baselineDigest: value.plan.baseline.digest,
                    desiredSnapshotDigest: value.plan.diff.desiredSnapshotDigest
                )
                try planStore.makeDurable(plan)
                let rollback = try planStore.loadRollback(
                    rollbackPlanDigest: value.plan.diff.rollbackPlanDigest,
                    transactionID: applyPlanDigest,
                    applyPlanDigest: applyPlanDigest,
                    expectedDevice: value.target.expectedDeviceAddress,
                    baselineDigest: value.plan.baseline.digest
                )
                try planStore.makeDurable(rollback)
            } catch {
                throw ChildResponseError.requestPlanBinding
            }
            arguments = ["control", "apply"] + targetArguments + ["--plan", plan.url.path]
        case .readback:
            arguments = ["control", "readback"] + targetArguments
        case let .rollback(value):
            let plan: RawPlanReference
            do {
                plan = try planStore.loadRollback(
                    rollbackPlanDigest: value.plan.rollbackPlanDigest,
                    transactionID: value.plan.transactionID,
                    applyPlanDigest: value.plan.transactionID,
                    expectedDevice: value.target.expectedDeviceAddress,
                    baselineDigest: value.plan.baseline.digest
                )
                try planStore.makeDurable(plan)
            } catch {
                throw ChildResponseError.requestPlanBinding
            }
            arguments = ["control", "rollback"] + targetArguments + ["--plan", plan.url.path]
        }
        return .init(operation: request.operation, executableURL: executable, arguments: arguments)
    }

    private func decode(
        request: BridgeRequest,
        result: DCXCTLProcessResult
    ) throws -> BridgeResponse {
        let decoder = BridgeJSONCodec.decoder()
        let receipt = result.receipt(for: request.operation)
        let body: BridgeResponseBody
        switch request.body {
        case .helperStatus:
            throw HelperConfigurationError.operationUnavailable
        case let .identitySearch(value):
            let output = try decoder.decode(LiveSearchOutput.self, from: result.stdout)
            guard output.status == "identified", output.selectedBaud == 38_400,
                  output.validResponses == 10,
                  output.protocolIdentity.deviceAddress == value.target.expectedDeviceAddress else {
                throw ChildResponseError.invalidSearch
            }
            let identity = DeviceIdentityV1(
                manufacturer: output.protocolIdentity.manufacturer,
                model: output.protocolIdentity.model,
                deviceAddress: output.protocolIdentity.deviceAddress,
                selectedBaud: output.selectedBaud,
                validSearchResponses: output.validResponses
            )
            try identity.validate()
            body = .identitySearch(.init(identity: identity, receipt: receipt))
        case let .snapshotCapture(value):
            let snapshot = try ingestSnapshot(
                result.stdout,
                target: value.target,
                validSearchResponses: 10
            )
            body = .snapshotCapture(.init(snapshot: snapshot, receipt: receipt))
        case let .diffPreview(value):
            let output = try decoder.decode(DiffCommandOutput.self, from: result.stdout)
            guard output.baselineSnapshotDigest == value.baseline.digest,
                  output.desiredProfileDigest == value.desired.digest else {
                throw ChildResponseError.bindingMismatch
            }
            let apply = try planStore.persistApply(
                output.applyPlan,
                expectedDevice: value.target.expectedDeviceAddress,
                baselineDigest: value.baseline.digest,
                desiredSnapshotDigest: output.desiredSnapshotDigest
            )
            let rollback = try planStore.persistRollback(
                output.rollbackPlan,
                expectedDevice: value.target.expectedDeviceAddress,
                baselineDigest: value.baseline.digest,
                applyPlanDigest: apply.digest
            )
            let diff = try SemanticDiffV1(
                baselineSnapshotDigest: output.baselineSnapshotDigest,
                desiredProfileDigest: output.desiredProfileDigest,
                desiredSnapshotDigest: output.desiredSnapshotDigest,
                applyPlanDigest: apply.digest,
                rollbackPlanDigest: rollback.digest,
                changes: output.changes
            )
            body = .diffPreview(.init(diff: diff, receipt: receipt))
        case let .apply(value):
            let output = try decoder.decode(ApplyCommandOutput.self, from: result.stdout)
            let transactionID = value.plan.diff.applyPlanDigest
            guard output.transactionID == transactionID,
                  output.baselineSnapshotDigest == value.plan.baseline.digest,
                  output.desiredSnapshotDigest == value.plan.diff.desiredSnapshotDigest else {
                throw ChildResponseError.bindingMismatch
            }
            let readback = try output.readback.map {
                try ingestSnapshot($0, target: value.target, validSearchResponses: 1)
            }
            let matches = readback?.digest == output.desiredSnapshotDigest
            guard output.readbackMatchesDesired == matches,
                  matches || output.rollbackAvailable,
                  readback != nil || !matches else {
                throw ChildResponseError.bindingMismatch
            }
            body = .apply(.init(
                transactionID: transactionID,
                baselineDigest: output.baselineSnapshotDigest,
                desiredSnapshotDigest: output.desiredSnapshotDigest,
                readback: readback,
                readbackMatchesDesired: matches,
                rollbackRequired: !matches,
                rollbackAvailable: output.rollbackAvailable,
                receipt: receipt
            ))
        case let .readback(value):
            let snapshot = try ingestSnapshot(
                result.stdout,
                target: value.target,
                validSearchResponses: 10
            )
            body = .readback(.init(
                transactionID: value.transactionID,
                matchesDesired: snapshot.digest == value.expectedDesiredDigest,
                snapshot: snapshot,
                receipt: receipt
            ))
        case let .rollback(value):
            let output = try decoder.decode(RollbackCommandOutput.self, from: result.stdout)
            guard output.transactionID == value.plan.transactionID,
                  output.baselineSnapshotDigest == value.plan.baseline.digest else {
                throw ChildResponseError.bindingMismatch
            }
            let restored = try output.restored.map {
                try ingestSnapshot($0, target: value.target, validSearchResponses: 1)
            }
            let equalsBaseline = restored?.digest == output.baselineSnapshotDigest
            guard output.equalsBaseline == equalsBaseline else {
                throw ChildResponseError.bindingMismatch
            }
            body = .rollback(.init(
                transactionID: value.plan.transactionID,
                baselineDigest: output.baselineSnapshotDigest,
                restored: restored,
                equalsBaseline: equalsBaseline,
                receipt: receipt
            ))
        }
        return BridgeResponse(requestID: request.requestID, body: body)
    }

    private func ingestSnapshot(
        _ data: Data,
        target: DCXTargetReference,
        validSearchResponses: UInt8
    ) throws -> SnapshotV1 {
        let carrier = try BridgeJSONCodec.decoder().decode(CoreSnapshotCarrierV1.self, from: data)
        return try ingestSnapshot(
            carrier,
            target: target,
            validSearchResponses: validSearchResponses
        )
    }

    private func ingestSnapshot(
        _ carrier: CoreSnapshotCarrierV1,
        target: DCXTargetReference,
        validSearchResponses: UInt8
    ) throws -> SnapshotV1 {
        try carrier.validate(expectedDevice: target.expectedDeviceAddress)
        let canonical = try BridgeJSONCodec.encoder().encode(carrier)
        _ = try snapshotStore.persist(canonical, carrier: carrier)
        return try carrier.summary(
            target: target,
            validSearchResponses: validSearchResponses
        )
    }

    private func failure(
        _ request: BridgeRequest,
        _ code: BridgeErrorPayload.Code,
        _ message: String,
        _ retryable: Bool
    ) -> BridgeResponse {
        .init(
            requestID: request.requestID,
            operation: request.operation,
            error: .init(code: code, message: message, retryable: retryable)
        )
    }

    private func preAdmissionFailure(
        _ request: BridgeRequest,
        _ code: BridgeErrorPayload.Code,
        _ message: String,
        _ retryable: Bool
    ) -> BridgeResponse {
        commandFailure(
            request,
            applyAdmitted: false,
            code,
            message,
            retryable
        )
    }

    private func commandFailure(
        _ request: BridgeRequest,
        applyAdmitted: Bool,
        _ code: BridgeErrorPayload.Code,
        _ message: String,
        _ retryable: Bool
    ) -> BridgeResponse {
        failure(
            request,
            request.operation == .apply && !applyAdmitted ? .mutationNotAdmitted : code,
            message,
            retryable
        )
    }
}

private struct LiveSearchOutput: Decodable {
    struct Identity: Decodable {
        let manufacturer: String
        let model: String
        let deviceAddress: UInt8
    }

    let status: String
    let protocolIdentity: Identity
    let selectedBaud: UInt32
    let validResponses: UInt8
}

private struct DiffCommandOutput: Decodable {
    let baselineSnapshotDigest: String
    let desiredProfileDigest: String
    let desiredSnapshotDigest: String
    let changes: [SemanticChangeV1]
    let applyPlan: JSONValue
    let rollbackPlan: JSONValue

    enum CodingKeys: String, CodingKey {
        case baselineSnapshotDigest = "baseline_snapshot_digest"
        case desiredProfileDigest = "desired_profile_digest"
        case desiredSnapshotDigest = "desired_snapshot_digest"
        case changes
        case applyPlan = "apply_plan"
        case rollbackPlan = "rollback_plan"
    }
}

private struct ApplyCommandOutput: Decodable {
    let transactionID: String
    let baselineSnapshotDigest: String
    let desiredSnapshotDigest: String
    let readback: CoreSnapshotCarrierV1?
    let rollbackAvailable: Bool
    let readbackMatchesDesired: Bool

    enum CodingKeys: String, CodingKey {
        case transactionID = "transaction_id"
        case baselineSnapshotDigest = "baseline_snapshot_digest"
        case desiredSnapshotDigest = "desired_snapshot_digest"
        case readback
        case rollbackAvailable = "rollback_available"
        case readbackMatchesDesired = "readback_matches_desired"
    }
}

private struct RollbackCommandOutput: Decodable {
    let transactionID: String
    let baselineSnapshotDigest: String
    let restored: CoreSnapshotCarrierV1?
    let equalsBaseline: Bool

    enum CodingKeys: String, CodingKey {
        case transactionID = "transaction_id"
        case baselineSnapshotDigest = "baseline_snapshot_digest"
        case restored
        case equalsBaseline = "equals_baseline"
    }
}

private final class TransactionWorkspace: @unchecked Sendable {
    private let root: URL
    private let directory: URL
    private let fileManager: FileManager

    init(root: URL, fileManager: FileManager = .default) throws {
        self.root = root.standardizedFileURL
        self.fileManager = fileManager
        directory = self.root.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try fileManager.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
    }

    func write<T: Encodable>(_ value: T, named name: String) throws -> URL {
        guard !name.contains("/"), !name.contains("..") else {
            throw ChildResponseError.invalidWorkspaceLeaf
        }
        let url = directory.appendingPathComponent(name, isDirectory: false)
        let data = try BridgeJSONCodec.encoder().encode(value)
        guard data.count <= DCXBridgeContract.maximumFrameBytes else {
            throw AppGroupBoundaryError.frameTooLarge
        }
        try data.write(to: url, options: [.atomic, .completeFileProtection])
        try fileManager.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
        return url
    }

    func remove() {
        let standardized = directory.standardizedFileURL
        guard standardized.path.hasPrefix(root.path + "/"), standardized != root else { return }
        try? fileManager.removeItem(at: standardized)
    }

    deinit { remove() }
}

private enum ChildResponseError: Error {
    case invalidSearch
    case bindingMismatch
    case invalidWorkspaceLeaf
    case requestPlanBinding
}

private enum MutationRecoveryStateError: Error {
    case applyAlreadyActive
    case recoveryUnavailable
    case recoveryInProgress
    case bindingMismatch
    case configurationChanged
}
