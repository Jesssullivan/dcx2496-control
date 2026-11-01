import AppKit
import DCXLogicBridge
import Foundation

/// Lifecycle-owned runtime for the containing app. There is deliberately no
/// daemon entry point: the app calls `enterForeground()` and `leaveForeground()`.
public final class ForegroundHelperRuntime: @unchecked Sendable {
    private let locations: AppGroupLocations
    private let socketServer = AppGroupSocketServer()
    private let coreMIDI = CoreMIDIPresentation()
    private let coordinator: DCXHelperCoordinator
    private let lock = NSLock()
    private var running = false
    private let periodic = PeriodicRoomVerification()
    private var periodicTimer: DispatchSourceTimer?

    public init() throws {
        let locations = try AppGroupLocations()
        self.locations = locations
        let configuration: Result<HelperConfigurationV1, Error>
        do {
            configuration = .success(try HelperConfigurationV1.load(from: locations.helperConfigurationURL))
        } catch {
            configuration = .failure(error)
        }
        coordinator = .init(
            locations: locations,
            configuration: configuration,
            coreMIDI: coreMIDI
        )
    }

    public func enterForeground() throws {
        lock.lock()
        guard !running else {
            lock.unlock()
            return
        }
        lock.unlock()

        try locations.prepareBridgeDirectory()
        // Pick up an App Group configuration replaced while the app was
        // inactive. Invalid or absent configuration leaves the bridge running
        // in its existing fail-closed, unconfigured state.
        _ = try? reloadConfiguration()
        try coreMIDI.start()
        do {
            coordinator.setForeground(true)
            try socketServer.start(socketURL: locations.socketURL) { [coordinator] request in
                coordinator.handle(request)
            }
        } catch {
            coordinator.setForeground(false)
            coreMIDI.stop()
            throw error
        }

        lock.lock()
        running = true
        lock.unlock()
        startPeriodicClock()
        // Startup reads are explicitly opted in and run before Logic preload.
        // This startup-only option excludes Logic. Periodic reads have a separate opt-in.
        if let approved = currentApprovedRoomConfiguration(), approved.verifyOnForegroundLaunch,
           NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.logic10").isEmpty {
            Task.detached { [weak self] in _ = try? self?.verifySavedApprovedRoom() }
        }
    }

    public func leaveForeground() {
        lock.lock()
        let wasRunning = running
        running = false
        let stoppedTimer = periodicTimer
        periodicTimer = nil
        lock.unlock()
        guard wasRunning else { return }
        periodic.stop()
        stoppedTimer?.cancel()
        coordinator.setForeground(false)
        socketServer.stop()
        coreMIDI.stop()
    }

    public func localStatus() -> HelperStatusResponse? {
        guard let request = try? BridgeRequest(body: .helperStatus(.init())) else { return nil }
        let response = coordinator.handle(request)
        guard case let .helperStatus(status) = response.body else { return nil }
        return status
    }

    /// Called only by the explicit foreground verification button, never MIDI.
    public func verifyRoom(expectation: Data) throws -> RoomVerificationReceipt {
        try coordinator.verifyRoom(expectation: expectation)
    }

    private var approvedRoomURL: URL {
        locations.helperConfigurationURL.deletingLastPathComponent().appendingPathComponent("approved-room-configuration.json")
    }

    public func currentApprovedRoomConfiguration() -> ApprovedRoomConfiguration? {
        guard let configuration = currentConfiguration(),
              let approved = try? ApprovedRoomConfiguration.load(from: approvedRoomURL),
              (try? approved.requireBinding(to: configuration)) != nil else { return nil }
        return approved
    }

    public func saveApprovedRoomExpectation(_ expectation: Data, verifyOnForegroundLaunch: Bool,
                                            allowExplicitRefreshRequests: Bool, periodicVerificationEnabled: Bool = false) throws {
        guard let configuration = currentConfiguration() else { throw RoomVerificationError.unavailable }
        try coordinator.validateApprovedRoomExpectation(expectation)
        let approved = try ApprovedRoomConfiguration(configuration: configuration, expectation: expectation,
            verifyOnForegroundLaunch: verifyOnForegroundLaunch, allowExplicitRefreshRequests: allowExplicitRefreshRequests,
            periodicVerificationEnabled: periodicVerificationEnabled)
        try approved.requireBinding(to: configuration)
        try approved.save(to: approvedRoomURL)
    }

    /// A fixed URL may request a read of the configured target only. No URL
    /// component supplies a port, frame, profile or write operation.
    public func verifySavedApprovedRoom(requestURL: URL? = nil) throws -> RoomVerificationReceipt {
        guard let approved = currentApprovedRoomConfiguration() else { throw RoomVerificationError.unavailable }
        if let requestURL {
            guard ApprovedRoomConfiguration.acceptsRefreshURL(requestURL), approved.allowExplicitRefreshRequests else {
                throw RoomVerificationError.unavailable
            }
        }
        let binding = try checkedSavedBinding()
        guard binding.approved == approved else { throw RoomVerificationError.unavailable }
        return try coordinator.verifyBoundRoom(expectation: approved.expectation, forceFresh: false, expectedConfiguration: binding.configuration, approvedTargetRefresh: true,
            bindingIsCurrent: { [weak self] in self?.savedBindingMatches(binding.key) == true })
    }

    private struct SavedBinding: Encodable {
        let configuration: HelperConfigurationV1
        let approved: ApprovedRoomConfiguration
    }

    /// Disk-only binding check. It never calls back into the coordinator lock.
    private func checkedSavedBinding() throws -> (configuration: HelperConfigurationV1, approved: ApprovedRoomConfiguration, key: Data) {
        let configuration = try HelperConfigurationV1.load(from: locations.helperConfigurationURL)
        let approved = try ApprovedRoomConfiguration.load(from: approvedRoomURL)
        try approved.requireBinding(to: configuration)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        return (configuration, approved, try encoder.encode(SavedBinding(configuration: configuration, approved: approved)))
    }

    private func savedBindingMatches(_ key: Data) -> Bool {
        (try? checkedSavedBinding().key) == key
    }

    private func startPeriodicClock() {
        lock.lock()
        guard running, periodicTimer == nil else { lock.unlock(); return }
        periodic.start()
        let timer = DispatchSource.makeTimerSource(queue: DispatchQueue(label: "io.tinyland.dcx.periodic-room"))
        timer.schedule(deadline: .now())
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            self.periodicTick()
            self.lock.lock()
            if self.running { self.periodicTimer?.schedule(deadline: .now() + .seconds(5)) }
            self.lock.unlock()
        }
        periodicTimer = timer
        timer.resume()
        lock.unlock()
    }

    private func periodicTick() {
        let saved = try? checkedSavedBinding()
        let enabled = saved?.approved.periodicVerificationEnabled == true
        let capturedAt = saved.flatMap { coordinator.roomCaptureDate(expectation: $0.approved.expectation) }
        let step = periodic.poll(binding: enabled ? saved?.key : nil, capturedAt: capturedAt, now: Date())
        if step.invalidatesPriorBinding { coordinator.invalidateApprovedRoomBinding() }
        guard let attempt = step.attempt, let saved else { return }
        // Only this lifecycle-owned worker can schedule a periodic device read.
        // Incoming MIDI and heartbeat publication never invoke this method.
        Task.detached { [weak self] in
            guard let self else { return }
            let outcome: PeriodicRoomVerification.Outcome
            do {
                guard self.periodic.isCurrent(attempt), self.savedBindingMatches(attempt.binding), saved.approved.periodicVerificationEnabled else {
                    throw RoomVerificationError.unavailable
                }
                let receipt = try self.coordinator.verifyBoundRoom(expectation: saved.approved.expectation,
                    forceFresh: true, expectedConfiguration: saved.configuration, approvedTargetRefresh: true, bindingIsCurrent: { [weak self] in
                        guard let self else { return false }
                        return self.periodic.isCurrent(attempt) && self.savedBindingMatches(attempt.binding)
                    })
                let formatter = ISO8601DateFormatter()
                formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
                guard receipt.matches, let date = formatter.date(from: receipt.verifiedAt) else {
                    throw RoomVerificationError.invalidComparison
                }
                outcome = .success(capturedAt: date)
            } catch RoomVerificationError.busy {
                outcome = .busy
            } catch {
                outcome = .failed
            }
            self.periodic.complete(attempt, outcome: outcome, now: Date())
        }
    }

    /// Currently loaded checked configuration, if any.
    public func currentConfiguration() -> HelperConfigurationV1? {
        coordinator.currentConfiguration()
    }

    /// Atomically save one checked configuration at the fixed App Group path
    /// and install the exact reloaded value for subsequent requests.
    public func saveConfiguration(_ configuration: HelperConfigurationV1) throws {
        _ = try coordinator.installConfiguration(configuration) { [locations] in
            try configuration.save(to: locations.helperConfigurationURL)
            return try HelperConfigurationV1.load(from: locations.helperConfigurationURL)
        }
    }

    /// Reload the fixed App Group configuration without restarting the app,
    /// socket, CoreMIDI presentation, or an already-running child transaction.
    /// A durable mutation recovery lease rejects replacement until an exact
    /// rollback readback reaches the pinned baseline.
    @discardableResult
    public func reloadConfiguration() throws -> HelperConfigurationV1 {
        try coordinator.replaceConfiguration { [locations] in
            do {
                return .success(
                    try HelperConfigurationV1.load(from: locations.helperConfigurationURL)
                )
            } catch {
                return .failure(error)
            }
        }
    }

    deinit { leaveForeground() }
}
