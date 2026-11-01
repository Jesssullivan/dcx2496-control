import CoreMIDI
import CryptoKit
import DCXLogicBridge
import Foundation

/// Live provenance added only after the foreground helper captures the device.
/// A matching offline comparison alone cannot construct this receipt.
public struct RoomVerificationReceipt: Codable, Equatable, Sendable {
    public let schemaVersion: String
    public var status: String
    public let helperInstanceID: String
    public let sceneID: String
    public let sceneRevision: UInt32
    public let compiledDigest: String
    public let presetDigest: String
    public let expectationDigest: String
    public let snapshotDigest: String
    public let expectedSnapshotDigest: String
    public let verifiedAt: String
    public var expiresAt: String
    public let readable: Bool
    public var matches: Bool
    public let validationScope: String
    public let comparedFields: Int
    public var invalidationReason: String?

    /// The heartbeat extends helper liveness, never the actual capture age.
    func renewed(at now: Date, capturedAt: Date, ttyPresent: Bool) -> Self? {
        guard ttyPresent, now >= capturedAt, now.timeIntervalSince(capturedAt) < 60 else { return nil }
        var value = self
        value.expiresAt = RoomVerificationPublisher.timestamp(min(capturedAt.addingTimeInterval(60), now.addingTimeInterval(2)))
        return value
    }
}

struct RoomComparison: Codable, Sendable {
    let schemaVersion: String
    let sceneId: String
    let sceneRevision: UInt32
    let compiledDigest: String
    let presetDigest: String
    let expectationDigest: String
    let snapshotDigest: String
    let expectedSnapshotDigest: String
    let matches: Bool
    let readable: Bool
    let comparedFields: Int
}

/// Heartbeats repeat a bounded attestation without re-reading serial hardware.
/// Any device operation, configuration change or foreground exit invalidates it.
final class RoomVerificationPublisher: @unchecked Sendable {
    private let lock = NSLock()
    private let instanceID = UUID().uuidString
    private let submit: @Sendable (RoomVerificationReceipt) -> Void
    private let startHeartbeats: Bool
    private let ttyPresent: @Sendable (String) -> Bool
    private var receipt: RoomVerificationReceipt?
    private var capturedAt: Date?
    private var ttyPath: String?
    private var timer: DispatchSourceTimer?
    private var bindingIsCurrent: @Sendable () -> Bool = { true }

    init(startHeartbeats: Bool = true, submit: (@Sendable (RoomVerificationReceipt) -> Void)? = nil,
         ttyPresent: @escaping @Sendable (String) -> Bool = { FileManager.default.fileExists(atPath: $0) }) {
        let sender = PracticeMIDISender()
        self.submit = submit ?? { value in sender.send(value) }
        self.startHeartbeats = startHeartbeats
        self.ttyPresent = ttyPresent
    }

    func install(_ comparison: RoomComparison, expectation: Data, capturedAt: Date, ttyPath: String,
                 bindingIsCurrent: @escaping @Sendable () -> Bool = { true }) throws -> RoomVerificationReceipt {
        guard bindingIsCurrent() else { throw RoomVerificationError.unavailable }
        let digest = "sha256/" + SHA256.hash(data: expectation).map { String(format: "%02x", $0) }.joined()
        guard comparison.schemaVersion == "dcx.room-comparison/v1",
              comparison.expectationDigest == digest,
              comparison.sceneRevision > 0, comparison.comparedFields > 6,
              !comparison.matches || (comparison.readable && comparison.snapshotDigest == comparison.expectedSnapshotDigest) else {
            throw RoomVerificationError.invalidComparison
        }
        let value = RoomVerificationReceipt(
            schemaVersion: "dcx.room-verification/v1",
            status: comparison.matches ? "match" : "drift",
            helperInstanceID: instanceID,
            sceneID: comparison.sceneId,
            sceneRevision: comparison.sceneRevision,
            compiledDigest: comparison.compiledDigest,
            presetDigest: comparison.presetDigest,
            expectationDigest: comparison.expectationDigest,
            snapshotDigest: comparison.snapshotDigest,
            expectedSnapshotDigest: comparison.expectedSnapshotDigest,
            verifiedAt: Self.timestamp(capturedAt),
            expiresAt: Self.timestamp(min(capturedAt.addingTimeInterval(60), Date().addingTimeInterval(2))),
            readable: comparison.readable,
            matches: comparison.matches,
            validationScope: "exact-approved-snapshot-and-reviewed-profiles",
            comparedFields: comparison.comparedFields,
            invalidationReason: nil
        )
        lock.lock()
        receipt = value
        self.bindingIsCurrent = bindingIsCurrent
        self.capturedAt = capturedAt
        self.ttyPath = ttyPath
        timer?.cancel()
        let timer: DispatchSourceTimer? = startHeartbeats
            ? DispatchSource.makeTimerSource(queue: DispatchQueue(label: "io.tinyland.dcx.room-attestation")) : nil
        timer?.schedule(deadline: .now(), repeating: .seconds(1))
        timer?.setEventHandler { [weak self] in self?.publish() }
        self.timer = timer
        submit(value)
        lock.unlock()
        timer?.resume()
        return value
    }

