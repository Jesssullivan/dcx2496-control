import CoreMIDI
import DCXLogicBridge
import Foundation

public final class CoreMIDIPresentation: @unchecked Sendable {
    public static let commandsName = "Tinyland DCX Commands"
    public static let statusName = "Tinyland DCX Status"
    public static let commandsUniqueID: Int32 = 0x4443_5843
    public static let statusUniqueID: Int32 = 0x4443_5853

    private let lock = NSLock()
    private var client = MIDIClientRef()
    private var commands = MIDIEndpointRef()
    private var status = MIDIEndpointRef()

    public init() {}

    /// Creates presentation endpoints only. Incoming v1 Commands bytes are
    /// deliberately discarded: there is no MIDI-to-device mapping and this
    /// callback never calls the bridge, a child process, or serial code.
    public func start() throws {
        lock.lock()
        defer { lock.unlock() }
        guard client == 0 else { return }

        var newClient = MIDIClientRef()
        var result = MIDIClientCreateWithBlock(
            "Tinyland DCX Logic Helper" as CFString,
            &newClient,
            { _ in }
        )
        guard result == noErr else { throw CoreMIDIError.createClient(result) }

        var newCommands = MIDIEndpointRef()
        result = MIDIDestinationCreateWithBlock(
            newClient,
            Self.commandsName as CFString,
            &newCommands,
            { _, _ in /* presentation-only in bridge v1 */ }
        )
        guard result == noErr else {
            MIDIClientDispose(newClient)
            throw CoreMIDIError.createCommands(result)
        }

        var newStatus = MIDIEndpointRef()
        result = MIDISourceCreate(newClient, Self.statusName as CFString, &newStatus)
        guard result == noErr else {
            MIDIEndpointDispose(newCommands)
            MIDIClientDispose(newClient)
            throw CoreMIDIError.createStatus(result)
        }

        result = MIDIObjectSetIntegerProperty(
            newCommands,
            kMIDIPropertyUniqueID,
            Self.commandsUniqueID
        )
        guard result == noErr else {
            Self.dispose(client: newClient, commands: newCommands, status: newStatus)
            throw CoreMIDIError.setUniqueID(result)
        }
        result = MIDIObjectSetIntegerProperty(newStatus, kMIDIPropertyUniqueID, Self.statusUniqueID)
        guard result == noErr else {
            Self.dispose(client: newClient, commands: newCommands, status: newStatus)
            throw CoreMIDIError.setUniqueID(result)
        }

        client = newClient
        commands = newCommands
        status = newStatus
    }

    public func stop() {
        lock.lock()
        let oldClient = client
        let oldCommands = commands
        let oldStatus = status
        client = 0
        commands = 0
        status = 0
        lock.unlock()
        Self.dispose(client: oldClient, commands: oldCommands, status: oldStatus)
    }

    public func bridgeStatus() -> CoreMIDIEndpointStatusV1 {
        lock.lock()
        let online = client != 0 && commands != 0 && status != 0
        lock.unlock()
        return .init(
            commandsName: Self.commandsName,
            commandsUniqueID: Self.commandsUniqueID,
            statusName: Self.statusName,
            statusUniqueID: Self.statusUniqueID,
            online: online
        )
    }

    deinit { stop() }

    private static func dispose(
        client: MIDIClientRef,
        commands: MIDIEndpointRef,
        status: MIDIEndpointRef
    ) {
        if commands != 0 { MIDIEndpointDispose(commands) }
        if status != 0 { MIDIEndpointDispose(status) }
        if client != 0 { MIDIClientDispose(client) }
    }
}

public enum CoreMIDIError: Error, Equatable, Sendable {
    case createClient(OSStatus)
    case createCommands(OSStatus)
    case createStatus(OSStatus)
    case setUniqueID(OSStatus)
}
