import Darwin
import Dispatch
import Foundation

enum GuiQuitDecision {
    case accepted
    case busy(String)
    case unavailable(String)
}

final class GuiLifecycleGate {
    private let lock = NSLock()
    private var activeOperation: String?

    func tryBegin(_ operation: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard activeOperation == nil else { return false }
        activeOperation = operation
        return true
    }

    func end() {
        lock.lock()
        activeOperation = nil
        lock.unlock()
    }

    var isBusy: Bool {
        lock.lock()
        defer { lock.unlock() }
        return activeOperation != nil
    }
}

private let guiControlProtocolVersion = 1

func guiControlAudit(_ event: String) {
    fputs("gui-control event=\(event) pid=\(getpid())\n", stderr)
}

private struct GuiControlLease: Codable {
    let version: Int
    let instanceId: String
    let pid: Int32
    let bundleId: String
    let state: String
}

private struct GuiControlRequest: Decodable {
    let version: Int
    let method: String
    let requestId: String
}

private struct GuiControlResponse: Encodable {
    let version: Int
    let requestId: String
    let accepted: Bool
    let code: String?
    let message: String?
}

final class GuiControlServer {
    private let dataDir: URL
    private let ipcDir: URL
    private let socketURL: URL
    private let stateURL: URL
    private let lockURL: URL
    private let bundleId: String
    private let instanceId = UUID().uuidString
    private let requestQuit: () -> GuiQuitDecision
    private let queue = DispatchQueue(label: "com.ivan.televybackup.gui-control", qos: .userInitiated)
    private let clientQueue = DispatchQueue(
        label: "com.ivan.televybackup.gui-control.clients",
        qos: .utility,
        attributes: .concurrent
    )

    private var listenerFd: Int32 = -1
    private var lifecycleLockFd: Int32 = -1
    private var listenerSource: DispatchSourceRead?

    init(dataDir: URL, bundleId: String, requestQuit: @escaping () -> GuiQuitDecision) {
        self.dataDir = dataDir
        ipcDir = dataDir.appendingPathComponent("ipc", isDirectory: true)
        socketURL = ipcDir.appendingPathComponent("gui.sock")
        stateURL = ipcDir.appendingPathComponent("gui.state.json")
        lockURL = ipcDir.appendingPathComponent("gui.lock")
        self.bundleId = bundleId
        self.requestQuit = requestQuit
    }

    deinit {
        stop(rewriteState: false)
    }

