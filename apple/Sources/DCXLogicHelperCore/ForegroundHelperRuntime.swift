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
    }

    public func leaveForeground() {
        lock.lock()
        let wasRunning = running
        running = false
        lock.unlock()
        guard wasRunning else { return }
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

    /// Currently loaded checked configuration, if any.
    public func currentConfiguration() -> HelperConfigurationV1? {
        coordinator.currentConfiguration()
    }

    /// Atomically save one checked configuration at the fixed App Group path
    /// and install the exact reloaded value for subsequent requests.
    public func saveConfiguration(_ configuration: HelperConfigurationV1) throws {
        try configuration.save(to: locations.helperConfigurationURL)
        _ = try reloadConfiguration()
    }

    /// Reload the fixed App Group configuration without restarting the app,
    /// socket, CoreMIDI presentation, or an already-running child transaction.
    /// An active transaction retains the immutable configuration it captured;
    /// the reloaded value applies to the next request.
    @discardableResult
    public func reloadConfiguration() throws -> HelperConfigurationV1 {
        do {
            let loaded = try HelperConfigurationV1.load(from: locations.helperConfigurationURL)
            coordinator.replaceConfiguration(.success(loaded))
            return loaded
        } catch {
            coordinator.replaceConfiguration(.failure(error))
            throw error
        }
    }

    deinit { leaveForeground() }
}
