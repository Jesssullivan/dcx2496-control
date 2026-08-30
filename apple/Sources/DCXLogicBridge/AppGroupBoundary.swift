import Darwin
import Foundation

public struct AppGroupLocations: Sendable {
    public let containerURL: URL
    public let socketURL: URL
    public let helperConfigurationURL: URL
    public let transactionRootURL: URL
    public let snapshotRootURL: URL
    public let planRootURL: URL

    public init(
        appGroupIdentifier: String = DCXBridgeContract.appGroupIdentifier,
        fileManager: FileManager = .default
    ) throws {
        guard appGroupIdentifier == DCXBridgeContract.appGroupIdentifier else {
            throw AppGroupBoundaryError.unexpectedAppGroup
        }
        guard let container = fileManager.containerURL(
            forSecurityApplicationGroupIdentifier: appGroupIdentifier
        )?.standardizedFileURL else {
            throw AppGroupBoundaryError.containerUnavailable
        }
        let bridgeDirectory = container.appendingPathComponent("DCXLogicBridge", isDirectory: true)
        let socket = container.appendingPathComponent(
            DCXBridgeContract.socketFileName,
            isDirectory: false
        )
        guard socket.path.utf8.count < UnixSocketAddress.maximumPathBytes else {
            throw AppGroupBoundaryError.socketPathTooLong
        }
        containerURL = container
        socketURL = socket
        helperConfigurationURL = bridgeDirectory.appendingPathComponent(
            DCXBridgeContract.helperConfigurationFileName,
            isDirectory: false
        )
        transactionRootURL = bridgeDirectory.appendingPathComponent("Transactions", isDirectory: true)
        snapshotRootURL = bridgeDirectory.appendingPathComponent("Snapshots", isDirectory: true)
        planRootURL = bridgeDirectory.appendingPathComponent("Plans", isDirectory: true)
    }

    public func prepareBridgeDirectory(fileManager: FileManager = .default) throws {
        let directory = helperConfigurationURL.deletingLastPathComponent()
        try fileManager.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
    }
}

public enum BridgeJSONCodec {
    public static func encoder() -> JSONEncoder {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return encoder
    }

    public static func decoder() -> JSONDecoder {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }

    public static func frame<T: Encodable>(_ value: T) throws -> Data {
        let payload = try encoder().encode(value)
        guard payload.count <= DCXBridgeContract.maximumFrameBytes else {
            throw AppGroupBoundaryError.frameTooLarge
        }
        var size = UInt32(payload.count).bigEndian
        var framed = Data(bytes: &size, count: MemoryLayout<UInt32>.size)
        framed.append(payload)
        return framed
    }

    public static func unframe<T: Decodable>(_ type: T.Type, data: Data) throws -> T {
        guard data.count >= MemoryLayout<UInt32>.size else {
            throw AppGroupBoundaryError.truncatedFrame
        }
        let payloadLength = data.prefix(4).reduce(UInt32.zero) { ($0 << 8) | UInt32($1) }
        guard payloadLength <= DCXBridgeContract.maximumFrameBytes else {
            throw AppGroupBoundaryError.frameTooLarge
        }
        let expected = 4 + Int(payloadLength)
        guard data.count == expected else { throw AppGroupBoundaryError.truncatedFrame }
        return try decoder().decode(type, from: data.dropFirst(4))
    }
}

public final class AppGroupSocketClient: @unchecked Sendable {
    private let socketURL: URL
    private let timeoutSeconds: Int

    public init(socketURL: URL, timeoutSeconds: Int = 135) throws {
        guard (1...135).contains(timeoutSeconds) else {
            throw AppGroupBoundaryError.invalidTimeout
        }
        _ = try UnixSocketAddress(path: socketURL.path)
        self.socketURL = socketURL
        self.timeoutSeconds = timeoutSeconds
    }

