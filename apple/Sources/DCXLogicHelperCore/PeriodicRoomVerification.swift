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
        /// A verified-close Dump0 or Dump1 timeout after `received` bytes.
        case partialReadFailure(received: UInt32)
        /// A verified-close Dump0 or Dump1 read whose complete frame failed
        /// validation, such as a 26-byte frame where a 1015-byte dump belongs.
        case validationReadFailure
    }
    static let successDelay: TimeInterval = 30
    /// Partial dumps in a row, with no successful capture between them, that
    /// stop the periodic reader. The first two retry on the next period.
    static let partialReadCap = 3
    /// Validation failures in a row, with no successful capture between them,
    /// that stop the periodic reader. The first two retry on the next period.
    static let validationReadCap = 3
    /// TIN-6964: a stop at a verified-close cap lifts by itself after this
    /// cooldown. Each further cap stop doubles it, up to `maximumCapCooldown`.
    static let capCooldown: TimeInterval = 300
    static let maximumCapCooldown: TimeInterval = 1800
    static let failedReason = "periodic-read-failed"
    static func partialCapReason(received: UInt32) -> String {
        "periodic-partial-read-cap-reached: \(partialReadCap) partial dumps in a row, last received \(received) bytes"
    }
    static let validationCapReason =
        "periodic-validation-read-cap-reached: \(validationReadCap) dump frames in a row failed validation"
    static func cooldownSuffix(_ seconds: TimeInterval) -> String {
        "; resumes after \(Int(seconds)) seconds"
    }
    static func cooldown(afterStops stops: Int) -> TimeInterval {
        min(maximumCapCooldown, capCooldown * pow(2, Double(max(stops, 1) - 1)))
    }
    private let lock = NSLock()
    private var running = false
    private var generation: UInt64 = 0
    private var binding: Data?
    private var active: Attempt?
    private var due: Date?
    private var observedCapture: Date?
    private var readFailureCount = 0
    private var busyCount = 0
    private var partialReadCount = 0
    private var validationReadCount = 0
    private var capStopCount = 0
    private var blocked = false
    private var blockedReason: String?
    /// Set only for a verified-close cap stop. A latched failure has none.
    private var resumeAt: Date?

    /// Why periodic reads stopped, or nil while they may run.
    var blockReason: String? {
        lock.lock(); defer { lock.unlock() }
        return blocked ? blockedReason : nil
    }

    /// When a cap stop lifts by itself, or nil while running or latched.
    var resumesAt: Date? {
        lock.lock(); defer { lock.unlock() }
        return blocked ? resumeAt : nil
    }

    private func clearBlock() {
        blocked = false
        blockedReason = nil
        resumeAt = nil
    }

    private func resetReadCounts() {
        busyCount = 0
        readFailureCount = 0
        partialReadCount = 0
        validationReadCount = 0
        capStopCount = 0
    }

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
        partialReadCount = 0
        validationReadCount = 0
        capStopCount = 0
        clearBlock()
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
            resetReadCounts()
            clearBlock()
            observedCapture = nil
        }
        // A real explicit/URL/startup capture defers the periodic deadline too.
        // Merely observing the same heartbeat cannot slide the deadline.
        if let capturedAt, capturedAt <= now, observedCapture == nil || capturedAt > observedCapture! {
            observedCapture = capturedAt
            due = now.addingTimeInterval(Self.successDelay)
            clearBlock()
            resetReadCounts()
        }
        // A verified-close cap stop lifts after its cooldown. The next read
        // repeats every binding, recovery and serial admission check.
        if blocked, let resumeAt, now >= resumeAt {
            clearBlock()
            partialReadCount = 0
            validationReadCount = 0
            due = now
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
            resetReadCounts()
        case .busy:
            busyCount = min(busyCount + 1, 6)
            due = now.addingTimeInterval(min(300, 15 * pow(2, Double(busyCount - 1))))
        case .retryableReadFailure:
            // A fresh read still passes every binding, recovery and serial guard.
            readFailureCount = min(readFailureCount + 1, 6)
            due = now.addingTimeInterval(min(300, 15 * pow(2, Double(readFailureCount - 1))))
        case let .partialReadFailure(received):
            // Busy and empty timeouts do not reset this count. Only a
            // successful capture or a changed binding does.
            partialReadCount += 1
            if partialReadCount >= Self.partialReadCap {
                stopAtCap(Self.partialCapReason(received: received), now: now)
            } else {
                due = now.addingTimeInterval(Self.successDelay)
            }
        case .validationReadFailure:
            // Its own count. Empty timeouts, partial dumps and busy admission
            // do not reset it. Only a successful capture or a changed binding does.
            validationReadCount += 1
            if validationReadCount >= Self.validationReadCap {
                stopAtCap(Self.validationCapReason, now: now)
            } else {
                due = now.addingTimeInterval(Self.successDelay)
            }
        case .failed:
            // Unknown or failed transport retirement is never retried by a timer.
            blocked = true
            blockedReason = Self.failedReason
            resumeAt = nil
            due = nil
        }
    }

    private func stopAtCap(_ reason: String, now: Date) {
        capStopCount += 1
        let cooldown = Self.cooldown(afterStops: capStopCount)
        blocked = true
        blockedReason = reason + Self.cooldownSuffix(cooldown)
        resumeAt = now.addingTimeInterval(cooldown)
        due = nil
    }
}