    /// Reuse liveness only while the original capture remains fresh. This never
    /// changes verifiedAt or publishes a new capture timestamp.
    func current(expectation: Data, now: Date = Date()) -> RoomVerificationReceipt? {
        let digest = "sha256/" + SHA256.hash(data: expectation).map { String(format: "%02x", $0) }.joined()
        lock.lock()
        defer { lock.unlock() }
        guard bindingIsCurrent(), let receipt, receipt.expectationDigest == digest, let capturedAt, let ttyPath else { return nil }
        return receipt.renewed(at: now, capturedAt: capturedAt,
                               ttyPresent: ttyPresent(ttyPath))
    }

    func captureDate(expectation: Data) -> Date? {
        let digest = "sha256/" + SHA256.hash(data: expectation).map { String(format: "%02x", $0) }.joined()
        lock.lock(); defer { lock.unlock() }
        guard bindingIsCurrent(), receipt?.matches == true, receipt?.expectationDigest == digest else { return nil }
        return capturedAt
    }

    func invalidate(_ reason: String) {
        lock.lock()
        timer?.cancel()
        timer = nil
        let invalidated = receipt
        receipt = nil
        capturedAt = nil
        ttyPath = nil
        bindingIsCurrent = { true }
        lock.unlock()
        guard var value = invalidated else { return }
        value.status = "invalidated"
        value.matches = false
        value.expiresAt = Self.timestamp(Date(timeIntervalSince1970: 0))
        value.invalidationReason = reason
        submit(value)
    }

    private func publish() {
        lock.lock()
        guard var value = receipt, let capturedAt, let ttyPath else { lock.unlock(); return }
        let now = Date()
        guard bindingIsCurrent(), let renewed = value.renewed(at: now, capturedAt: capturedAt,
                                        ttyPresent: ttyPresent(ttyPath)) else {
            lock.unlock()
            invalidate("expired-or-disconnected")
            return
        }
        value = renewed
        // Keep the lock through submission so invalidation cannot race an old match heartbeat.
        submit(value)
        lock.unlock()
    }

    fileprivate static func timestamp(_ date: Date) -> String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter.string(from: date)
    }

    deinit { timer?.cancel() }
}

/// TIN-6964: foreground startup and the fixed URL share one approved read.
/// The lock protects admission and publication, never the serial capture itself.
final class RoomVerificationPreparation: @unchecked Sendable {
    final class BindingChecks: @unchecked Sendable {
        private let lock = NSLock()
        private var checks: [@Sendable () -> Bool] = []
        func append(_ check: @escaping @Sendable () -> Bool) {
            lock.lock(); checks.append(check); lock.unlock()
        }
        func isValid() -> Bool {
            lock.lock(); let snapshot = checks; lock.unlock()
            return snapshot.allSatisfy { $0() }
        }
    }
    final class Attempt: @unchecked Sendable {
        let bindingChecks = BindingChecks()
        let key: Data
        var invalidated = false
        var result: Result<RoomVerificationReceipt, Error>?
        init(key: Data) { self.key = key }
    }

    private let condition = NSCondition()
    private var active: Attempt?
    private var completedKey: Data?
    private var waiters = 0

    var isInFlight: Bool {
        condition.lock()
        defer { condition.unlock() }
        return active != nil
    }

    var waitingRequestCount: Int {
        condition.lock()
        defer { condition.unlock() }
        return waiters
    }

