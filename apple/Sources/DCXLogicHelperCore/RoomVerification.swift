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
    private var receipt: RoomVerificationReceipt?
    private var capturedAt: Date?
    private var ttyPath: String?
    private var timer: DispatchSourceTimer?

    init(startHeartbeats: Bool = true, submit: (@Sendable (RoomVerificationReceipt) -> Void)? = nil) {
        let sender = PracticeMIDISender()
        self.submit = submit ?? { value in sender.send(value) }
        self.startHeartbeats = startHeartbeats
    }

    func install(_ comparison: RoomComparison, expectation: Data, capturedAt: Date, ttyPath: String) throws -> RoomVerificationReceipt {
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

    func invalidate(_ reason: String) {
        lock.lock()
        timer?.cancel()
        timer = nil
        let invalidated = receipt
        receipt = nil
        capturedAt = nil
        ttyPath = nil
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
        guard let renewed = value.renewed(at: now, capturedAt: capturedAt,
                                        ttyPresent: FileManager.default.fileExists(atPath: ttyPath)) else {
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

public enum RoomVerificationError: Error, Equatable {
    case invalidExpectation
    case invalidComparison
    case unavailable
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
