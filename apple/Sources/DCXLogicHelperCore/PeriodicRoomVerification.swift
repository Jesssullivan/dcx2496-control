import Foundation

/// Process-wide owner of the periodic room reader. This is a defensive guard.
/// The app has one runtime, and a runtime already allows one periodic timer.
final class PeriodicRoomReaderClaim: @unchecked Sendable {
    static let process = PeriodicRoomReaderClaim()
    private let lock = NSLock()
    // Weak, so an owner freed without a release does not hold the claim.
    private weak var owner: AnyObject?

    /// True when `candidate` now owns the reader. Repeating the owner's claim is idempotent.
    func acquire(_ candidate: AnyObject) -> Bool {
        lock.lock(); defer { lock.unlock() }
        guard owner == nil || owner === candidate else { return false }
        owner = candidate
        return true
    }

    /// Only the current owner can release. Other callers change nothing.
    func release(_ candidate: AnyObject) {
        lock.lock(); defer { lock.unlock() }
        if owner === candidate { owner = nil }
    }
}

/// The runtime's periodic clock state. `begin` takes the claim and names a new
/// generation. A timer event acts only while its generation is current.
final class PeriodicRoomClockOwnership: @unchecked Sendable {
    private let claim: PeriodicRoomReaderClaim
    private let lock = NSLock()
    private var generation: UInt64 = 0
    private var current: UInt64?

    init(claim: PeriodicRoomReaderClaim = .process) {
        self.claim = claim
    }

    /// The new generation, or nil when another owner holds the claim.
    func begin(owner: AnyObject) -> UInt64? {
        lock.lock(); defer { lock.unlock() }
        guard claim.acquire(owner) else { return nil }
        generation &+= 1
        current = generation
        return generation
    }

    func isCurrent(_ candidate: UInt64) -> Bool {
        lock.lock(); defer { lock.unlock() }
        return current == candidate
    }

    /// Retires `retiring` and releases the claim. A stop that names an older
    /// generation changes nothing, so a late stop cannot retire a newer clock.
    func end(owner: AnyObject, generation retiring: UInt64) {
        lock.lock(); defer { lock.unlock() }
        guard current == retiring else { return }
        current = nil
        claim.release(owner)
    }
}

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
        case retryableReadFailure
    }
    static let successDelay: TimeInterval = 30
    private let lock = NSLock()
    private var running = false
    private var generation: UInt64 = 0
    private var binding: Data?
    private var active: Attempt?
    private var due: Date?
    private var observedCapture: Date?
    private var readFailureCount = 0
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
            readFailureCount = 0
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
            readFailureCount = 0
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
            readFailureCount = 0
        case .busy:
            busyCount = min(busyCount + 1, 6)
            due = now.addingTimeInterval(min(300, 15 * pow(2, Double(busyCount - 1))))
        case .retryableReadFailure:
            // A fresh read still passes every binding, recovery and serial guard.
            readFailureCount = min(readFailureCount + 1, 6)
            due = now.addingTimeInterval(min(300, 15 * pow(2, Double(readFailureCount - 1))))
        case .failed:
            // Unknown or failed transport retirement is never retried by a timer.
            blocked = true
            due = nil
        }
    }
}