    /// This synchronous boundary is intended only for an AU's non-render UI.
    /// Callers must dispatch it away from both the main and render threads.
    public func exchange(_ request: BridgeRequest) throws -> BridgeResponse {
        let descriptor = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard descriptor >= 0 else { throw AppGroupBoundaryError.socketFailure(errno) }
        defer { Darwin.close(descriptor) }

        try SocketIO.configureNoSigPipe(descriptor)
        var timeout = timeval(tv_sec: timeoutSeconds, tv_usec: 0)
        guard setsockopt(
            descriptor,
            SOL_SOCKET,
            SO_RCVTIMEO,
            &timeout,
            socklen_t(MemoryLayout<timeval>.size)
        ) == 0,
        setsockopt(
            descriptor,
            SOL_SOCKET,
            SO_SNDTIMEO,
            &timeout,
            socklen_t(MemoryLayout<timeval>.size)
        ) == 0 else {
            throw AppGroupBoundaryError.socketFailure(errno)
        }

        var address = try UnixSocketAddress(path: socketURL.path).value
        let connected = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(descriptor, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connected == 0 else { throw AppGroupBoundaryError.socketFailure(errno) }

        try SocketIO.writeAll(try BridgeJSONCodec.frame(request), to: descriptor)
        let responseData = try SocketIO.readFrame(from: descriptor)
        let response = try BridgeJSONCodec.unframe(BridgeResponse.self, data: responseData)
        guard response.requestID == request.requestID else {
            throw AppGroupBoundaryError.responseIDMismatch
        }
        return response
    }
}

/// A serial, one-request-per-connection listener. It has no self-start or
/// launchd behavior; the containing app must explicitly start it while active
/// and stop it when it leaves the foreground.
public final class AppGroupSocketServer: @unchecked Sendable {
    public typealias Handler = @Sendable (BridgeRequest) -> BridgeResponse

    private let queue = DispatchQueue(label: "io.tinyland.dcx2496.logic.bridge", qos: .userInitiated)
    private let stateLock = NSLock()
    private var source: DispatchSourceRead?
    private var descriptor: Int32 = -1
    private var listenerLockDescriptor: Int32 = -1
    private var socketURL: URL?
    private var handler: Handler?

    public init() {}

    public func start(socketURL: URL, handler: @escaping Handler) throws {
        stateLock.lock()
        defer { stateLock.unlock() }
        guard source == nil else { throw AppGroupBoundaryError.alreadyListening }

        var address = try UnixSocketAddress(path: socketURL.path).value
        let listenerLockURL = socketURL.appendingPathExtension("lock")
        let listenerLock = Darwin.open(
            listenerLockURL.path,
            O_CREAT | O_RDWR | O_CLOEXEC | O_NOFOLLOW,
            S_IRUSR | S_IWUSR
        )
        guard listenerLock >= 0 else {
            throw AppGroupBoundaryError.socketFailure(errno)
        }
        var ownsListenerLock = true
        defer {
            if ownsListenerLock {
                _ = Darwin.flock(listenerLock, LOCK_UN)
                Darwin.close(listenerLock)
            }
        }
        var lockMetadata = stat()
        guard Darwin.fstat(listenerLock, &lockMetadata) == 0,
              (lockMetadata.st_mode & S_IFMT) == S_IFREG,
              lockMetadata.st_uid == geteuid(),
              lockMetadata.st_mode & 0o077 == 0 else {
            throw AppGroupBoundaryError.socketFailure(EACCES)
        }
        guard Darwin.flock(listenerLock, LOCK_EX | LOCK_NB) == 0 else {
            if errno == EWOULDBLOCK || errno == EAGAIN {
                throw AppGroupBoundaryError.alreadyListening
            }
            throw AppGroupBoundaryError.socketFailure(errno)
        }
        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw AppGroupBoundaryError.socketFailure(errno) }
        var ownsDescriptor = true
        defer {
            if ownsDescriptor { Darwin.close(fd) }
        }

        // Only this exact, validated App Group socket leaf is ever unlinked.
        if Darwin.unlink(socketURL.path) != 0, errno != ENOENT {
            throw AppGroupBoundaryError.socketFailure(errno)
        }
        let bound = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bound == 0 else { throw AppGroupBoundaryError.socketFailure(errno) }
        guard Darwin.chmod(socketURL.path, S_IRUSR | S_IWUSR) == 0 else {
            throw AppGroupBoundaryError.socketFailure(errno)
        }
        guard Darwin.listen(fd, 1) == 0 else {
            throw AppGroupBoundaryError.socketFailure(errno)
        }

        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: queue)
        source.setEventHandler { [weak self] in self?.acceptOne() }
        source.setCancelHandler { Darwin.close(fd) }
        self.descriptor = fd
        listenerLockDescriptor = listenerLock
        self.socketURL = socketURL
        self.handler = handler
        self.source = source
        ownsDescriptor = false
        ownsListenerLock = false
        source.resume()
    }

    public func stop() {
        stateLock.lock()
        let source = self.source
        let socketURL = self.socketURL
        let listenerLock = listenerLockDescriptor
        self.source = nil
        self.socketURL = nil
        handler = nil
        descriptor = -1
        listenerLockDescriptor = -1
        stateLock.unlock()

        source?.cancel()
        if let socketURL {
            _ = Darwin.unlink(socketURL.path)
        }
        if listenerLock >= 0 {
            _ = Darwin.flock(listenerLock, LOCK_UN)
            Darwin.close(listenerLock)
        }
    }

    deinit { stop() }

    private func acceptOne() {
        stateLock.lock()
        let listener = descriptor
        let handler = handler
        stateLock.unlock()
        guard listener >= 0, let handler else { return }

        let client = Darwin.accept(listener, nil, nil)
        guard client >= 0 else { return }
        defer { Darwin.close(client) }

        do {
            try SocketIO.configureNoSigPipe(client)
            try SocketIO.configureTimeout(client, seconds: 5)
            let requestData = try SocketIO.readFrame(from: client)
            let request = try BridgeJSONCodec.unframe(BridgeRequest.self, data: requestData)
            try SocketIO.writeAll(try BridgeJSONCodec.frame(handler(request)), to: client)
        } catch {
            let response = BridgeResponse(
                requestID: "invalid",
                operation: nil,
                error: .init(code: .invalidRequest, message: "request could not be decoded", retryable: false)
            )
            if let framed = try? BridgeJSONCodec.frame(response) {
                try? SocketIO.writeAll(framed, to: client)
            }
        }
    }
}

