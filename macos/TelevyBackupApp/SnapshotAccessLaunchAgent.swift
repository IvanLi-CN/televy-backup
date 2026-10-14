import Foundation
import Security
import Darwin

enum SnapshotAccessRuntimeIdentity {
    static func isCurrent(pid: Int32, appPath: String) -> Bool {
        guard pid > 0 else { return false }
        var code: SecCode?
        let attributes = [kSecGuestAttributePid as String: NSNumber(value: pid)] as CFDictionary
        guard SecCodeCopyGuestWithAttributes(nil, attributes, [], &code) == errSecSuccess,
              let code,
              // Dynamic validation compares the kernel's running CDHash with the on-disk
              // signature. Signing information alone reads the replacement file instead.
              SecCodeCheckValidity(code, [], nil) == errSecSuccess else { return false }
        var path: CFURL?
        var staticCode: SecStaticCode?
        guard SecCodeCopyStaticCode(code, [], &staticCode) == errSecSuccess, let staticCode,
              SecCodeCopyPath(staticCode, [], &path) == errSecSuccess, let path else { return false }
        return (path as URL).standardizedFileURL == URL(fileURLWithPath: appPath).standardizedFileURL
    }
}

enum SnapshotAccessLaunchAgent {
    static var registrationLockURL: URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("televybackup-snapshot-access-registration.lock")
    }

    struct CommandResult {
        let status: Int32
        let output: String
    }

    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    static func register(
        service: String,
        domain: String,
        plistPath: String,
        executablePath: String,
        lockURL: URL = registrationLockURL,
        run: ([String]) -> CommandResult,
        isCurrent: (Int32) -> Bool,
        prepareRestart: () throws -> Void
    ) throws {
        // Keep one inode across attempts: unlinking a lock file lets a second process
        // acquire a different inode while the first registration is still running.
        let lock = try acquireRegistrationLock(lockURL)
        defer { _ = close(lock) }
        let loaded = run(["print", service])
        if loaded.status == 0 {
            try useLoadedService(
                loaded, service: service, executablePath: executablePath,
                run: run, isCurrent: isCurrent, prepareRestart: prepareRestart
            )
            return
        }
        let bootstrap = run(["bootstrap", domain, plistPath])
        guard bootstrap.status != 0 else { return }
        // A duplicate bootstrap commonly returns error 5 without an "already loaded"
        // message. Only an inspected owned service permits recovery from that failure.
        let raced = run(["print", service])
        guard raced.status == 0 else { throw failure(bootstrap) }
        try useLoadedService(
            raced, service: service, executablePath: executablePath,
            run: run, isCurrent: isCurrent, prepareRestart: prepareRestart
        )
    }

    private static func acquireRegistrationLock(_ url: URL) throws -> Int32 {
        let fd = open(url.path, O_CREAT | O_RDWR | O_NOFOLLOW | O_CLOEXEC, mode_t(0o600))
        guard fd >= 0 else {
            throw Failure(message: "Snapshot Access registration lock is unavailable")
        }
        var info = stat()
        guard fstat(fd, &info) == 0, (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG),
              info.st_uid == getuid(), info.st_nlink == 1, (info.st_mode & 0o777) == 0o600 else {
            _ = close(fd)
            throw Failure(message: "Snapshot Access registration lock is not a private regular file")
        }
        guard flock(fd, LOCK_EX | LOCK_NB) == 0 else {
            _ = close(fd)
            throw Failure(message: "Snapshot Access registration is already in progress; retry when it finishes")
        }
        return fd
    }

    private static func useLoadedService(
        _ loaded: CommandResult,
        service: String,
        executablePath: String,
        run: ([String]) -> CommandResult,
        isCurrent: (Int32) -> Bool,
        prepareRestart: () throws -> Void
    ) throws {
        guard SnapshotAccessLaunchdState.program(output: loaded.output) == executablePath else {
            throw Failure(message: "Snapshot Access LaunchAgent is owned by another executable")
        }
        let pid = SnapshotAccessLaunchdState.pid(output: loaded.output)
        if let pid, isCurrent(pid) { return }
        // A stopped job has no process to kill. Its journal is recovered on startup.
        if pid != nil { try prepareRestart() }
        let confirmed = run(["print", service])
        guard confirmed.status == 0,
              SnapshotAccessLaunchdState.program(output: confirmed.output) == executablePath,
              SnapshotAccessLaunchdState.pid(output: confirmed.output) == pid else {
            throw Failure(message: "Snapshot Access service changed during registration; retry after it settles")
        }
        let start = run(pid == nil ? ["kickstart", service] : ["kickstart", "-k", service])
        guard start.status == 0 else { throw failure(start) }
    }

    private static func failure(_ result: CommandResult) -> Failure {
        let message = result.output.trimmingCharacters(in: .whitespacesAndNewlines)
        return Failure(message: message.isEmpty ? "launchctl operation failed with status \(result.status)" : message)
    }
}

struct SnapshotAccessRestartStatus: Decodable {
    let serviceReachable: Bool
    let activeLeases: UInt64?
    let pendingCleanup: UInt64?

    func requireIdle() throws {
        guard serviceReachable, let activeLeases, let pendingCleanup else {
            throw SnapshotAccessLaunchAgent.Failure(message: "Cannot verify that Snapshot Access has no active lease; it was not restarted")
        }
        guard activeLeases == 0 && pendingCleanup == 0 else {
            throw SnapshotAccessLaunchAgent.Failure(message: "Snapshot Access is busy with a lease or cleanup; retry when it finishes")
        }
    }
}