    func run(key: Data, reuse: () -> RoomVerificationReceipt?,
             bindingIsCurrent: @escaping @Sendable () -> Bool = { true },
             capture: (Attempt) throws -> RoomVerificationReceipt) throws -> RoomVerificationReceipt {
        condition.lock()
        if let active {
            guard active.key == key else { condition.unlock(); throw RoomVerificationError.unavailable }
            active.bindingChecks.append(bindingIsCurrent)
            waiters += 1
            while active.result == nil && !active.invalidated { condition.wait() }
            waiters -= 1
            let result: Result<RoomVerificationReceipt, Error> = active.invalidated
                ? .failure(RoomVerificationError.unavailable) : active.result!
            condition.unlock()
            return try result.get()
        }
        if completedKey == key, let receipt = reuse() {
            condition.unlock()
            return receipt
        }
        let attempt = Attempt(key: key)
        attempt.bindingChecks.append(bindingIsCurrent)
        active = attempt
        completedKey = nil
        condition.unlock()
        let captured = Result { try capture(attempt) }
        condition.lock()
        let result: Result<RoomVerificationReceipt, Error> = attempt.invalidated
            ? .failure(RoomVerificationError.unavailable) : captured
        attempt.result = result
        if case .success = result { completedKey = key }
        active = nil
        condition.broadcast()
        condition.unlock()
        return try result.get()
    }

    /// Invalidation also prevents an already-running capture from publishing.
    func invalidate(_ body: () -> Void) {
        condition.lock()
        completedKey = nil
        active?.invalidated = true
        body()
        condition.broadcast()
        condition.unlock()
    }

    func publish(_ attempt: Attempt, _ body: () throws -> RoomVerificationReceipt) throws -> RoomVerificationReceipt {
        condition.lock()
        defer { condition.unlock() }
        guard active === attempt, !attempt.invalidated, attempt.bindingChecks.isValid() else { throw RoomVerificationError.unavailable }
        return try body()
    }
}

public enum RoomVerificationError: Error, Equatable {
    case invalidExpectation
    case invalidComparison
    case unavailable
    case busy
}

/// Output-only IPC. Incoming MIDI never invokes a device command.
private final class PracticeMIDISender: @unchecked Sendable {
    private let lock = NSLock()
    private var client = MIDIClientRef()
    private var port = MIDIPortRef()

    func send(_ receipt: RoomVerificationReceipt) {
        lock.lock()
        defer { lock.unlock() }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        guard let payload = try? encoder.encode(receipt), payload.count <= 8192,
              payload.allSatisfy({ $0 < 128 }) else { return }
        let destinations = (0..<MIDIGetNumberOfDestinations()).map(MIDIGetDestination).filter {
            property($0, kMIDIPropertyName) == "io.tinyland.asfw.practice.app"
                && property($0, kMIDIPropertyManufacturer) == "Tinyland"
                && property($0, kMIDIPropertyModel) == "LS56 Practice IPC v1"
        }
        guard destinations.count == 1 else { return }
        if client == 0 {
            guard MIDIClientCreateWithBlock("Tinyland DCX Room Verification" as CFString, &client, { _ in }) == noErr else { return }
            guard MIDIOutputPortCreate(client, "DCX verified room" as CFString, &port) == noErr else {
                MIDIClientDispose(client); client = 0; return
            }
        }
        var frame: [UInt8] = [0xf0, 0x7d, 0x41, 0x53, 0x46, 0x57, 0x01,
                              UInt8(payload.count & 0x7f), UInt8((payload.count >> 7) & 0x7f)]
        frame.append(contentsOf: payload)
        frame.append(payload.reduce(UInt8(0)) { ($0 &+ $1) & 0x7f })
        frame.append(0xf7)
        let capacity = frame.count + MemoryLayout<MIDIPacketList>.size + 32
        let memory = UnsafeMutableRawPointer.allocate(byteCount: capacity, alignment: MemoryLayout<MIDIPacketList>.alignment)
        defer { memory.deallocate() }
        let list = memory.bindMemory(to: MIDIPacketList.self, capacity: 1)
        let packet = MIDIPacketListInit(list)
        _ = frame.withUnsafeBufferPointer { buffer in
            MIDIPacketListAdd(list, capacity, packet, 0, frame.count, buffer.baseAddress!)
        }
        _ = MIDISend(port, destinations[0], list)
    }

    private func property(_ object: MIDIObjectRef, _ key: CFString) -> String? {
        var value: Unmanaged<CFString>?
        guard MIDIObjectGetStringProperty(object, key, &value) == noErr else { return nil }
        return value?.takeRetainedValue() as String?
    }

    deinit {
        if port != 0 { MIDIPortDispose(port) }
        if client != 0 { MIDIClientDispose(client) }
    }
}