private struct UnixSocketAddress {
    static let maximumPathBytes = MemoryLayout.size(ofValue: sockaddr_un().sun_path)
    let value: sockaddr_un

    init(path: String) throws {
        let bytes = Array(path.utf8)
        guard !bytes.isEmpty, bytes.count < Self.maximumPathBytes else {
            throw AppGroupBoundaryError.socketPathTooLong
        }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        withUnsafeMutableBytes(of: &address.sun_path) { buffer in
            buffer.initializeMemory(as: UInt8.self, repeating: 0)
            buffer.copyBytes(from: bytes)
        }
        value = address
    }
}

private enum SocketIO {
    static func configureNoSigPipe(_ descriptor: Int32) throws {
        var enabled: Int32 = 1
        guard setsockopt(
            descriptor,
            SOL_SOCKET,
            SO_NOSIGPIPE,
            &enabled,
            socklen_t(MemoryLayout<Int32>.size)
        ) == 0 else {
            throw AppGroupBoundaryError.socketFailure(errno)
        }
    }

    static func configureTimeout(_ descriptor: Int32, seconds: Int) throws {
        var timeout = timeval(tv_sec: seconds, tv_usec: 0)
        guard setsockopt(
            descriptor,
            SOL_SOCKET,
            SO_RCVTIMEO,
            &timeout,
            socklen_t(MemoryLayout<timeval>.size)
        ) == 0,
        setsockopt(
            descriptor,
            SOL_SOCKET,
            SO_SNDTIMEO,
            &timeout,
            socklen_t(MemoryLayout<timeval>.size)
        ) == 0 else {
            throw AppGroupBoundaryError.socketFailure(errno)
        }
    }

    static func writeAll(_ data: Data, to descriptor: Int32) throws {
        try data.withUnsafeBytes { rawBuffer in
            guard let base = rawBuffer.baseAddress else { return }
            var offset = 0
            while offset < rawBuffer.count {
                let written = Darwin.write(descriptor, base.advanced(by: offset), rawBuffer.count - offset)
                if written < 0, errno == EINTR { continue }
                guard written > 0 else { throw AppGroupBoundaryError.socketFailure(errno) }
                offset += written
            }
        }
    }

    static func readFrame(from descriptor: Int32) throws -> Data {
        let header = try readExact(4, from: descriptor)
        let length = header.reduce(UInt32.zero) { ($0 << 8) | UInt32($1) }
        guard length <= DCXBridgeContract.maximumFrameBytes else {
            throw AppGroupBoundaryError.frameTooLarge
        }
        var frame = header
        frame.append(try readExact(Int(length), from: descriptor))
        return frame
    }

    private static func readExact(_ count: Int, from descriptor: Int32) throws -> Data {
        var data = Data(count: count)
        try data.withUnsafeMutableBytes { rawBuffer in
            guard let base = rawBuffer.baseAddress else { return }
            var offset = 0
            while offset < count {
                let received = Darwin.read(descriptor, base.advanced(by: offset), count - offset)
                if received < 0, errno == EINTR { continue }
                guard received > 0 else { throw AppGroupBoundaryError.truncatedFrame }
                offset += received
            }
        }
        return data
    }
}

public enum AppGroupBoundaryError: Error, Equatable, Sendable {
    case unexpectedAppGroup
    case containerUnavailable
    case socketPathTooLong
    case frameTooLarge
    case truncatedFrame
    case invalidTimeout
    case responseIDMismatch
    case alreadyListening
    case socketFailure(Int32)
}