    func start() -> String? {
        guard isOwnedDirectory(dataDir.path) else {
            return "GUI control data directory is unsafe."
        }
        if let failure = preparePrivateIPCDirectory() {
            return failure
        }
        if FileManager.default.fileExists(atPath: stateURL.path), !isPrivateRegularFile(stateURL.path) {
            return "Existing GUI control lease is unsafe."
        }
        guard acquireLifecycleLock() else {
            return "Another GUI controller already owns this data directory."
        }
        guard removeRecoverableSocket() else {
            closeLifecycleLock()
            return "Existing GUI control socket is unsafe or cannot be removed."
        }

        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else {
            closeLifecycleLock()
            return "GUI control socket could not be created."
        }
        listenerFd = fd
        guard setNonBlocking(fd), bindUnixSocket(fd, path: socketURL.path), listen(fd, 8) == 0 else {
            stop(rewriteState: false)
            return "GUI control socket could not listen."
        }
        guard chmod(socketURL.path, mode_t(0o600)) == 0, writeLease(state: "running") else {
            stop(rewriteState: false)
            return "GUI control lease could not be written safely."
        }

        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: queue)
        source.setEventHandler { [weak self] in
            self?.acceptPendingClients()
        }
        source.setCancelHandler {}
        source.resume()
        listenerSource = source
        guiControlAudit("listener.started")
        return nil
    }

    func markStopped() {
        guard lifecycleLockFd >= 0 else { return }
        _ = writeLease(state: "stopped")
        closeListener()
        guiControlAudit("lease.stopped")
    }

    private func stop(rewriteState: Bool) {
        if rewriteState {
            _ = writeLease(state: "stopped")
        }
        closeListener()
        closeLifecycleLock()
    }

    private func closeListener() {
        listenerSource?.cancel()
        listenerSource = nil
        if listenerFd >= 0 {
            _ = close(listenerFd)
            listenerFd = -1
        }
        if isSocket(socketURL.path) {
            _ = unlink(socketURL.path)
        }
    }

    private func closeLifecycleLock() {
        if lifecycleLockFd >= 0 {
            _ = flock(lifecycleLockFd, LOCK_UN)
            _ = close(lifecycleLockFd)
            lifecycleLockFd = -1
        }
    }

    private func preparePrivateIPCDirectory() -> String? {
        let manager = FileManager.default
        if manager.fileExists(atPath: ipcDir.path) {
            guard isPrivateDirectory(ipcDir.path) else {
                return "GUI control directory is not a private directory."
            }
        } else {
            do {
                try manager.createDirectory(
                    at: ipcDir,
                    withIntermediateDirectories: true,
                    attributes: [.posixPermissions: 0o700]
                )
            } catch {
                return "GUI control directory could not be created."
            }
        }
        guard chmod(ipcDir.path, mode_t(0o700)) == 0 else {
            return "GUI control directory permissions could not be restricted."
        }
        return nil
    }

    private func acquireLifecycleLock() -> Bool {
        if FileManager.default.fileExists(atPath: lockURL.path), !isPrivateRegularFile(lockURL.path) {
            return false
        }
        let fd = open(lockURL.path, O_CREAT | O_RDWR | O_NOFOLLOW, mode_t(0o600))
        guard fd >= 0 else { return false }
        guard fchmod(fd, mode_t(0o600)) == 0, flock(fd, LOCK_EX | LOCK_NB) == 0 else {
            _ = close(fd)
            return false
        }
        lifecycleLockFd = fd
        return true
    }

    private func removeRecoverableSocket() -> Bool {
        guard FileManager.default.fileExists(atPath: socketURL.path) else { return true }
        guard isSocket(socketURL.path) else { return false }
        return unlink(socketURL.path) == 0
    }

    private func writeLease(state: String) -> Bool {
        if FileManager.default.fileExists(atPath: stateURL.path), !isPrivateRegularFile(stateURL.path) {
            return false
        }
        let lease = GuiControlLease(
            version: guiControlProtocolVersion,
            instanceId: instanceId,
            pid: getpid(),
            bundleId: bundleId,
            state: state
        )
        guard let data = try? JSONEncoder().encode(lease) else { return false }
        let fd = open(stateURL.path, O_CREAT | O_WRONLY | O_TRUNC | O_NOFOLLOW, mode_t(0o600))
        guard fd >= 0 else { return false }
        defer { _ = close(fd) }

        var info = stat()
        let kindIsRegular = fstat(fd, &info) == 0 && (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG)
        guard kindIsRegular, info.st_uid == getuid(), fchmod(fd, mode_t(0o600)) == 0 else {
            return false
        }
        return writeAll(data, to: fd)
    }

    private func writeAll(_ data: Data, to fd: Int32) -> Bool {
        data.withUnsafeBytes { rawBuffer in
            guard var pointer = rawBuffer.baseAddress?.assumingMemoryBound(to: UInt8.self) else {
                return data.isEmpty
            }
            var remaining = rawBuffer.count
            while remaining > 0 {
                let written = Darwin.write(fd, pointer, remaining)
                guard written > 0 else { return false }
                pointer = pointer.advanced(by: written)
                remaining -= written
            }
            return true
        }
    }

    private func acceptPendingClients() {
        while true {
            var address = sockaddr()
            var length = socklen_t(MemoryLayout<sockaddr>.size)
            let client = accept(listenerFd, &address, &length)
            if client < 0 {
                if errno == EAGAIN || errno == EWOULDBLOCK { return }
                return
            }
            clientQueue.async { [weak self] in
                self?.handleClient(client)
            }
        }
    }

    private func handleClient(_ client: Int32) {
        defer { _ = close(client) }
        guard setBlocking(client) else { return }
        setClientTimeout(client)
        guard let line = readLine(from: client), let requestData = line.data(using: .utf8),
              let request = try? JSONDecoder().decode(GuiControlRequest.self, from: requestData)
        else {
            return
        }

        guard request.version == guiControlProtocolVersion, request.method == "gui.quit" else {
            writeResponse(
                GuiControlResponse(
                    version: guiControlProtocolVersion,
                    requestId: request.requestId,
                    accepted: false,
                    code: "gui.unavailable",
                    message: "GUI control protocol is incompatible"
                ),
                to: client
            )
            return
        }

        let response: GuiControlResponse
        guiControlAudit("request.gui.quit")
        switch requestQuit() {
        case .accepted:
            guiControlAudit("request.accepted")
            response = GuiControlResponse(
                version: guiControlProtocolVersion,
                requestId: request.requestId,
                accepted: true,
                code: nil,
                message: nil
            )
        case let .busy(message):
            guiControlAudit("request.busy")
            response = GuiControlResponse(
                version: guiControlProtocolVersion,
                requestId: request.requestId,
                accepted: false,
                code: "gui.busy",
                message: message
            )
        case let .unavailable(message):
            guiControlAudit("request.unavailable")
            response = GuiControlResponse(
                version: guiControlProtocolVersion,
                requestId: request.requestId,
                accepted: false,
                code: "gui.unavailable",
                message: message
            )
        }
        writeResponse(response, to: client)
    }

    private func writeResponse(_ response: GuiControlResponse, to client: Int32) {
        guard let data = try? JSONEncoder().encode(response) else { return }
        var payload = data
        payload.append(0x0A)
        payload.withUnsafeBytes { rawBuffer in
            guard var pointer = rawBuffer.baseAddress?.assumingMemoryBound(to: UInt8.self) else { return }
            var remaining = rawBuffer.count
            while remaining > 0 {
                let written = Darwin.write(client, pointer, remaining)
                guard written > 0 else { return }
                remaining -= written
                pointer = pointer.advanced(by: written)
            }
        }
    }

    private func readLine(from client: Int32) -> String? {
        var buffer = [UInt8](repeating: 0, count: 4096)
        let count = Darwin.read(client, &buffer, buffer.count)
        guard count > 0 else { return nil }
        let bytes = buffer.prefix(Int(count)).prefix { $0 != 0x0A }
        return String(bytes: bytes, encoding: .utf8)
    }

    private func setNonBlocking(_ fd: Int32) -> Bool {
        let flags = fcntl(fd, F_GETFL)
        return flags >= 0 && fcntl(fd, F_SETFL, flags | O_NONBLOCK) == 0
    }

    private func setBlocking(_ fd: Int32) -> Bool {
        let flags = fcntl(fd, F_GETFL)
        return flags >= 0 && fcntl(fd, F_SETFL, flags & ~O_NONBLOCK) == 0
    }

    private func setClientTimeout(_ fd: Int32) {
        var timeout = timeval(tv_sec: 2, tv_usec: 0)
        withUnsafePointer(to: &timeout) { pointer in
            _ = setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, pointer, socklen_t(MemoryLayout<timeval>.size))
            _ = setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, pointer, socklen_t(MemoryLayout<timeval>.size))
        }
    }

    private func bindUnixSocket(_ fd: Int32, path: String) -> Bool {
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let maximumPathLength = MemoryLayout.size(ofValue: address.sun_path) - 1
        guard path.utf8.count <= maximumPathLength else { return false }
        _ = path.withCString { pointer in
            strncpy(&address.sun_path.0, pointer, maximumPathLength)
        }
        let length = socklen_t(MemoryLayout<sockaddr_un>.size)
        return withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { addressPointer in
                bind(fd, addressPointer, length) == 0
            }
        }
    }

    private func isPrivateDirectory(_ path: String) -> Bool {
        var info = stat()
        guard lstat(path, &info) == 0 else { return false }
        let kind = info.st_mode & mode_t(S_IFMT)
        return kind == mode_t(S_IFDIR)
            && info.st_uid == getuid()
            && (info.st_mode & mode_t(0o077)) == 0
    }

    private func isOwnedDirectory(_ path: String) -> Bool {
        var info = stat()
        guard lstat(path, &info) == 0 else { return false }
        return (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFDIR)
            && info.st_uid == getuid()
    }

    private func isSymlink(_ path: String) -> Bool {
        var info = stat()
        guard lstat(path, &info) == 0 else { return false }
        return (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFLNK)
    }

    private func isSocket(_ path: String) -> Bool {
        var info = stat()
        guard lstat(path, &info) == 0 else { return false }
        return (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFSOCK)
    }

    private func isPrivateRegularFile(_ path: String) -> Bool {
        var info = stat()
        guard lstat(path, &info) == 0 else { return false }
        return (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG)
            && info.st_uid == getuid()
            && (info.st_mode & mode_t(0o077)) == 0
    }
}
