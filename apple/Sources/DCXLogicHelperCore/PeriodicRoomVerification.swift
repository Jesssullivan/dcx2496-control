import Foundation

/// Scheduling state only. The foreground runtime owns the one timer and all
/// captures still pass through the coordinator's existing serial ownership.
final class PeriodicRoomVerification: @unchecked Sendable {
    struct Attempt: Equatable {
        let id = UUID()
        let generation: UInt64
        let binding: Data
    }
    struct Step {
        let invalidatesPriorBinding: Bool
        let attempt: Attempt?
    }
    enum Outcome {
        case success(capturedAt: Date)
        case busy
        case failed
    }
    static let successDelay: TimeInterval = 300
    private let lock = NSLock()
    private var running = false
    private var generation: UInt64 = 0
    private var binding: Data?
    private var active: Attempt?
    private var due: Date?
    private var observedCapture: Date?
    private var busyCount = 0
    private var blocked = false

    func start() {
        lock.lock(); defer { lock.unlock() }
        running = true
    }

    func stop() {
        lock.lock(); defer { lock.unlock() }
        running = false
        generation &+= 1
        binding = nil
        due = nil
        observedCapture = nil
        blocked = false
    }

    func poll(binding checkedBinding: Data?, capturedAt: Date?, now: Date) -> Step {
        lock.lock(); defer { lock.unlock() }
        guard running else { return Step(invalidatesPriorBinding: false, attempt: nil) }
        let changed = binding != checkedBinding
        let invalidates = binding != nil && changed
        if changed {
            generation &+= 1
            binding = checkedBinding
            due = now
            busyCount = 0
            blocked = false
            observedCapture = nil
        }
        // A real explicit/URL/startup capture defers the periodic deadline too.
        // Merely observing the same heartbeat cannot slide the deadline.
        if let capturedAt, capturedAt <= now, observedCapture == nil || capturedAt > observedCapture! {
            observedCapture = capturedAt
            due = now.addingTimeInterval(Self.successDelay)
            blocked = false
            busyCount = 0
        }
        guard let binding, active == nil, !blocked, due == nil || now >= due! else {
            return Step(invalidatesPriorBinding: invalidates, attempt: nil)
        }
        let attempt = Attempt(generation: generation, binding: binding)
        active = attempt
        return Step(invalidatesPriorBinding: invalidates, attempt: attempt)
    }

    func isCurrent(_ attempt: Attempt) -> Bool {
        lock.lock(); defer { lock.unlock() }
        return running && generation == attempt.generation && binding == attempt.binding
    }

    func complete(_ attempt: Attempt, outcome: Outcome, now: Date) {
        lock.lock(); defer { lock.unlock() }
        guard active?.id == attempt.id else { return }
        active = nil
        guard running, attempt.generation == generation, attempt.binding == binding else { return }
        switch outcome {
        case let .success(capturedAt):
            observedCapture = capturedAt
            due = now.addingTimeInterval(Self.successDelay)
            busyCount = 0
        case .busy:
            busyCount = min(busyCount + 1, 6)
            due = now.addingTimeInterval(min(300, 15 * pow(2, Double(busyCount - 1))))
        case .failed:
            // Unknown or failed transport retirement is never retried by a timer.
            blocked = true
            due = nil
        }
    }
}
