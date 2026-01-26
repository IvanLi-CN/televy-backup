import AppKit
import Combine
import Darwin
import Foundation
import SwiftUI

fileprivate enum StatusFreshness {
    static let staleMs: Int64 = 5_000
    static let disconnectedMs: Int64 = 60_000
    static let toastMaxAgeSeconds: Int = 15
}

struct VisualEffectView: NSViewRepresentable {
    let material: NSVisualEffectView.Material
    let blendingMode: NSVisualEffectView.BlendingMode
    let state: NSVisualEffectView.State

    func makeNSView(context _: Context) -> NSVisualEffectView {
        let v = NSVisualEffectView()
        v.material = material
        v.blendingMode = blendingMode
        v.state = state
        return v
    }

    func updateNSView(_ nsView: NSVisualEffectView, context _: Context) {
        nsView.material = material
        nsView.blendingMode = blendingMode
        nsView.state = state
    }
}

final class ModelStore {
    static let shared = AppModel()
}

final class AppModel: ObservableObject {
    @Published var sourcePath: String = ""
    @Published var label: String = "manual"
    @Published var chatId: String = ""
    @Published var botTokenDraft: String = ""
    @Published var botTokenDraftIsMasked: Bool = false
    @Published var mtprotoApiId: String = ""
    @Published var mtprotoApiHashDraft: String = ""
    @Published var mtprotoApiHashDraftIsMasked: Bool = false
    @Published var scheduleEnabled: Bool = false
    @Published var scheduleKind: String = "hourly"

    @Published var telegramOk: Bool = false
    @Published var telegramStatusText: String = "Telegram Storage • Offline"
    @Published var botTokenPresent: Bool = false
    @Published var masterKeyPresent: Bool = false
    @Published var mtprotoApiHashPresent: Bool = false
    @Published var mtprotoSessionPresent: Bool = false
    @Published var secretPresenceKnown: Bool = false
    @Published var secretPresenceFetchInFlight: Bool = false
    @Published var telegramValidateOk: Bool? = nil
    @Published var telegramValidateText: String = "Not validated"
    @Published var refreshInFlight: Bool = false

    @Published var toastText: String? = nil
    @Published var toastIsError: Bool = false

    @Published var lastRunOk: Bool? = nil
    @Published var lastRunErrorCode: String? = nil

    @Published var isRunning: Bool = false
    @Published var phase: String = "idle"

    @Published var lastBytesUploaded: Int64 = 0
    @Published var lastBytesDeduped: Int64 = 0
    @Published var lastDurationSeconds: Double = 0
    @Published var lastRunAt: Date?

    @Published var currentBytesUploaded: Int64 = 0
    @Published var currentBytesDeduped: Int64 = 0
    @Published var taskStartedAt: Date?

    @Published var statusSnapshot: StatusSnapshot? = nil
    @Published var statusSnapshotReceivedAt: Date? = nil
    @Published var statusStreamFrozen: Bool = false
    @Published var popoverDesiredHeight: CGFloat = 460

    private let fileLogQueue = DispatchQueue(label: "TelevyBackup.uiLog", qos: .utility)
    private var didWriteStartupLog: Bool = false
    private var didShowUiLogWriteErrorToast: Bool = false
    private var settingsWindow: NSWindow? = nil
    private var developerWindow: NSWindow? = nil

    struct StatusActivityItem: Identifiable {
        let id = UUID()
        let at: Date
        let text: String
    }

    @Published var statusActivity: [StatusActivityItem] = []
    private var statusStaleLevel: Int = 0
    private var statusStaleTimer: DispatchSourceTimer? = nil
    private var statusPollTimer: DispatchSourceTimer? = nil
    private var lastNotifiedRunFinishedAtByTargetId: [String: String] = [:]

    private var statusStreamTask: Process? = nil
    private var statusStreamReconnectWork: DispatchWorkItem? = nil
    private var statusStreamBackoffSeconds: Double = 0.5
    private var daemonTask: Process? = nil
    private var lastDaemonStartAttemptAt: Date? = nil

    private enum PopoverSizing {
        static let maxHeight: CGFloat = 720
        static let chromeHeightEstimate: CGFloat = 240
        static let rowHeight: CGFloat = 62
        static let listInsetTop: CGFloat = 0
        static let listInsetBottom: CGFloat = 0
        static let emptyStateHeight: CGFloat = 276
        static let minHeight: CGFloat = 320
    }

    init() {
        startStatusStaleTimer()
    }

    private enum LegacyConfigWriteError: LocalizedError {
        case v2ConfigDetected

        var errorDescription: String? {
            switch self {
            case .v2ConfigDetected:
                return "This app view uses legacy settings. Open Settings (v2) to edit targets/endpoints."
            }
        }
    }

    func defaultConfigDir() -> URL {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return home
            .appendingPathComponent("Library")
            .appendingPathComponent("Application Support")
            .appendingPathComponent("TelevyBackup")
    }

    func defaultDataDir() -> URL {
        // Keep macOS defaults consistent with docs: data/logs live under Application Support.
        defaultConfigDir()
    }

    private func statusJsonURL() -> URL {
        if let env = ProcessInfo.processInfo.environment["TELEVYBACKUP_DATA_DIR"], !env.isEmpty {
            return URL(fileURLWithPath: env)
                .appendingPathComponent("status")
                .appendingPathComponent("status.json")
        }
        return defaultDataDir()
            .appendingPathComponent("status")
            .appendingPathComponent("status.json")
    }

    private func logDirURL() -> URL {
        if let env = ProcessInfo.processInfo.environment["TELEVYBACKUP_LOG_DIR"], !env.isEmpty {
            return URL(fileURLWithPath: env)
        }
        if let env = ProcessInfo.processInfo.environment["TELEVYBACKUP_DATA_DIR"], !env.isEmpty {
            return URL(fileURLWithPath: env).appendingPathComponent("logs")
        }
        return defaultDataDir().appendingPathComponent("logs")
    }

    func configTomlPath() -> URL {
        if let env = ProcessInfo.processInfo.environment["TELEVYBACKUP_CONFIG_DIR"] {
            return URL(fileURLWithPath: env).appendingPathComponent("config.toml")
        }
        return defaultConfigDir().appendingPathComponent("config.toml")
    }

    func cliPath() -> String? {
        let bundledInMacOS = Bundle.main.bundleURL
            .appendingPathComponent("Contents")
            .appendingPathComponent("MacOS")
            .appendingPathComponent("televybackup-cli")
        if FileManager.default.isExecutableFile(atPath: bundledInMacOS.path) {
            return bundledInMacOS.path
        }
        if let url = Bundle.main.url(forResource: "televybackup", withExtension: nil) {
            return url.path
        }
        if let p = ProcessInfo.processInfo.environment["TELEVYBACKUP_CLI_PATH"], !p.isEmpty {
            return p
        }
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/usr/bin/which")
        task.arguments = ["televybackup"]
        let pipe = Pipe()
        task.standardOutput = pipe
        do {
            try task.run()
            task.waitUntilExit()
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            let out = String(decoding: data, as: UTF8.self)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            return out.isEmpty ? nil : out
        } catch {
            return nil
        }
    }

    func daemonPath() -> String? {
        let bundled = Bundle.main.bundleURL
            .appendingPathComponent("Contents")
            .appendingPathComponent("MacOS")
            .appendingPathComponent("televybackupd")
        if FileManager.default.isExecutableFile(atPath: bundled.path) {
            return bundled.path
        }
        if let p = ProcessInfo.processInfo.environment["TELEVYBACKUP_DAEMON_PATH"], !p.isEmpty {
            return p
        }
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/usr/bin/which")
        task.arguments = ["televybackupd"]
        let pipe = Pipe()
        task.standardOutput = pipe
        do {
            try task.run()
            task.waitUntilExit()
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            let out = String(decoding: data, as: UTF8.self)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            return out.isEmpty ? nil : out
        } catch {
            return nil
        }
    }

    func refresh(userInitiated: Bool = false) {
        if !didWriteStartupLog {
            didWriteStartupLog = true
            appendLog("UI started")
        }
        if userInitiated {
            DispatchQueue.main.async {
                if self.refreshInFlight { return }
                self.refreshInFlight = true
                self.showToast("Refreshing…", isError: false)
            }
        }
        DispatchQueue.global(qos: .utility).async {
            self.refreshSettings(withSecrets: false) {
                guard userInitiated else { return }
                DispatchQueue.main.async {
                    self.refreshInFlight = false
                    self.showToast("Refreshed", isError: false)
                }
            }
        }
    }

    func ensureStatusStreamRunning() {
        DispatchQueue.main.async {
            self.statusStreamReconnectWork?.cancel()
            self.statusStreamReconnectWork = nil
        }

        guard statusStreamTask == nil else { return }
        guard let cli = cliPath() else {
            appendLog("WARN: televybackup not found (falling back to status.json polling)")
            appendStatusActivity("Using status.json polling (CLI not found)")
            ensureStatusPollRunning()
            return
        }

        stopStatusPollIfNeeded()

        appendLog("Starting status stream")

        let task = Process()
        task.executableURL = URL(fileURLWithPath: cli)
        task.arguments = ["--json", "status", "stream"]

        var env = ProcessInfo.processInfo.environment
        if env["TELEVYBACKUP_CONFIG_DIR"] == nil {
            env["TELEVYBACKUP_CONFIG_DIR"] = defaultConfigDir().path
        }
        if env["TELEVYBACKUP_DATA_DIR"] == nil {
            env["TELEVYBACKUP_DATA_DIR"] = defaultDataDir().path
        }
        task.environment = env

        let out = Pipe()
        let err = Pipe()
        task.standardOutput = out
        task.standardError = err

        var stdoutBuf = ""
        var stderrBuf = ""

        out.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            if data.isEmpty { return }
            stdoutBuf += String(decoding: data, as: UTF8.self)
            while let idx = stdoutBuf.firstIndex(of: "\n") {
                let line = String(stdoutBuf[..<idx])
                stdoutBuf.removeSubrange(...idx)
                self.handleOutputLine(line)
            }
        }
        err.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            if data.isEmpty { return }
            stderrBuf += String(decoding: data, as: UTF8.self)
            while let idx = stderrBuf.firstIndex(of: "\n") {
                let line = String(stderrBuf[..<idx])
                stderrBuf.removeSubrange(...idx)
                self.handleOutputLine(line)
            }
        }

        DispatchQueue.global(qos: .utility).async {
            do {
                try task.run()
                task.waitUntilExit()
            } catch {
                self.appendLog("ERROR: failed to start status stream: \(error)")
            }

            DispatchQueue.main.async {
                out.fileHandleForReading.readabilityHandler = nil
                err.fileHandleForReading.readabilityHandler = nil
                self.statusStreamTask = nil
                self.scheduleStatusStreamReconnect()
            }
        }

        statusStreamTask = task
        statusStreamBackoffSeconds = 0.5
    }

    private func stopStatusPollIfNeeded() {
        if let t = statusPollTimer {
            t.cancel()
            statusPollTimer = nil
            appendStatusActivity("Stopped status.json polling (stream active)")
        }
    }

    private func ensureStatusPollRunning() {
        if statusPollTimer != nil { return }
        let t = DispatchSource.makeTimerSource(queue: DispatchQueue.global(qos: .utility))
        t.schedule(deadline: .now() + 0.10, repeating: 1.0)
        t.setEventHandler { [weak self] in
            guard let self else { return }
            let url = self.statusJsonURL()
            guard let data = try? Data(contentsOf: url) else { return }
            guard let snap = try? JSONDecoder().decode(StatusSnapshot.self, from: data) else { return }
            DispatchQueue.main.async {
                if self.statusStreamFrozen { return }
                if self.statusSnapshot?.generatedAt == snap.generatedAt { return }
                self.applyStatusSnapshot(snap)
            }
        }
        statusPollTimer = t
        t.activate()
    }

    func ensureDaemonRunning() {
        let now = Date()
        if let last = lastDaemonStartAttemptAt, now.timeIntervalSince(last) < 3 {
            return
        }
        lastDaemonStartAttemptAt = now

        if isDaemonRunning() {
            return
        }

        // Prefer launchd service if installed (Homebrew services).
        if kickstartLaunchAgent(label: "homebrew.mxcl.televybackupd") {
            appendStatusActivity("Daemon kickstarted via launchd (homebrew.mxcl.televybackupd)")
            return
        }

        // Fallback: spawn a bundled or PATH daemon for local/dev runs.
        guard let daemon = daemonPath() else {
            appendLog("WARN: televybackupd not found (daemon auto-start unavailable)")
            showToast("Daemon not found (install televybackupd)", isError: true)
            return
        }

        if daemonTask?.isRunning == true {
            return
        }

        let task = Process()
        task.executableURL = URL(fileURLWithPath: daemon)

        var env = ProcessInfo.processInfo.environment
        if env["TELEVYBACKUP_CONFIG_DIR"] == nil {
            env["TELEVYBACKUP_CONFIG_DIR"] = defaultConfigDir().path
        }
        if env["TELEVYBACKUP_DATA_DIR"] == nil {
            env["TELEVYBACKUP_DATA_DIR"] = defaultDataDir().path
        }
        task.environment = env

        // Best-effort logging for the spawned daemon (dev fallback).
        let logDir = logDirURL()
        try? FileManager.default.createDirectory(at: logDir, withIntermediateDirectories: true)
        let outPath = logDir.appendingPathComponent("televybackupd.spawned.log").path
        if FileManager.default.fileExists(atPath: outPath) == false {
            _ = FileManager.default.createFile(atPath: outPath, contents: Data())
        }
        if let fh = try? FileHandle(forWritingTo: URL(fileURLWithPath: outPath)) {
            _ = try? fh.seekToEnd()
            let pipe = Pipe()
            pipe.fileHandleForReading.readabilityHandler = { handle in
                let data = handle.availableData
                if data.isEmpty { return }
                try? fh.write(contentsOf: data)
            }
            task.standardOutput = pipe
            task.standardError = pipe
        }

        do {
            try task.run()
            daemonTask = task
            appendStatusActivity("Daemon spawned (\(daemon))")
        } catch {
            appendLog("ERROR: failed to start daemon: \(error)")
            showToast("Failed to start daemon (see ui.log)", isError: true)
        }
    }

    private func isDaemonRunning() -> Bool {
        let res = runCommandCapture(exe: "/usr/bin/pgrep", args: ["-x", "televybackupd"], timeoutSeconds: 2)
        return res.status == 0
    }

    private func kickstartLaunchAgent(label: String) -> Bool {
        let uid = getuid()
        let service = "gui/\(uid)/\(label)"
        let kick = runCommandCapture(exe: "/bin/launchctl", args: ["kickstart", "-k", service], timeoutSeconds: 3)
        if kick.status == 0 {
            return true
        }
        let start = runCommandCapture(exe: "/bin/launchctl", args: ["start", service], timeoutSeconds: 3)
        return start.status == 0
    }

    private func scheduleStatusStreamReconnect() {
        if statusStreamReconnectWork != nil { return }
        let delay = min(statusStreamBackoffSeconds, 30.0)
        statusStreamBackoffSeconds = min(statusStreamBackoffSeconds * 1.8, 30.0)

        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.statusStreamReconnectWork = nil
            self.ensureStatusStreamRunning()
        }
        statusStreamReconnectWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: work)
    }

    func updatePopoverHeightForTargets(targetCount: Int) {
        let listContentHeight: CGFloat
        if targetCount <= 0 {
            listContentHeight = PopoverSizing.emptyStateHeight
        } else {
            listContentHeight = (CGFloat(targetCount) * PopoverSizing.rowHeight)
                + PopoverSizing.listInsetTop + PopoverSizing.listInsetBottom
        }
        let desired = min(PopoverSizing.maxHeight, PopoverSizing.chromeHeightEstimate + listContentHeight)
        let clamped = max(PopoverSizing.minHeight, desired)
        if abs(popoverDesiredHeight - clamped) >= 1 {
            popoverDesiredHeight = clamped
        }
    }

    func targetsListMaxHeight() -> CGFloat {
        max(160, PopoverSizing.maxHeight - PopoverSizing.chromeHeightEstimate)
    }

    func targetsListInsets() -> EdgeInsets {
        EdgeInsets(top: PopoverSizing.listInsetTop, leading: 0, bottom: PopoverSizing.listInsetBottom, trailing: 0)
    }

    func openDeveloperWindow() {
        DispatchQueue.main.async {
            let window: NSWindow
            if let existing = self.developerWindow {
                window = existing
            } else {
                let root = DeveloperWindowRootView()
                    .environmentObject(self)
                let controller = NSHostingController(rootView: root)
                controller.view.wantsLayer = true
                controller.view.layer?.backgroundColor = NSColor.clear.cgColor

                let w = NSWindow(contentViewController: controller)
                w.title = "Developer"
                if #available(macOS 11.0, *) {
                    w.toolbarStyle = .unified
                }
                w.setContentSize(NSSize(width: 860, height: 720))
                w.minSize = NSSize(width: 720, height: 560)
                w.maxSize = NSSize(width: 1600, height: 2000)
                w.styleMask.insert([.titled, .closable, .miniaturizable, .resizable])
                w.isReleasedWhenClosed = false
                w.center()
                self.configureDeveloperWindowIfNeeded(w)
                self.developerWindow = w
                window = w
            }

            self.configureDeveloperWindowIfNeeded(window)
            self.ensureStatusStreamRunning()
            NSApp.activate(ignoringOtherApps: true)
            window.makeKeyAndOrderFront(nil)
        }
    }

    private func configureDeveloperWindowIfNeeded(_ window: NSWindow) {
        if window.isOpaque {
            window.isOpaque = false
        }
        if window.backgroundColor != .clear {
            window.backgroundColor = .clear
        }
        if !window.styleMask.contains(.fullSizeContentView) {
            window.styleMask.insert(.fullSizeContentView)
        }
        if window.appearance?.name != .vibrantLight {
            window.appearance = NSAppearance(named: .vibrantLight)
        }
        window.titlebarAppearsTransparent = true
        window.isMovableByWindowBackground = true

        if let contentView = window.contentView {
            contentView.wantsLayer = true
            contentView.layer?.backgroundColor = NSColor.clear.cgColor
        }
    }

    func copyStatusSnapshotJsonToClipboard() {
        guard let statusSnapshot else { return }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        guard let data = try? encoder.encode(statusSnapshot),
              let text = String(data: data, encoding: .utf8)
        else { return }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(text, forType: .string)
        showToast("Copied status JSON", isError: false)
    }

    func revealStatusSourceInFinder() {
        NSWorkspace.shared.activateFileViewerSelecting([statusJsonURL()])
    }

    private func startStatusStaleTimer() {
        if statusStaleTimer != nil { return }
        let t = DispatchSource.makeTimerSource(queue: DispatchQueue.main)
        t.schedule(deadline: .now() + 0.3, repeating: 0.5)
        t.setEventHandler { [weak self] in
            guard let self else { return }
            guard let snap = self.statusSnapshot else { return }
            let nowMs = Int64(Date().timeIntervalSince1970 * 1000.0)
            let age = max(0, nowMs - snap.generatedAt)
            let level: Int = (age > StatusFreshness.disconnectedMs) ? 2 : ((age > StatusFreshness.staleMs) ? 1 : 0)
            if level != self.statusStaleLevel {
                self.statusStaleLevel = level
                if level == 1 {
                    self.appendStatusActivity("Stale (> \(StatusFreshness.staleMs / 1000)s since generatedAt)")
                } else if level == 2 {
                    self.appendStatusActivity("Disconnected (> \(StatusFreshness.disconnectedMs / 1000)s since generatedAt)")
                } else {
                    self.appendStatusActivity("Fresh (updates resumed)")
                }
            }
        }
        statusStaleTimer = t
        t.activate()
    }

    private func appendStatusActivity(_ text: String) {
        let item = StatusActivityItem(at: Date(), text: text)
        statusActivity.append(item)
        if statusActivity.count > 200 {
            statusActivity.removeFirst(statusActivity.count - 200)
        }
    }

    private func applyStatusSnapshot(_ snap: StatusSnapshot) {
        if statusStreamFrozen { return }
        statusSnapshot = snap
        statusSnapshotReceivedAt = Date()
        appendStatusActivity("Snapshot received (schema=\(snap.schemaVersion), targets=\(snap.targets.count))")
        updatePopoverHeightForTargets(targetCount: snap.targets.count)

        // Surface "what happened" for very short runs: show a toast when we observe a new lastRun.
        for t in snap.targets {
            guard let finishedAt = t.lastRun?.finishedAt, !finishedAt.isEmpty else { continue }
            let prevFinishedAt = lastNotifiedRunFinishedAtByTargetId[t.targetId]
            if prevFinishedAt == nil {
                // First time we see this target's lastRun; don't toast on startup.
                lastNotifiedRunFinishedAtByTargetId[t.targetId] = finishedAt
                continue
            }
            guard prevFinishedAt != finishedAt else { continue }
            lastNotifiedRunFinishedAtByTargetId[t.targetId] = finishedAt

            guard let d = parseIsoDate(finishedAt) else { continue }
            let ageSeconds = Int(Date().timeIntervalSince(d))
            if ageSeconds < 0 || ageSeconds > StatusFreshness.toastMaxAgeSeconds { continue }

            let label = (t.label?.isEmpty == false) ? (t.label ?? t.targetId) : t.targetId
            let runStatus = t.lastRun?.status ?? "unknown"
            if runStatus == "failed" {
                let code = t.lastRun?.errorCode ?? "failed"
                showToast("\(label) • Failed (\(code))", isError: true)
                continue
            }
            if runStatus == "succeeded" {
                var parts: [String] = []
                if let files = t.lastRun?.filesIndexed, files > 0 {
                    parts.append("\(files) files")
                }
                if let uploaded = t.lastRun?.bytesUploaded, uploaded > 0 {
                    parts.append("+\(formatBytes(uploaded))")
                } else if let saved = t.lastRun?.bytesDeduped, saved > 0 {
                    parts.append("saved \(formatBytes(saved))")
                }
                if let dur = t.lastRun?.durationSeconds, dur > 0 {
                    parts.append(formatDuration(dur))
                }
                let suffix = parts.isEmpty ? "" : (" • " + parts.joined(separator: " • "))
                showToast("\(label) • Backup finished\(suffix)", isError: false)
            }
        }

    }

    private func parseIsoDate(_ s: String) -> Date? {
        let f1 = ISO8601DateFormatter()
        f1.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = f1.date(from: s) { return d }
        let f2 = ISO8601DateFormatter()
        f2.formatOptions = [.withInternetDateTime]
        return f2.date(from: s)
    }

    func refreshSecretsPresence(force: Bool = false) {
        DispatchQueue.main.async {
            if self.secretPresenceFetchInFlight { return }
            if self.secretPresenceKnown && !force { return }
            self.secretPresenceFetchInFlight = true
            DispatchQueue.global(qos: .utility).async {
                self.refreshSettings(withSecrets: true)
                DispatchQueue.main.async {
                    self.secretPresenceFetchInFlight = false
                }
            }
        }
    }

    private func updateTelegramStatus() {
        let apiId = Int(mtprotoApiId.trimmingCharacters(in: .whitespacesAndNewlines)) ?? 0
        let configured = botTokenPresent
            && masterKeyPresent
            && !chatId.isEmpty
            && apiId > 0
            && mtprotoApiHashPresent
        telegramOk = configured
        if telegramValidateOk == true {
            telegramStatusText = "Telegram Storage • Connected"
        } else if configured {
            telegramStatusText = "Telegram Storage • Not validated"
        } else {
            telegramStatusText = "Telegram Storage • Offline"
        }
    }

    static func maskedTokenPlaceholder() -> String {
        String(repeating: "•", count: 18)
    }

    func saveSettings() {
        do {
            try writeConfigToml()
            appendLog("Saved: \(configTomlPath().path)")
            DispatchQueue.global(qos: .utility).async {
                self.refreshSettings(withSecrets: false)
            }
        } catch {
            appendLog("ERROR: save config failed: \(error)")
            if error is LegacyConfigWriteError {
                showToast("Save disabled for settings v2 (open Settings)", isError: true)
            } else {
                showToast("Save failed (see ui.log)", isError: true)
            }
        }
    }

    func setBotToken() {
        guard let cli = cliPath() else {
            appendLog("ERROR: televybackup not found (set TELEVYBACKUP_CLI_PATH or install it)")
            return
        }
        if botTokenDraftIsMasked {
            showToast("Paste a new token to replace", isError: true)
            return
        }
        let token = botTokenDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !token.isEmpty else {
            appendLog("ERROR: bot token is empty")
            showToast("Bot token is empty", isError: true)
            return
        }
        showToast("Saving token…", isError: false)
        runProcess(
            exe: cli,
            args: ["--json", "secrets", "set-telegram-bot-token"],
            stdin: token + "\n",
            updateTaskState: false,
            onExit: { status in
                if status == 0 {
                    self.botTokenDraft = Self.maskedTokenPlaceholder()
                    self.botTokenDraftIsMasked = true
                    self.showToast("Saved (encrypted)", isError: false)
                    self.botTokenPresent = true
                    self.secretPresenceKnown = true
                    self.updateTelegramStatus()
                } else {
                    self.showToast("Failed to save token (see ui.log)", isError: true)
                }
            }
        )
    }

    func initMasterKey() {
        guard let cli = cliPath() else {
            appendLog("ERROR: televybackup not found (set TELEVYBACKUP_CLI_PATH or install it)")
            return
        }
        showToast("Ensuring master key…", isError: false)

        runProcess(
            exe: cli,
            args: ["--json", "secrets", "migrate-keychain"],
            updateTaskState: false,
            onExit: { status in
                self.refreshSecretsPresence(force: true)
                if status != 0 {
                    self.showToast("Migration failed (see ui.log)", isError: true)
                    self.updateTelegramStatus()
                    return
                }

                if self.masterKeyPresent {
                    self.showToast("Master key ready", isError: false)
                    self.updateTelegramStatus()
                    return
                }

                self.runProcess(
                    exe: cli,
                    args: ["--json", "secrets", "init-master-key"],
                    updateTaskState: false,
                    onExit: { status2 in
                        self.refreshSecretsPresence(force: true)
                        if status2 == 0 {
                            self.showToast("Master key created (encrypted)", isError: false)
                        } else {
                            self.showToast("Failed to init master key (see ui.log)", isError: true)
                        }
                        self.updateTelegramStatus()
                    }
                )
            }
        )
    }

    func migrateKeychainSecrets() {
        guard let cli = cliPath() else {
            appendLog("ERROR: televybackup not found (set TELEVYBACKUP_CLI_PATH or install it)")
            return
        }
        showToast("Migrating Keychain secrets…", isError: false)
        runProcess(
            exe: cli,
            args: ["--json", "secrets", "migrate-keychain"],
            updateTaskState: false,
            onExit: { status in
                self.refreshSecretsPresence(force: true)
                if status == 0 {
                    self.showToast("Migration complete", isError: false)
                } else {
                    self.showToast("Migration failed (see ui.log)", isError: true)
                }
                self.updateTelegramStatus()
            }
        )
    }

    func setMtprotoApiHash() {
        guard let cli = cliPath() else {
            appendLog("ERROR: televybackup not found (set TELEVYBACKUP_CLI_PATH or install it)")
            return
        }
        if mtprotoApiHashDraftIsMasked {
            showToast("Paste a new api_hash to replace", isError: true)
            return
        }
        let apiHash = mtprotoApiHashDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !apiHash.isEmpty else {
            appendLog("ERROR: mtproto api_hash is empty")
            showToast("API hash is empty", isError: true)
            return
        }
        showToast("Saving api_hash…", isError: false)
        runProcess(
            exe: cli,
            args: ["--json", "secrets", "set-telegram-api-hash"],
            stdin: apiHash + "\n",
            updateTaskState: false,
            onExit: { status in
                self.refreshSecretsPresence(force: true)
                if status == 0 {
                    self.mtprotoApiHashDraft = Self.maskedTokenPlaceholder()
                    self.mtprotoApiHashDraftIsMasked = true
                    self.showToast("Saved (encrypted)", isError: false)
                    self.mtprotoApiHashPresent = true
                } else {
                    self.showToast("Failed to save api_hash (see ui.log)", isError: true)
                }
                self.updateTelegramStatus()
            }
        )
    }

    func clearMtprotoSession() {
        guard let cli = cliPath() else {
            appendLog("ERROR: televybackup not found (set TELEVYBACKUP_CLI_PATH or install it)")
            return
        }
        showToast("Clearing MTProto session…", isError: false)
        runProcess(
            exe: cli,
            args: ["--json", "secrets", "clear-telegram-mtproto-session"],
            updateTaskState: false,
            onExit: { status in
                self.refreshSecretsPresence(force: true)
                if status == 0 {
                    self.showToast("Session cleared", isError: false)
                } else {
                    self.showToast("Failed to clear session (see ui.log)", isError: true)
                }
                self.updateTelegramStatus()
            }
        )
    }

    func testConnection() {
        guard let cli = cliPath() else {
            appendLog("ERROR: televybackup not found (set TELEVYBACKUP_CLI_PATH or install it)")
            return
        }
        showToast("Testing connection…", isError: false)
        DispatchQueue.main.async {
            self.telegramValidateOk = nil
            self.telegramValidateText = "Testing…"
        }
        runProcess(
            exe: cli,
            args: ["--json", "telegram", "validate"],
            timeoutSeconds: 90,
            onTimeout: {
                self.telegramValidateOk = false
                self.telegramValidateText = "Timed out"
                self.showToast("Test timed out (check network)", isError: true)
            },
            updateTaskState: false,
            onExit: { status in
                if status == 0 {
                    self.telegramValidateOk = true
                    self.telegramValidateText = "Connected"
                    self.showToast("Telegram OK", isError: false)
                } else {
                    self.telegramValidateOk = false
                    self.telegramValidateText = "Failed (see ui.log)"
                    self.showToast("Test failed (see ui.log)", isError: true)
                }
                self.refreshSecretsPresence(force: true)
                self.updateTelegramStatus()
            }
        )
    }

    func runBackupNow() {
        guard let cli = cliPath() else {
            appendLog("ERROR: televybackup not found (set TELEVYBACKUP_CLI_PATH or install it)")
            return
        }
        guard !sourcePath.isEmpty else {
            appendLog("ERROR: source path is empty")
            showToast("Choose a source folder first", isError: true)
            return
        }
        runProcess(exe: cli, args: ["--events", "backup", "run", "--source", sourcePath, "--label", label])
    }

    func chooseSourceFolder() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = false
        panel.prompt = "Choose"
        panel.message = "Choose a folder to back up"
        if !sourcePath.isEmpty {
            panel.directoryURL = URL(fileURLWithPath: sourcePath)
        }
        let response = panel.runModal()
        if response == .OK, let url = panel.url {
            self.sourcePath = url.path
            showToast("Source selected", isError: false)
            saveSettings()
        }
    }

    func openLogs() {
        let dir = logDirURL()
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        } catch {
            showToast("Failed to create logs folder", isError: true)
            return
        }
        if !NSWorkspace.shared.open(dir) {
            showToast("Failed to open logs folder", isError: true)
        }
    }

    private func writeConfigToml() throws {
        let path = configTomlPath()
        if let existing = try? String(contentsOf: path, encoding: .utf8) {
            if existing.contains("version = 2")
                || existing.contains("[[targets]]")
                || existing.contains("[[telegram_endpoints]]")
            {
                throw LegacyConfigWriteError.v2ConfigDetected
            }
        }

        let dir = configTomlPath().deletingLastPathComponent()
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let apiId = Int(mtprotoApiId.trimmingCharacters(in: .whitespacesAndNewlines)) ?? 0

        let toml = """
        sources = [\(tomlStringArray([sourcePath].filter { !$0.isEmpty }))]

        [schedule]
        enabled = \(scheduleEnabled ? "true" : "false")
        kind = \(tomlString(scheduleKind))
        hourly_minute = 0
        daily_at = "02:00"
        timezone = "local"

        [retention]
        keep_last_snapshots = 7

        [chunking]
        min_bytes = 1048576
        avg_bytes = 4194304
        max_bytes = 10485760

        [telegram]
        mode = "mtproto"
        chat_id = \(tomlString(chatId))
        bot_token_key = "telegram.bot_token"

        [telegram.mtproto]
        api_id = \(apiId)
        api_hash_key = "telegram.mtproto.api_hash"
        session_key = "telegram.mtproto.session"

        [telegram.rate_limit]
        max_concurrent_uploads = 2
        min_delay_ms = 250
        """
        try toml.write(to: path, atomically: true, encoding: .utf8)
    }

    private func refreshSettings(withSecrets: Bool, completion: (() -> Void)? = nil) {
        guard let cli = cliPath() else {
            DispatchQueue.main.async { completion?() }
            return
        }
        var args = ["--json", "settings", "get"]
        if withSecrets { args.append("--with-secrets") }
        let timeout = withSecrets ? 180.0 : 10.0
        let result = runCommandCapture(exe: cli, args: args, timeoutSeconds: timeout)
        if result.status != 0 {
            appendLog("ERROR: settings get failed: exit=\(result.status) reason=\(result.reason.rawValue)")
            if !result.stderr.isEmpty {
                appendLog("stderr: \(result.stderr.prefix(2000))")
            }
        }

        let combined = (!result.stdout.isEmpty ? result.stdout : result.stderr)
        guard let obj = parseJsonObject(combined) else {
            appendLog("ERROR: settings get JSON parse failed")
            if !result.stdout.isEmpty { appendLog("stdout: \(result.stdout.prefix(2000))") }
            if !result.stderr.isEmpty { appendLog("stderr: \(result.stderr.prefix(2000))") }

            if let fallback = readConfigTomlBasics() {
                appendLog("WARN: falling back to config.toml parsing")
                DispatchQueue.main.async {
                    if let first = fallback.sources.first {
                        self.sourcePath = first
                    }
                    self.scheduleEnabled = fallback.scheduleEnabled
                    self.scheduleKind = fallback.scheduleKind
                    self.chatId = fallback.chatId
                    self.mtprotoApiId = fallback.apiId > 0 ? String(fallback.apiId) : ""
                    self.updateTelegramStatus()
                    completion?()
                }
            } else {
                DispatchQueue.main.async { completion?() }
            }
            return
        }

        let settings = obj["settings"] as? [String: Any]
        let targets = (settings?["targets"] as? [[String: Any]]) ?? []
        let endpoints = (settings?["telegram_endpoints"] as? [[String: Any]]) ?? []
        let schedule = (settings?["schedule"] as? [String: Any]) ?? [:]
        let telegram = (settings?["telegram"] as? [String: Any]) ?? [:]
        let mtproto = (telegram["mtproto"] as? [String: Any]) ?? [:]
        let apiIdNum = (mtproto["api_id"] as? NSNumber)?.intValue ?? 0

        let selectedTarget = targets.first
        let selectedSourcePath = (selectedTarget?["source_path"] as? String) ?? ""
        let selectedEndpointId = (selectedTarget?["endpoint_id"] as? String) ?? ""
        let selectedEndpoint = endpoints.first(where: { ($0["id"] as? String) == selectedEndpointId })
        let chatId = (selectedEndpoint?["chat_id"] as? String) ?? ""

        let scheduleEnabled = (schedule["enabled"] as? Bool) ?? false
        let scheduleKind = (schedule["kind"] as? String) ?? "hourly"

        DispatchQueue.main.async {
            self.sourcePath = selectedSourcePath
            self.scheduleEnabled = scheduleEnabled
            self.scheduleKind = scheduleKind

            self.chatId = chatId
            self.mtprotoApiId = apiIdNum > 0 ? String(apiIdNum) : ""
            if withSecrets {
                let secrets = obj["secrets"] as? [String: Any]
                let masterPresent = (secrets?["masterKeyPresent"] as? Bool) ?? false
                let apiHashPresent = (secrets?["telegramMtprotoApiHashPresent"] as? Bool) ?? false

                let botByEndpoint = secrets?["telegramBotTokenPresentByEndpoint"] as? [String: Any]
                let sessionByEndpoint =
                    secrets?["telegramMtprotoSessionPresentByEndpoint"] as? [String: Any]
                let botAny = botByEndpoint?[selectedEndpointId]
                let sessionAny = sessionByEndpoint?[selectedEndpointId]
                let botPresent =
                    (botAny as? Bool) ?? ((botAny as? NSNumber)?.boolValue ?? false)
                let sessionPresent =
                    (sessionAny as? Bool) ?? ((sessionAny as? NSNumber)?.boolValue ?? false)

                self.botTokenPresent = botPresent
                self.masterKeyPresent = masterPresent
                self.mtprotoApiHashPresent = apiHashPresent
                self.mtprotoSessionPresent = sessionPresent
                self.secretPresenceKnown = true
            }
            if withSecrets {
                if !self.botTokenPresent || self.chatId.isEmpty {
                    self.telegramValidateOk = false
                    self.telegramValidateText = "Missing token / chat id"
                } else if apiIdNum <= 0 && !self.mtprotoApiHashPresent {
                    self.telegramValidateOk = false
                    self.telegramValidateText = "Missing api_id / api_hash"
                } else if apiIdNum <= 0 {
                    self.telegramValidateOk = false
                    self.telegramValidateText = "Missing api_id"
                } else if !self.mtprotoApiHashPresent {
                    self.telegramValidateOk = false
                    self.telegramValidateText = "Missing api_hash"
                } else if self.telegramValidateText == "Missing token / chat id"
                    || self.telegramValidateText == "Missing api_id / api_hash"
                    || self.telegramValidateText == "Missing api_id"
                    || self.telegramValidateText == "Missing api_hash"
                {
                    self.telegramValidateOk = nil
                    self.telegramValidateText = "Not validated"
                }
            } else {
                if self.chatId.isEmpty {
                    self.telegramValidateOk = false
                    self.telegramValidateText = "Missing chat id"
                } else if self.telegramValidateText == "Missing token / chat id" || self.telegramValidateText == "Missing chat id" {
                    self.telegramValidateOk = nil
                    self.telegramValidateText = "Not validated"
                }
            }
            if self.secretPresenceKnown {
                if self.botTokenPresent && self.botTokenDraft.isEmpty {
                    self.botTokenDraft = Self.maskedTokenPlaceholder()
                    self.botTokenDraftIsMasked = true
                }
                if !self.botTokenPresent && self.botTokenDraftIsMasked {
                    self.botTokenDraft = ""
                    self.botTokenDraftIsMasked = false
                }
                if self.mtprotoApiHashPresent && self.mtprotoApiHashDraft.isEmpty {
                    self.mtprotoApiHashDraft = Self.maskedTokenPlaceholder()
                    self.mtprotoApiHashDraftIsMasked = true
                }
                if !self.mtprotoApiHashPresent && self.mtprotoApiHashDraftIsMasked {
                    self.mtprotoApiHashDraft = ""
                    self.mtprotoApiHashDraftIsMasked = false
                }
            }
            self.updateTelegramStatus()
            completion?()
        }
    }

	    func openSettingsWindow() {
	        DispatchQueue.main.async {
	            let window: NSWindow
	            if let existing = self.settingsWindow {
	                window = existing
	            } else {
	                let root = SettingsWindowRootView()
	                    .environmentObject(self)
	                let controller = NSHostingController(rootView: root)
	                controller.view.wantsLayer = true
	                controller.view.layer?.backgroundColor = NSColor.clear.cgColor
	                let w = NSWindow(contentViewController: controller)
	                w.title = "Settings"
	                w.titleVisibility = .hidden
	                if #available(macOS 11.0, *) {
	                    w.toolbarStyle = .unified
	                }
	                let fixedWidth: CGFloat = 820
	                w.setContentSize(NSSize(width: fixedWidth, height: 560))
	                // Keep a consistent top bar layout: allow vertical resize, lock width.
	                w.minSize = NSSize(width: fixedWidth, height: 520)
	                w.maxSize = NSSize(width: fixedWidth, height: 2000)
	                w.styleMask.insert([.titled, .closable, .miniaturizable, .resizable])
	                w.isReleasedWhenClosed = false
	                w.center()
	                self.configureSettingsWindowIfNeeded(w)
	                self.settingsWindow = w
	                window = w
	            }

	            self.configureSettingsWindowIfNeeded(window)
	            NSApp.activate(ignoringOtherApps: true)
	            window.makeKeyAndOrderFront(nil)
	        }
	    }

	    private func configureSettingsWindowIfNeeded(_ window: NSWindow) {
	        if window.isOpaque {
	            window.isOpaque = false
	        }
	        if window.backgroundColor != .clear {
	            window.backgroundColor = .clear
	        }
	        if !window.styleMask.contains(.fullSizeContentView) {
	            window.styleMask.insert(.fullSizeContentView)
	        }
	        if window.appearance?.name != .vibrantLight {
	            window.appearance = NSAppearance(named: .vibrantLight)
	        }
	        window.titlebarAppearsTransparent = true
	        window.isMovableByWindowBackground = true

	        // Lock Settings window width (only allow vertical resizing).
	        let fixedWidth: CGFloat = 820
	        if window.minSize.width != fixedWidth || window.maxSize.width != fixedWidth {
	            window.minSize = NSSize(width: fixedWidth, height: window.minSize.height)
	            window.maxSize = NSSize(width: fixedWidth, height: max(window.maxSize.height, 2000))
	        }

	        if let contentView = window.contentView {
	            contentView.wantsLayer = true
	            contentView.layer?.backgroundColor = NSColor.clear.cgColor
	        }
	    }

    private func appendLog(_ line: String) {
        let trimmed = sanitizeLogLine(line.trimmingCharacters(in: .newlines))
        guard !trimmed.isEmpty else { return }
        appendFileLog(trimmed)
    }

    private func sanitizeLogLine(_ line: String) -> String {
        // Redact any api.telegram.org URL segment to avoid leaking secrets.
        // Keep the rest of the line intact (e.g., JSON stderr) by replacing only the URL substring.
        let patterns = [
            #"https?://api\.telegram\.org[^\s"'()\[\]{}<>,]+"#,
            #"api\.telegram\.org(?=[/\?])[^\s"'()\[\]{}<>,]+"#,
        ]

        var out = line
        for pattern in patterns {
            guard let re = try? NSRegularExpression(pattern: pattern, options: []) else { continue }
            let range = NSRange(out.startIndex..<out.endIndex, in: out)
            out = re.stringByReplacingMatches(in: out, range: range, withTemplate: "[redacted_url]")
        }
        return out
    }

    private func uiLogFileURL() -> URL {
        logDirURL().appendingPathComponent("ui.log")
    }

    private func appendFileLog(_ message: String) {
        let ts = ISO8601DateFormatter().string(from: Date())
        let line = "\(ts) \(message)\n"
        let url = uiLogFileURL()
        fileLogQueue.async {
            do {
                let dir = url.deletingLastPathComponent()
                try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
                if !FileManager.default.fileExists(atPath: url.path) {
                    try Data().write(to: url, options: .atomic)
                }
                let handle = try FileHandle(forWritingTo: url)
                try handle.seekToEnd()
                if let data = line.data(using: .utf8) {
                    try handle.write(contentsOf: data)
                }
                try handle.close()
            } catch {
                DispatchQueue.main.async {
                    if !self.didShowUiLogWriteErrorToast {
                        self.didShowUiLogWriteErrorToast = true
                        self.showToast("Failed to write ui.log", isError: true)
                    }
                }
            }
        }
    }

    private func parseJsonObject(_ output: String) -> [String: Any]? {
        let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { return nil }
        if let data = trimmed.data(using: .utf8),
           let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        {
            return obj
        }
        for line in trimmed.split(separator: "\n").reversed() {
            let s = line.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !s.isEmpty else { continue }
            if let data = s.data(using: .utf8),
               let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
            {
                return obj
            }
        }
        return nil
    }

    private struct ConfigBasics {
        let sources: [String]
        let scheduleEnabled: Bool
        let scheduleKind: String
        let chatId: String
        let apiId: Int
    }

    private func readConfigTomlBasics() -> ConfigBasics? {
        let path = configTomlPath()
        guard let text = try? String(contentsOf: path, encoding: .utf8) else { return nil }

        var section: String? = nil
        var sources: [String] = []
        var scheduleEnabled: Bool = false
        var scheduleKind: String = "hourly"
        var chatId: String = ""
        var apiId: Int = 0

        for raw in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            if line.isEmpty || line.hasPrefix("#") { continue }

            if line.hasPrefix("[") && line.hasSuffix("]") {
                let name = line.dropFirst().dropLast().trimmingCharacters(in: .whitespacesAndNewlines)
                section = name.isEmpty ? nil : String(name)
                continue
            }

            guard let eq = line.firstIndex(of: "=") else { continue }
            let key = line[..<eq].trimmingCharacters(in: .whitespacesAndNewlines)
            let value = line[line.index(after: eq)...].trimmingCharacters(in: .whitespacesAndNewlines)

            if section == nil && key == "sources" {
                if value.hasPrefix("[") && value.hasSuffix("]") {
                    let inner = value.dropFirst().dropLast()
                    let parts = inner.split(separator: ",")
                    sources = parts.compactMap { part in
                        let v = part.trimmingCharacters(in: .whitespacesAndNewlines)
                        guard v.hasPrefix("\"") && v.hasSuffix("\"") else { return nil }
                        return String(v.dropFirst().dropLast())
                    }
                }
                continue
            }

            if section == "schedule" {
                if key == "enabled" {
                    scheduleEnabled = (value == "true")
                } else if key == "kind" {
                    if value.hasPrefix("\"") && value.hasSuffix("\"") {
                        scheduleKind = String(value.dropFirst().dropLast())
                    }
                }
                continue
            }

            if section == "telegram" {
                if key == "chat_id" {
                    if value.hasPrefix("\"") && value.hasSuffix("\"") {
                        chatId = String(value.dropFirst().dropLast())
                    }
                }
                continue
            }

            if section == "telegram.mtproto" {
                if key == "api_id" {
                    apiId = Int(value) ?? 0
                }
                continue
            }
        }

        return ConfigBasics(
            sources: sources,
            scheduleEnabled: scheduleEnabled,
            scheduleKind: scheduleKind,
            chatId: chatId,
            apiId: apiId
        )
    }

    func runCommandCapture(
        exe: String,
        args: [String],
        stdin: String? = nil,
        timeoutSeconds: Double? = nil
    ) -> (stdout: String, stderr: String, status: Int32, reason: Process.TerminationReason) {
        let task = Process()
        task.executableURL = URL(fileURLWithPath: exe)
        task.arguments = args

        let out = Pipe()
        let err = Pipe()
        task.standardOutput = out
        task.standardError = err

        if let stdin {
            let input = Pipe()
            task.standardInput = input
            input.fileHandleForWriting.write(stdin.data(using: .utf8) ?? Data())
            try? input.fileHandleForWriting.close()
        }

        var status: Int32 = 1
        var reason: Process.TerminationReason = .exit

        do {
            try task.run()

            if let timeoutSeconds {
                let pid = task.processIdentifier
                DispatchQueue.global(qos: .userInitiated)
                    .asyncAfter(deadline: .now() + timeoutSeconds) {
                        guard task.isRunning, task.processIdentifier == pid else { return }
                        self.appendLog("ERROR: command timed out after \(Int(timeoutSeconds))s")
                        task.terminate()
                        DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + 2.0) {
                            guard task.isRunning, task.processIdentifier == pid else { return }
                            _ = kill(pid_t(pid), SIGKILL)
                        }
                    }
            }

            task.waitUntilExit()
            status = task.terminationStatus
            reason = task.terminationReason
        } catch {
            return ("", "\(error)", 1, .exit)
        }

        let outData = out.fileHandleForReading.readDataToEndOfFile()
        let errData = err.fileHandleForReading.readDataToEndOfFile()
        let stdout = String(decoding: outData, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        let stderr = String(decoding: errData, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        return (stdout, stderr, status, reason)
    }

    private func handleOutputLine(_ line: String) {
        guard let data = line.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return }

        if let type = obj["type"] as? String, type == "status.snapshot" {
            if let snap = try? JSONDecoder().decode(StatusSnapshot.self, from: data) {
                DispatchQueue.main.async { self.applyStatusSnapshot(snap) }
            }
            return
        }

        appendLog(line)

        if let code = obj["code"] as? String {
            DispatchQueue.main.async {
                self.lastRunErrorCode = code
            }
            return
        }

        guard let type = obj["type"] as? String else { return }

        if type == "task.progress" {
            let phase = obj["phase"] as? String ?? "running"
            let bytesUploaded = (obj["bytesUploaded"] as? NSNumber)?.int64Value ?? 0
            let bytesDeduped = (obj["bytesDeduped"] as? NSNumber)?.int64Value ?? 0
            DispatchQueue.main.async { self.phase = phase }
            DispatchQueue.main.async {
                self.currentBytesUploaded = bytesUploaded
                self.currentBytesDeduped = bytesDeduped
            }
            return
        }

        if type == "task.state" {
            let state = obj["state"] as? String ?? ""
            let kind = obj["kind"] as? String ?? ""
            DispatchQueue.main.async {
                if state == "running" {
                    self.isRunning = true
                    self.phase = kind
                    self.currentBytesUploaded = 0
                    self.currentBytesDeduped = 0
                    self.taskStartedAt = Date()
                    self.lastRunOk = nil
                    self.lastRunErrorCode = nil
                } else {
                    self.isRunning = false
                    self.phase = "idle"
                    self.taskStartedAt = nil
                }
            }

            if state == "succeeded",
               let result = obj["result"] as? [String: Any]
            {
                let bytesUploaded = (result["bytesUploaded"] as? NSNumber)?.int64Value ?? 0
                let bytesDeduped = (result["bytesDeduped"] as? NSNumber)?.int64Value ?? 0
                let duration = (result["durationSeconds"] as? NSNumber)?.doubleValue ?? 0
                DispatchQueue.main.async {
                    self.lastRunOk = true
                    self.lastRunErrorCode = nil
                    self.lastBytesUploaded = bytesUploaded
                    self.lastBytesDeduped = bytesDeduped
                    self.lastDurationSeconds = duration
                    self.lastRunAt = Date()
                    self.currentBytesUploaded = bytesUploaded
                    self.currentBytesDeduped = bytesDeduped
                }
                refreshSettings(withSecrets: false)
            }
        }
    }

    private func showToast(_ text: String, isError: Bool) {
        DispatchQueue.main.async {
            self.toastText = text
            self.toastIsError = isError
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.8) {
            if self.toastText == text {
                self.toastText = nil
            }
        }
    }

    private func runProcess(
        exe: String,
        args: [String],
        stdin: String? = nil,
        timeoutSeconds: Double? = nil,
        onTimeout: (() -> Void)? = nil,
        updateTaskState: Bool = true,
        onExit: ((Int32) -> Void)? = nil
    ) {
        if updateTaskState {
            DispatchQueue.main.async {
                self.isRunning = true
                self.phase = "running"
            }
        }
        appendLog("$ \(exe) \(args.joined(separator: " "))")

        let task = Process()
        task.executableURL = URL(fileURLWithPath: exe)
        task.arguments = args

        var env = ProcessInfo.processInfo.environment
        if env["TELEVYBACKUP_CONFIG_DIR"] == nil {
            env["TELEVYBACKUP_CONFIG_DIR"] = defaultConfigDir().path
        }
        if env["TELEVYBACKUP_DATA_DIR"] == nil {
            env["TELEVYBACKUP_DATA_DIR"] = defaultDataDir().path
        }
        task.environment = env

        let out = Pipe()
        let err = Pipe()
        task.standardOutput = out
        task.standardError = err

        if let stdin {
            let input = Pipe()
            task.standardInput = input
            input.fileHandleForWriting.write(stdin.data(using: .utf8) ?? Data())
            try? input.fileHandleForWriting.close()
        }

        var stdoutBuf = ""
        var stderrBuf = ""

        out.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            if data.isEmpty { return }
            stdoutBuf += String(decoding: data, as: UTF8.self)
            while let idx = stdoutBuf.firstIndex(of: "\n") {
                let line = String(stdoutBuf[..<idx])
                stdoutBuf.removeSubrange(...idx)
                self.handleOutputLine(line)
            }
        }
        err.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            if data.isEmpty { return }
            stderrBuf += String(decoding: data, as: UTF8.self)
            while let idx = stderrBuf.firstIndex(of: "\n") {
                let line = String(stderrBuf[..<idx])
                stderrBuf.removeSubrange(...idx)
                self.handleOutputLine(line)
            }
        }

        DispatchQueue.global(qos: .userInitiated).async {
            var status: Int32 = 1
            do {
                try task.run()

                if let timeoutSeconds {
                    let pid = task.processIdentifier
                    DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + timeoutSeconds) {
                        guard task.isRunning, task.processIdentifier == pid else { return }
                        self.handleOutputLine("ERROR: process timed out after \(Int(timeoutSeconds))s")
                        if let onTimeout {
                            DispatchQueue.main.async { onTimeout() }
                        }
                        task.terminate()
                        DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + 2.0) {
                            guard task.isRunning, task.processIdentifier == pid else { return }
                            _ = kill(pid_t(pid), SIGKILL)
                        }
                    }
                }

                task.waitUntilExit()
                status = task.terminationStatus
            } catch {
                self.handleOutputLine("ERROR: failed to run process: \(error)")
            }
            DispatchQueue.main.async {
                if updateTaskState {
                    self.isRunning = false
                    self.phase = "idle"
                    if status != 0 {
                        self.lastRunOk = false
                        self.lastBytesUploaded = self.currentBytesUploaded
                        self.lastBytesDeduped = self.currentBytesDeduped
                        if let startedAt = self.taskStartedAt {
                            self.lastDurationSeconds = Date().timeIntervalSince(startedAt)
                        }
                        self.lastRunAt = Date()
                        self.taskStartedAt = nil
                        self.showToast("Backup failed", isError: true)
                    }
                }
                out.fileHandleForReading.readabilityHandler = nil
                err.fileHandleForReading.readabilityHandler = nil
            }
            if !stdoutBuf.isEmpty {
                self.handleOutputLine(stdoutBuf.trimmingCharacters(in: .newlines))
            }
            if !stderrBuf.isEmpty {
                self.handleOutputLine(stderrBuf.trimmingCharacters(in: .newlines))
            }
            if let onExit {
                DispatchQueue.main.async { onExit(status) }
            }
        }
    }

    private func controlDirURL() -> URL {
        if let env = ProcessInfo.processInfo.environment["TELEVYBACKUP_DATA_DIR"], !env.isEmpty {
            return URL(fileURLWithPath: env).appendingPathComponent("control")
        }
        return defaultDataDir().appendingPathComponent("control")
    }

    func triggerBackupNowAllEnabled() {
        ensureDaemonRunning()
        let anyRunning = statusSnapshot?.targets.contains(where: { $0.state == "running" }) ?? false

        DispatchQueue.global(qos: .utility).async {
            let dir = self.controlDirURL()
            let path = dir.appendingPathComponent("backup-now")
            do {
                try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)

                let tmp = dir.appendingPathComponent("backup-now.tmp.\(getpid())")
                let payload = "manual backup trigger at \(ISO8601DateFormatter().string(from: Date()))\n"
                if let data = payload.data(using: .utf8) {
                    try data.write(to: tmp, options: .atomic)
                } else {
                    throw NSError(domain: "TelevyBackup", code: 1, userInfo: [
                        NSLocalizedDescriptionKey: "failed to encode trigger payload",
                    ])
                }
                if FileManager.default.fileExists(atPath: path.path) {
                    try? FileManager.default.removeItem(at: path)
                }
                try FileManager.default.moveItem(at: tmp, to: path)

                DispatchQueue.main.async {
                    self.appendStatusActivity("Manual backup trigger written (all enabled targets)")
                    self.showToast(anyRunning ? "Queued backup…" : "Starting backup…", isError: false)
                }
            } catch {
                DispatchQueue.main.async {
                    self.appendLog("ERROR: failed to write manual backup trigger: \(error)")
                    self.showToast("Failed to start backup (see ui.log)", isError: true)
                }
            }
        }
    }
}

private func tomlString(_ s: String) -> String {
    "\"\(s.replacingOccurrences(of: "\"", with: "\\\""))\""
}

private func tomlStringArray(_ items: [String]) -> String {
    items.map(tomlString).joined(separator: ", ")
}

private func formatBytes(_ bytes: Int64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"]
    var value = Double(bytes)
    var idx = 0
    while value >= 1024 && idx < units.count - 1 {
        value /= 1024
        idx += 1
    }
    if idx == 0 { return "\(Int(value)) \(units[idx])" }
    return String(format: "%.1f %@", value, units[idx])
}

private func formatDuration(_ seconds: Double) -> String {
    if seconds <= 0 { return "-" }
    if seconds < 60 { return String(format: "%.0fs", seconds) }
    let m = Int(seconds) / 60
    let s = Int(seconds) % 60
    return String(format: "%dm%02ds", m, s)
}

struct StatusLED: View {
    let color: Color

    var body: some View {
        ZStack {
            Circle()
                .fill(color)
                .opacity(0.95)
                .frame(width: 10, height: 10)
            Circle()
                .fill(color)
                .opacity(0.16)
                .frame(width: 18, height: 18)
        }
    }
}

struct GlassCard<Content: View>: View {
    let title: String
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title)
                .font(.system(size: 11, weight: .heavy))
                .foregroundStyle(.secondary)
                .tracking(0.8)
            content
        }
        .padding(12)
        .background(Color.white.opacity(0.18), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .strokeBorder(Color.white.opacity(0.22), lineWidth: 1)
        )
    }
}

struct PopoverRootView: View {
    @EnvironmentObject var model: AppModel

    var body: some View {
        ZStack {
            VisualEffectView(material: .popover, blendingMode: .behindWindow, state: .active)
                .ignoresSafeArea()

            // Liquid-glass shell (one shape, no nested rounding)
            ContainerRelativeShape()
                .fill(glassFill)
                .ignoresSafeArea()
            ContainerRelativeShape()
                .fill(glassHighlight)
                .blendMode(.screen)
                .ignoresSafeArea()
            ContainerRelativeShape()
                .strokeBorder(glassStroke, lineWidth: 1)
                .ignoresSafeArea()

            VStack(alignment: .leading, spacing: 12) {
                header
                OverviewView()
            }
            .padding(16)
        }
        .frame(width: 360)
        .preferredColorScheme(.light)
        .overlay(alignment: .bottom) {
            if let toast = model.toastText {
                ToastPill(text: toast, isError: model.toastIsError)
                    .padding(12)
                    .allowsHitTesting(false)
                    .transition(.opacity)
            }
        }
    }

    private var header: some View {
        let nowMs = Int64(Date().timeIntervalSince1970 * 1000.0)
        let snap = model.statusSnapshot
        let ageMs = snap == nil ? Int64.max : max(0, nowMs - (snap?.generatedAt ?? 0))
        let isDaemon = (snap?.source.kind == "daemon")
        let statusText: String = {
            if snap == nil { return "Status • Disconnected" }
            if !isDaemon { return "Status • Stale" }
            if ageMs > StatusFreshness.disconnectedMs { return "Status • Disconnected" }
            if ageMs > StatusFreshness.staleMs { return "Status • Stale" }
            return "Status • Live"
        }()
        let statusColor: Color = {
            if snap == nil { return .red }
            if !isDaemon { return .orange }
            if ageMs > StatusFreshness.disconnectedMs { return .red }
            if ageMs > StatusFreshness.staleMs { return .orange }
            return .green
        }()

        return VStack(spacing: 0) {
            HStack(alignment: .center, spacing: 10) {
                ZStack {
                    RoundedRectangle(cornerRadius: 9, style: .continuous)
                        .fill(Color.white.opacity(0.32))
                        .frame(width: 28, height: 28)
                        .overlay(
                            RoundedRectangle(cornerRadius: 9, style: .continuous)
                                .strokeBorder(Color.white.opacity(0.35), lineWidth: 1)
                        )
                    Circle()
                        .fill(Color.blue)
                        .frame(width: 12, height: 12)
                        .opacity(0.95)
                }

                VStack(alignment: .leading, spacing: 2) {
                    Text("TelevyBackup")
                        .font(.system(size: 15, weight: .bold))
                        .foregroundStyle(Color.primary.opacity(0.95))
                    Text(statusText)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(Color.secondary.opacity(0.95))
                }

                Spacer()

                Button {
                    model.triggerBackupNowAllEnabled()
                } label: {
                    Image(systemName: "play.fill")
                        .font(.system(size: 11, weight: .heavy))
                        .frame(width: 22, height: 22)
                        .background(Color.white.opacity(0.26), in: RoundedRectangle(cornerRadius: 10))
                        .overlay(
                            RoundedRectangle(cornerRadius: 10)
                                .strokeBorder(Color.white.opacity(0.35), lineWidth: 1)
                        )
                }
                .buttonStyle(.plain)
                .help("Backup now (all enabled targets)")

                StatusLED(color: statusColor)

                Button {
                    model.openSettingsWindow()
                } label: {
                    Image(systemName: "gearshape.fill")
                        .font(.system(size: 13, weight: .semibold))
                        .frame(width: 22, height: 22)
                        .background(Color.white.opacity(0.26), in: RoundedRectangle(cornerRadius: 10))
                        .overlay(
                            RoundedRectangle(cornerRadius: 10)
                                .strokeBorder(Color.white.opacity(0.35), lineWidth: 1)
                        )
                }
                .buttonStyle(.plain)
            }
            .padding(.bottom, 12)

            Rectangle()
                .fill(Color.black.opacity(0.08))
                .frame(height: 1)
        }
        .padding(.bottom, 2)
    }

    private var glassFill: LinearGradient {
        LinearGradient(
            colors: [
                Color.white.opacity(0.18),
                Color.white.opacity(0.08),
            ],
            startPoint: .top,
            endPoint: .bottom
        )
    }

    private var glassStroke: LinearGradient {
        LinearGradient(
            colors: [
                Color.white.opacity(0.55),
                Color.white.opacity(0.22),
                Color.black.opacity(0.08),
            ],
            startPoint: .topLeading,
            endPoint: .bottomTrailing
        )
    }

    private var glassHighlight: LinearGradient {
        LinearGradient(
            colors: [
                Color.white.opacity(0.12),
                Color.white.opacity(0.00),
            ],
            startPoint: .topLeading,
            endPoint: .bottomTrailing
        )
    }
}

struct OverviewView: View {
    @EnvironmentObject var model: AppModel

    var body: some View {
        let snap = model.statusSnapshot
        let nowMs = Int64(Date().timeIntervalSince1970 * 1000.0)

        VStack(alignment: .leading, spacing: 14) {
            networkSection(snap: snap, nowMs: nowMs)
            targetsSection(snap: snap, nowMs: nowMs)
        }
        .onAppear {
            model.ensureDaemonRunning()
            model.ensureStatusStreamRunning()
            model.updatePopoverHeightForTargets(targetCount: snap?.targets.count ?? 0)
        }
    }

    private func networkSection(snap: StatusSnapshot?, nowMs: Int64) -> some View {
        let staleAgeMs = snap == nil ? Int64.max : max(0, nowMs - (snap?.generatedAt ?? 0))
        let isDaemon = (snap?.source.kind == "daemon")
        let disconnected = isDaemon && staleAgeMs > StatusFreshness.disconnectedMs
        let hideValues = disconnected || (!isDaemon) || staleAgeMs > StatusFreshness.staleMs

        return VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline) {
                Text("NETWORK")
                    .font(.system(size: 11, weight: .heavy))
                    .foregroundStyle(.secondary)
                    .tracking(0.8)
                Spacer()
                Text(updatedText(snap: snap, nowMs: nowMs))
                    .font(.system(size: 11, weight: .heavy))
                    .foregroundStyle(Color.secondary.opacity(0.92))
            }

            HStack(spacing: 12) {
                statChip(
                    title: "↑ Up",
                    titleColor: Color.blue.opacity(0.92),
                    value: formatRateChip(snap?.global.up.bytesPerSecond, hide: hideValues),
                    session: formatSessionChip(snap?.global.upTotal.bytes, hide: hideValues)
                )
                statChip(
                    title: "↓ Down",
                    titleColor: Color.secondary.opacity(0.88),
                    value: formatRateChip(snap?.global.down.bytesPerSecond, hide: hideValues),
                    session: formatSessionChip(snap?.global.downTotal.bytes, hide: hideValues)
                )
            }
        }
    }

    private func statChip(title: String, titleColor: Color, value: String, session: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.system(size: 11, weight: .heavy))
                .foregroundStyle(titleColor)
            Text(value)
                .font(.system(size: 14, weight: .heavy, design: .monospaced))
                .foregroundStyle(Color.primary.opacity(0.92))
            Text(session)
                .font(.system(size: 11, weight: .heavy))
                .foregroundStyle(Color.secondary.opacity(0.88))
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(Color.white.opacity(0.18), lineWidth: 1)
        )
    }

    private func targetsSection(snap: StatusSnapshot?, nowMs: Int64) -> some View {
        let targets = snap?.targets ?? []

        return VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline) {
                Text("TARGETS")
                    .font(.system(size: 11, weight: .heavy))
                    .foregroundStyle(.secondary)
                    .tracking(0.8)
                Spacer()
                Text("\(targets.count) targets")
                    .font(.system(size: 11, weight: .heavy))
                    .foregroundStyle(Color.secondary.opacity(0.92))
            }

            targetsContainer(snap: snap, targets: targets, nowMs: nowMs)
        }
    }

    private func updatedText(snap: StatusSnapshot?, nowMs: Int64) -> String {
        guard let snap else { return "updated —" }
        let ageMs = max(0, nowMs - snap.generatedAt)
        if ageMs < 1_000 { return "updated \(ageMs)ms" }
        if ageMs < 60_000 { return "updated \(ageMs / 1000)s" }
        if ageMs < 3_600_000 { return "updated \(ageMs / 60_000)m" }
        return "updated \(ageMs / 3_600_000)h"
    }

    private func targetsContainer(snap: StatusSnapshot?, targets: [StatusTarget], nowMs: Int64) -> some View {
        let container = RoundedRectangle(cornerRadius: 12, style: .continuous)
        let height: CGFloat = {
            if targets.isEmpty {
                return 276
            }
            let desired = CGFloat(targets.count) * 62
            return min(model.targetsListMaxHeight(), desired)
        }()

        return ZStack {
            LinearGradient(
                colors: [
                    Color.white.opacity(0.16),
                    Color.white.opacity(0.10),
                ],
                startPoint: .top,
                endPoint: .bottom
            )
            .clipShape(container)
            .overlay(container.strokeBorder(Color.white.opacity(0.16), lineWidth: 1))

            if targets.isEmpty {
                if snap == nil {
                    waitingForStatusEmptyState()
                } else {
                    targetsEmptyState()
                }
            } else if let snap {
                TargetsListView(
                    targets: targets,
                    snapshotGeneratedAtMs: snap.generatedAt,
                    snapshotSourceKind: snap.source.kind
                )
                .padding(.vertical, 0)
            } else {
                waitingForStatusEmptyState()
            }
        }
        .frame(height: height)
    }

    private func targetsEmptyState() -> some View {
        VStack(spacing: 10) {
            ZStack {
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .fill(Color.white.opacity(0.12))
                    .overlay(
                        RoundedRectangle(cornerRadius: 18, style: .continuous)
                            .strokeBorder(Color.white.opacity(0.14), lineWidth: 1)
                    )
                Image(systemName: "folder.badge.plus")
                    .font(.system(size: 26, weight: .semibold))
                    .foregroundStyle(Color.secondary.opacity(0.7))
            }
            .frame(width: 80, height: 80)
            .padding(.bottom, 2)

            Text("No targets yet")
                .font(.system(size: 14, weight: .heavy))
                .foregroundStyle(Color.primary.opacity(0.92))
            Text("Add a folder to back up in Settings.")
                .font(.system(size: 11.5, weight: .semibold))
                .foregroundStyle(Color.secondary.opacity(0.92))

            Button("Open Settings…") { model.openSettingsWindow() }
                .buttonStyle(.borderedProminent)
                .tint(.blue)
                .controlSize(.large)
                .frame(width: 152)
                .padding(.top, 6)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func waitingForStatusEmptyState() -> some View {
        VStack(spacing: 10) {
            ZStack {
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .fill(Color.white.opacity(0.12))
                    .overlay(
                        RoundedRectangle(cornerRadius: 18, style: .continuous)
                            .strokeBorder(Color.white.opacity(0.14), lineWidth: 1)
                    )
                Image(systemName: "waveform.path.ecg")
                    .font(.system(size: 26, weight: .semibold))
                    .foregroundStyle(Color.secondary.opacity(0.7))
            }
            .frame(width: 80, height: 80)
            .padding(.bottom, 2)

            Text("Waiting for status…")
                .font(.system(size: 14, weight: .heavy))
                .foregroundStyle(Color.primary.opacity(0.92))
            Text("Starting daemon and reading snapshots.")
                .font(.system(size: 11.5, weight: .semibold))
                .foregroundStyle(Color.secondary.opacity(0.92))

            Button("Open Settings…") { model.openSettingsWindow() }
                .buttonStyle(.bordered)
                .controlSize(.large)
                .frame(width: 152)
                .padding(.top, 6)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func formatSessionChip(_ bytes: Int64?, hide: Bool) -> String {
        if hide { return "Session —" }
        guard let bytes else { return "Session —" }
        return "Session \(formatBytes(bytes))"
    }

    private func formatRateChip(_ bps: Int64?, hide: Bool) -> String {
        if hide { return "—" }
        guard let bps else { return "—" }
        return "\(formatBytes(bps))/s"
    }
}

private struct TargetsListView: View {
    @EnvironmentObject var model: AppModel
    let targets: [StatusTarget]
    let snapshotGeneratedAtMs: Int64
    let snapshotSourceKind: String

    @State private var contentMinY: CGFloat = 0
    @State private var contentHeight: CGFloat = 0
    @State private var containerHeight: CGFloat = 0

    private struct ContentMetrics: Equatable {
        let minY: CGFloat
        let height: CGFloat
    }

    private struct ContentMetricsKey: PreferenceKey {
        static var defaultValue: ContentMetrics = .init(minY: 0, height: 0)
        static func reduce(value: inout ContentMetrics, nextValue: () -> ContentMetrics) {
            value = nextValue()
        }
    }

    private struct ContainerHeightKey: PreferenceKey {
        static var defaultValue: CGFloat = 0
        static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
            value = nextValue()
        }
    }

    var body: some View {
        ScrollView(.vertical) {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(Array(targets.enumerated()), id: \.element.id) { idx, t in
                    TargetRowView(target: t, snapshotGeneratedAtMs: snapshotGeneratedAtMs, snapshotSourceKind: snapshotSourceKind)
                    if idx != targets.count - 1 {
                        Rectangle()
                            .fill(Color.black.opacity(0.08))
                            .frame(height: 1)
                            .padding(.horizontal, 12)
                    }
                }
            }
            .background(
                GeometryReader { proxy in
                    Color.clear.preference(
                        key: ContentMetricsKey.self,
                        value: ContentMetrics(
                            minY: proxy.frame(in: .named("targetsScroll")).minY,
                            height: proxy.size.height
                        )
                    )
                }
            )
        }
        .coordinateSpace(name: "targetsScroll")
        .background(
            GeometryReader { proxy in
                Color.clear.preference(key: ContainerHeightKey.self, value: proxy.size.height)
            }
        )
        .onPreferenceChange(ContentMetricsKey.self) { v in
            contentMinY = v.minY
            contentHeight = v.height
        }
        .onPreferenceChange(ContainerHeightKey.self) { h in
            containerHeight = h
        }
        .mask(fadeMask)
    }

    private var fadeMask: some View {
        let fade: CGFloat = 16
        let canScroll = contentHeight > containerHeight + 1
        let atTop = contentMinY >= -0.5
        let bottomEdge = contentMinY + contentHeight
        let atBottom = bottomEdge <= containerHeight + 0.5
        let showTop = canScroll && !atTop
        let showBottom = canScroll && !atBottom

        return VStack(spacing: 0) {
            LinearGradient(
                colors: [Color.black.opacity(showTop ? 0 : 1), Color.black],
                startPoint: .top,
                endPoint: .bottom
            )
            .frame(height: fade)
            Rectangle().fill(Color.black).frame(maxHeight: .infinity)
            LinearGradient(
                colors: [Color.black, Color.black.opacity(showBottom ? 0 : 1)],
                startPoint: .top,
                endPoint: .bottom
            )
            .frame(height: fade)
        }
    }
}

private struct TargetRowView: View {
    @EnvironmentObject var model: AppModel
    let target: StatusTarget
    let snapshotGeneratedAtMs: Int64
    let snapshotSourceKind: String

    var body: some View {
        let nowMs = Int64(Date().timeIntervalSince1970 * 1000.0)
        let staleAgeMs = max(0, nowMs - snapshotGeneratedAtMs)
        let isDaemon = (snapshotSourceKind == "daemon")
        let disconnected = isDaemon && staleAgeMs > StatusFreshness.disconnectedMs

        let running = (target.state == "running") && !disconnected && (isDaemon && staleAgeMs <= StatusFreshness.disconnectedMs)

        return VStack(alignment: .leading, spacing: 0) {
            if running {
                runningRow(nowMs: nowMs, staleAgeMs: staleAgeMs, isDaemon: isDaemon, disconnected: disconnected)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color.white.opacity(0.10))
            } else {
                idleRow(nowMs: nowMs, staleAgeMs: staleAgeMs, isDaemon: isDaemon, disconnected: disconnected)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 10)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    private func displayLabel() -> String {
        if let label = target.label, !label.isEmpty { return label }
        return shortId(target.targetId)
    }

    private func shortId(_ id: String) -> String {
        if id.count <= 8 { return id }
        return String(id.suffix(8))
    }

    private func stateBadge(staleAgeMs: Int64, isDaemon: Bool) -> some View {
        let (text, c): (String, Color) = {
            if !isDaemon || staleAgeMs > StatusFreshness.staleMs { return ("Stale", .orange) }
            if target.state == "running" { return ("Running", .blue) }
            if target.state == "failed" || target.lastRun?.status == "failed" { return ("Failed", .red) }
            if target.state == "stale" { return ("Stale", .orange) }
            return ("Idle", .secondary)
        }()

        return HStack(spacing: 6) {
            Circle()
                .fill(c)
                .frame(width: 6, height: 6)
                .opacity(0.90)
            Text(text)
                .font(.system(size: 11.5, weight: .heavy))
                .foregroundStyle(c.opacity(0.95))
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 3)
        .background(Color.black.opacity(0.05), in: RoundedRectangle(cornerRadius: 9.5, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 9.5, style: .continuous)
                .strokeBorder(Color.white.opacity(0.16), lineWidth: 1)
        )
    }

    private func formatRateInline(_ bps: Int64?) -> String {
        guard let bps else { return "—" }
        return "\(formatBytes(bps))/s"
    }

    private func runningRow(nowMs: Int64, staleAgeMs: Int64, isDaemon: Bool, disconnected: Bool) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(displayLabel())
                    .font(.system(size: 13.5, weight: .heavy))
                    .foregroundStyle(Color.primary.opacity(0.92))
                stateBadge(staleAgeMs: staleAgeMs, isDaemon: isDaemon)
                Spacer()
                Text("↑ \(formatRateInline(target.up.bytesPerSecond))")
                    .font(.system(size: 12, weight: .heavy, design: .monospaced))
                    .foregroundStyle(Color.primary.opacity(0.92))
            }

            Text(runningSummary(nowMs: nowMs))
                .font(.system(size: 11.5, weight: .semibold, design: .monospaced))
                .foregroundStyle(Color.secondary.opacity(0.92))

            progressBar
        }
    }

    private func idleRow(nowMs: Int64, staleAgeMs: Int64, isDaemon: Bool, disconnected: Bool) -> some View {
        let failed = target.state == "failed" || target.lastRun?.status == "failed"
        let showStale = (!isDaemon) || staleAgeMs > StatusFreshness.staleMs

        return VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(displayLabel())
                    .font(.system(size: 13.5, weight: .heavy))
                    .foregroundStyle(Color.primary.opacity(0.92))
                stateBadge(staleAgeMs: staleAgeMs, isDaemon: isDaemon)
                Spacer()
                Text(rightTopText(nowMs: nowMs, staleAgeMs: staleAgeMs, isDaemon: isDaemon, disconnected: disconnected))
                    .font(.system(size: 12, weight: failed ? .heavy : .semibold, design: failed ? .monospaced : .default))
                    .foregroundStyle(failed ? Color.primary.opacity(0.92) : Color.secondary.opacity(0.92))
            }

            HStack(alignment: .firstTextBaseline) {
                Text(target.sourcePath)
                    .font(.system(size: 10.8, weight: .semibold))
                    .foregroundStyle(Color.secondary.opacity(0.95))
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer()
                Text(rightBottomText(nowMs: nowMs, staleAgeMs: staleAgeMs, isDaemon: isDaemon, showStale: showStale, failed: failed))
                    .font(rightBottomFont(showStale: showStale, failed: failed))
                    .foregroundStyle(Color.secondary.opacity(0.92))
            }
        }
    }

    private func rightBottomFont(showStale: Bool, failed: Bool) -> Font {
        if failed {
            return .system(size: 12, weight: .semibold)
        }
        if showStale {
            return .system(size: 11.5, weight: .heavy, design: .monospaced)
        }
        return .system(size: 11.5, weight: .heavy, design: .monospaced)
    }

    private var progressBar: some View {
        let bg = RoundedRectangle(cornerRadius: 3, style: .continuous)
        return ZStack(alignment: .leading) {
            bg.fill(Color.black.opacity(0.10))
            if let frac = progressFraction() {
                bg.fill(Color.blue.opacity(0.92))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .scaleEffect(x: max(0.02, CGFloat(min(1.0, frac))), y: 1, anchor: .leading)
            } else {
                bg.fill(Color.blue.opacity(0.55))
                    .frame(width: 56)
            }
        }
        .frame(height: 6)
    }

    private func rightTopText(nowMs: Int64, staleAgeMs: Int64, isDaemon: Bool, disconnected: Bool) -> String {
        if disconnected { return "—" }
        if target.state == "failed" || target.lastRun?.status == "failed" {
            return target.lastRun?.errorCode ?? "failed"
        }
        if !isDaemon || staleAgeMs > StatusFreshness.staleMs {
            return "updated \(formatUpdatedAge(staleAgeMs)) ago"
        }
        if let finishedAt = target.lastRun?.finishedAt, let date = parseIso(finishedAt) {
            let age = Int(nowMs / 1000) - Int(date.timeIntervalSince1970)
            return "Last \(formatRelativeSeconds(age))"
        }
        return "Last —"
    }

    private func rightBottomText(nowMs: Int64, staleAgeMs: Int64, isDaemon: Bool, showStale: Bool, failed: Bool) -> String {
        if failed {
            if let finishedAt = target.lastRun?.finishedAt, let date = parseIso(finishedAt) {
                let age = Int(nowMs / 1000) - Int(date.timeIntervalSince1970)
                return "Last \(formatRelativeSeconds(age))"
            }
            return "Last —"
        }
        if showStale { return "—" }

        let dur = target.lastRun?.durationSeconds ?? 0
        if let bytes = target.lastRun?.bytesUploaded, bytes > 0, dur > 0 {
            return "+\(formatBytes(bytes)) • \(formatElapsed(seconds: Int(dur)))"
        }

        let mutParts: [String] = {
            var parts: [String] = []
            if let files = target.lastRun?.filesIndexed, files > 0 {
                parts.append("\(files) files")
            }
            if let deduped = target.lastRun?.bytesDeduped, deduped > 0 {
                parts.append("saved \(formatBytes(deduped))")
            }
            if dur > 0 {
                parts.append(formatElapsed(seconds: Int(dur)))
            }
            return parts
        }()
        if !mutParts.isEmpty {
            return mutParts.joined(separator: " • ")
        }

        return "—"
    }

    private func runningSummary(nowMs: Int64) -> String {
        let uploaded = target.progress?.bytesUploaded ?? 0
        let elapsed = elapsedText(nowMs: nowMs)
        return "Uploaded \(formatBytes(uploaded)) • Elapsed \(elapsed)"
    }

    private func progressFraction() -> Double? {
        if let p = target.progress {
            if let done = p.chunksDone, let total = p.chunksTotal, total > 0 {
                return min(1.0, Double(done) / Double(total))
            }
            if let done = p.filesDone, let total = p.filesTotal, total > 0 {
                return min(1.0, Double(done) / Double(total))
            }
        }
        return nil
    }

    private func elapsedText(nowMs: Int64) -> String {
        guard let since = target.runningSince else { return "—" }
        let seconds = max(0, (nowMs - since)) / 1000
        return formatElapsed(seconds: Int(seconds))
    }

    private func lastRunText(nowMs: Int64) -> String {
        guard let finishedAt = target.lastRun?.finishedAt,
              let date = parseIso(finishedAt)
        else { return "Last run: —" }
        let age = Int(nowMs / 1000) - Int(date.timeIntervalSince1970)
        let rel = formatRelativeSeconds(age)
        let dur = target.lastRun?.durationSeconds ?? 0
        let durText = dur > 0 ? formatElapsed(seconds: Int(dur)) : "—"
        return "Last run: \(rel) • \(durText)"
    }

    private func parseIso(_ s: String) -> Date? {
        let f1 = ISO8601DateFormatter()
        f1.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = f1.date(from: s) { return d }
        let f2 = ISO8601DateFormatter()
        f2.formatOptions = [.withInternetDateTime]
        return f2.date(from: s)
    }

    private func formatRelativeSeconds(_ s: Int) -> String {
        if s < 5 { return "just now" }
        if s < 60 { return "\(s)s ago" }
        if s < 3600 { return "\(s / 60)m ago" }
        if s < 86400 { return "\(s / 3600)h ago" }
        return "\(s / 86400)d ago"
    }

    private func formatElapsed(seconds: Int) -> String {
        if seconds < 60 { return String(format: "00:%02d", seconds) }
        if seconds < 3600 {
            let m = seconds / 60
            let s = seconds % 60
            return String(format: "%02d:%02d", m, s)
        }
        let h = seconds / 3600
        let m = (seconds % 3600) / 60
        return String(format: "%02d:%02d", h, m)
    }

    private func formatUpdatedAge(_ ms: Int64) -> String {
        if ms < 1_000 { return "\(ms)ms" }
        if ms < 60_000 { return "\(ms / 1000)s" }
        if ms < 3_600_000 { return "\(ms / 60_000)m" }
        return "\(ms / 3_600_000)h"
    }

}

struct DeveloperWindowRootView: View {
    @EnvironmentObject var model: AppModel
    @State private var selectedTargetId: String? = nil
    @State private var activityFilter: String = ""

    var body: some View {
        ZStack {
            VisualEffectView(material: .underWindowBackground, blendingMode: .behindWindow, state: .active)
                .ignoresSafeArea()

            if let snap = model.statusSnapshot {
                HStack(spacing: 0) {
                    sidebar(snapshot: snap)
                        .frame(width: 280)
                    Divider().opacity(0.2)
                    mainPanel(snapshot: snap)
                }
            } else {
                GlassCard(title: "STATUS") {
                    Text("Waiting for status snapshots…")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(.secondary)
                }
                .padding(18)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            }
        }
        .toolbar {
            ToolbarItemGroup(placement: .automatic) {
                Button {
                    model.copyStatusSnapshotJsonToClipboard()
                } label: {
                    Label("Copy JSON", systemImage: "doc.on.doc")
                }
                Button {
                    model.revealStatusSourceInFinder()
                } label: {
                    Label("Reveal…", systemImage: "folder")
                }
                Button {
                    model.statusStreamFrozen.toggle()
                } label: {
                    Label("Freeze", systemImage: model.statusStreamFrozen ? "pause.fill" : "play.fill")
                }
            }
        }
    }

    private func sidebar(snapshot snap: StatusSnapshot) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("TARGETS")
                .font(.system(size: 11, weight: .heavy))
                .foregroundStyle(.secondary)
                .tracking(0.8)
                .padding(.top, 14)
                .padding(.horizontal, 14)

            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(snap.targets) { t in
                        Button {
                            selectedTargetId = t.targetId
                        } label: {
                            sidebarRow(target: t, selected: effectiveSelectedTargetId(snapshot: snap) == t.targetId)
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 14)
                .padding(.bottom, 12)
            }

            Divider().opacity(0.2)
                .padding(.horizontal, 14)

            VStack(alignment: .leading, spacing: 4) {
                Text("Last snapshot: \(updatedAgeText(snapshot: snap))")
                    .font(.system(size: 11, weight: .semibold, design: .monospaced))
                    .foregroundStyle(.secondary)
                Text("Source: \(snap.source.kind)\(snap.source.detail.map { " (\($0))" } ?? "")")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 14)
            .padding(.bottom, 14)
        }
        .frame(maxHeight: .infinity, alignment: .top)
    }

    private func sidebarRow(target t: StatusTarget, selected: Bool) -> some View {
        HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(displayLabel(t))
                    .font(.system(size: 12, weight: .heavy))
                    .foregroundStyle(.primary)
                Text(t.targetId)
                    .font(.system(size: 11, weight: .semibold, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer()
            VStack(alignment: .trailing, spacing: 2) {
                Text(t.state)
                    .font(.system(size: 11, weight: .heavy, design: .monospaced))
                    .foregroundStyle(stateColor(t))
                Text("↑ \(t.up.bytesPerSecond.map { "\(formatBytes($0))/s" } ?? "—")")
                    .font(.system(size: 11, weight: .semibold, design: .monospaced))
                    .foregroundStyle(.secondary)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(selected ? Color.blue.opacity(0.10) : Color.white.opacity(0.12), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(Color.white.opacity(0.16), lineWidth: 1)
        )
    }

    private func mainPanel(snapshot snap: StatusSnapshot) -> some View {
        let selectedId = effectiveSelectedTargetId(snapshot: snap)
        let target = snap.targets.first(where: { $0.targetId == selectedId }) ?? snap.targets.first

        return ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if let target {
                    HStack(alignment: .firstTextBaseline, spacing: 0) {
                        Text(displayLabel(target))
                            .font(.system(size: 18, weight: .heavy))
                        Text(target.sourcePath)
                            .font(.system(size: 12, weight: .semibold, design: .monospaced))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .padding(.leading, 8)
                        Spacer()
                    }
                    .padding(.top, 14)
                    .padding(.horizontal, 16)
                }

                GlassCard(title: "GLOBAL") {
                    let up = snap.global.up.bytesPerSecond.map { "\(formatBytes($0))/s" } ?? "—"
                    let down = snap.global.down.bytesPerSecond.map { "\(formatBytes($0))/s" } ?? "—"
                    let upTot = snap.global.upTotal.bytes.map { formatBytes($0) } ?? "—"
                    let downTot = snap.global.downTotal.bytes.map { formatBytes($0) } ?? "—"

                    keyValue("schemaVersion", "\(snap.schemaVersion)")
                    keyValue("generatedAt", isoFromMs(snap.generatedAt))
                    keyValue("rates", "up=\(up) down=\(down)")
                    keyValue("session totals", "up=\(upTot) down=\(downTot)")
                }
                .padding(.horizontal, 16)

                if let target {
                    GlassCard(title: "TARGET DETAILS") {
                        keyValue("state", target.state)
                        keyValue("phase", target.progress?.phase ?? "—")
                        if let p = target.progress {
                            keyValue("progress", "chunks \(p.chunksDone ?? -1)/\(p.chunksTotal ?? -1)  files \(p.filesDone ?? -1)/\(p.filesTotal ?? -1)")
                            keyValue("bytesRead", p.bytesRead.map(String.init) ?? "—")
                            keyValue("bytesUploaded", p.bytesUploaded.map(String.init) ?? "—")
                            keyValue("bytesDeduped", p.bytesDeduped.map(String.init) ?? "—")
                        } else {
                            keyValue("progress", "—")
                        }
                        if let r = target.lastRun {
                            keyValue(
                                "lastRun",
                                "\(r.status ?? "—")  code=\(r.errorCode ?? "—")  dur=\(r.durationSeconds.map { String(format: "%.2fs", $0) } ?? "—")  files=\(r.filesIndexed.map(String.init) ?? "—")  uploaded=\(r.bytesUploaded.map(String.init) ?? "—")  deduped=\(r.bytesDeduped.map(String.init) ?? "—")"
                            )
                        } else {
                            keyValue("lastRun", "—")
                        }
                    }
                    .padding(.horizontal, 16)
                }

                GlassCard(title: "ACTIVITY") {
                    HStack {
                        Spacer()
                        TextField("filter: target", text: $activityFilter)
                            .textFieldStyle(.roundedBorder)
                            .font(.system(size: 12, weight: .semibold, design: .monospaced))
                            .frame(width: 220)
                    }
                    Divider().opacity(0.25)
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(filteredActivityItems(selectedTargetId: selectedId)) { item in
                            Text("\(isoFromDate(item.at))  \(item.text)")
                                .font(.system(size: 11, weight: .semibold, design: .monospaced))
                                .foregroundStyle(.primary.opacity(0.9))
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 16)
            }
        }
    }

    private func filteredActivityItems(selectedTargetId: String?) -> [AppModel.StatusActivityItem] {
        let all = Array(model.statusActivity.suffix(200).reversed())
        let needle = activityFilter.trimmingCharacters(in: .whitespacesAndNewlines)
        if !needle.isEmpty {
            return all.filter { $0.text.localizedCaseInsensitiveContains(needle) }
        }
        if let selectedTargetId {
            return all.filter { $0.text.contains(selectedTargetId) }
        }
        return all
    }

    private func effectiveSelectedTargetId(snapshot snap: StatusSnapshot) -> String? {
        if let selectedTargetId, snap.targets.contains(where: { $0.targetId == selectedTargetId }) {
            return selectedTargetId
        }
        return snap.targets.first?.targetId
    }

    private func updatedAgeText(snapshot snap: StatusSnapshot) -> String {
        let nowMs = Int64(Date().timeIntervalSince1970 * 1000.0)
        let ageMs = max(0, nowMs - snap.generatedAt)
        if ageMs < 1_000 { return "\(ageMs)ms ago" }
        if ageMs < 60_000 { return "\(ageMs / 1000)s ago" }
        if ageMs < 3_600_000 { return "\(ageMs / 60_000)m ago" }
        return "\(ageMs / 3_600_000)h ago"
    }

    private func stateColor(_ t: StatusTarget) -> Color {
        if t.state == "running" { return .green }
        if t.state == "failed" { return .red }
        if t.state == "stale" { return .orange }
        return .secondary
    }

    private func displayLabel(_ t: StatusTarget) -> String {
        if let label = t.label, !label.isEmpty { return label }
        return t.targetId
    }

    private func keyValue(_ key: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(key)
                .font(.system(size: 11, weight: .semibold, design: .monospaced))
                .foregroundStyle(.secondary)
            Spacer()
            Text(value)
                .font(.system(size: 11, weight: .semibold, design: .monospaced))
                .foregroundStyle(.primary)
                .multilineTextAlignment(.trailing)
        }
    }

    private func isoFromMs(_ ms: Int64) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(ms) / 1000.0)
        return isoFromDate(date)
    }

    private func isoFromDate(_ date: Date) -> String {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f.string(from: date)
    }
}

struct SettingsView: View {
    @EnvironmentObject var model: AppModel
    @FocusState private var tokenFocused: Bool
    @FocusState private var apiHashFocused: Bool

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                GlassCard(title: "TELEGRAM") {
                    HStack {
                        Text("Secrets")
                            .font(.system(size: 13, weight: .semibold))
                        Spacer()
                        Text(model.secretPresenceFetchInFlight ? "Checking…" : (model.secretPresenceKnown ? "Checked" : "Not checked"))
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.secondary)
                        Button("Check") { model.refreshSecretsPresence(force: true) }
                            .buttonStyle(.bordered)
                            .disabled(model.secretPresenceFetchInFlight)
                    }
                    Divider().opacity(0.4)

                    HStack {
                        Text("Bot Token")
                            .font(.system(size: 13, weight: .semibold))
                        Spacer()
                        Text(model.secretPresenceKnown ? (model.botTokenPresent ? "Saved (encrypted)" : "Not set") : "Not checked")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(model.secretPresenceKnown && model.botTokenPresent ? Color.green : Color.secondary)
                    }
                    Divider().opacity(0.4)

                    HStack {
                        Text("Chat ID")
                            .font(.system(size: 13, weight: .semibold))
                        Spacer()
                        TextField("-100123…", text: $model.chatId)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 170)
                    }

                    HStack {
                        SecureField("Paste new bot token", text: $model.botTokenDraft)
                            .focused($tokenFocused)
                        Button("Save token") { model.setBotToken() }
                            .buttonStyle(.bordered)
                    }

                    Divider().opacity(0.4)

                    HStack {
                        Text("API ID")
                            .font(.system(size: 13, weight: .semibold))
                        Spacer()
                        TextField("123456", text: $model.mtprotoApiId)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 170)
                    }

                    HStack {
                        Text("API hash")
                            .font(.system(size: 13, weight: .semibold))
                        Spacer()
                        Text(model.secretPresenceKnown ? (model.mtprotoApiHashPresent ? "Saved (encrypted)" : "Not set") : "Not checked")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(model.secretPresenceKnown && model.mtprotoApiHashPresent ? Color.green : Color.secondary)
                    }

                    HStack(spacing: 8) {
                        SecureField("Paste api_hash", text: $model.mtprotoApiHashDraft)
                            .focused($apiHashFocused)
                        Button("Save api_hash") { model.setMtprotoApiHash() }
                            .buttonStyle(.bordered)
                    }

                    HStack {
                        Button("Clear session") { model.clearMtprotoSession() }
                            .buttonStyle(.bordered)
                        Spacer()
                        Text(model.secretPresenceKnown ? (model.mtprotoSessionPresent ? "Session: saved" : "Session: none") : "Session: not checked")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.secondary)
                    }

                    HStack {
                        Button("Test connection") { model.testConnection() }
                            .buttonStyle(.bordered)
                        Spacer()
                    }

                    HStack {
                        Button("Migrate Keychain") { model.migrateKeychainSecrets() }
                            .buttonStyle(.bordered)
                        Button("Ensure master key") { model.initMasterKey() }
                            .buttonStyle(.bordered)
                        Spacer()
                        Text(model.secretPresenceKnown ? (model.masterKeyPresent ? "Master key: ready" : "Master key: missing") : "Master key: not checked")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(model.secretPresenceKnown ? (model.masterKeyPresent ? Color.secondary : Color.red) : Color.secondary)
                    }
                }

                GlassCard(title: "SCHEDULE") {
                    Toggle("Enable", isOn: $model.scheduleEnabled)
                    HStack {
                        Text("Frequency")
                        Spacer()
                        Picker("", selection: $model.scheduleKind) {
                            Text("Hourly").tag("hourly")
                            Text("Daily").tag("daily")
                        }
                        .pickerStyle(.menu)
                    }
                }

                HStack(spacing: 10) {
                    Button("Open logs") { model.openLogs() }
                        .buttonStyle(.bordered)
                    Spacer()
                    Button("Save") { model.saveSettings() }
                        .buttonStyle(.borderedProminent)
                        .tint(.blue)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .onChange(of: tokenFocused) { _, isFocused in
            if isFocused, model.botTokenDraftIsMasked {
                model.botTokenDraft = ""
                model.botTokenDraftIsMasked = false
            }
        }
        .onChange(of: apiHashFocused) { _, isFocused in
            if isFocused, model.mtprotoApiHashDraftIsMasked {
                model.mtprotoApiHashDraft = ""
                model.mtprotoApiHashDraftIsMasked = false
            }
        }
    }
}

struct ToastPill: View {
    let text: String
    let isError: Bool

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(isError ? Color.red : Color.green)
                .frame(width: 8, height: 8)
                .opacity(0.95)
            Text(text)
                .font(.system(size: 12, weight: .heavy))
                .foregroundStyle(Color.primary)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(.regularMaterial, in: Capsule())
        .overlay(Capsule().strokeBorder(Color.white.opacity(0.28), lineWidth: 1))
        .shadow(color: Color.black.opacity(0.14), radius: 10, x: 0, y: 4)
    }
}

private enum SettingsTitlebarAccessory {
    static let identifier = NSUserInterfaceItemIdentifier("televybackup.settings.title")
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    private let popover = NSPopover()
    private var statusItem: NSStatusItem?
    private var cancellables: Set<AnyCancellable> = []

    func applicationDidFinishLaunching(_ notification: Notification) {
        let status = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        if let button = status.button {
            button.image = NSImage(
                systemSymbolName: "externaldrive",
                accessibilityDescription: "TelevyBackup"
            )
            button.action = #selector(togglePopover(_:))
            button.target = self
        }
        statusItem = status

        popover.behavior = .transient
        popover.animates = true
        popover.contentSize = NSSize(width: 360, height: 460)
        popover.appearance = NSAppearance(named: .vibrantLight)
        let host = NSHostingController(rootView: PopoverRootView().environmentObject(ModelStore.shared))
        host.view.wantsLayer = true
        host.view.layer?.backgroundColor = NSColor.clear.cgColor
        popover.contentViewController = host

        // Best-effort: keep daemon running even if the popover is not opened yet.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.10) {
            ModelStore.shared.ensureDaemonRunning()
            ModelStore.shared.ensureStatusStreamRunning()
        }

        ModelStore.shared.$popoverDesiredHeight
            .receive(on: DispatchQueue.main)
            .sink { [weak self] h in
                guard let self else { return }
                let height = min(max(320, h), 720)
                if self.popover.contentSize.height != height {
                    self.popover.contentSize = NSSize(width: 360, height: height)
                }
            }
            .store(in: &cancellables)

        if ProcessInfo.processInfo.environment["TELEVYBACKUP_SHOW_POPOVER_ON_LAUNCH"] != "0" {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) {
                self.showPopover(nil)
            }
        }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        showPopover(nil)
        return true
    }

    @objc private func togglePopover(_ sender: Any?) {
        if popover.isShown {
            closePopover(sender)
        } else {
            showPopover(sender)
        }
    }

    private func showPopover(_ sender: Any?) {
        guard let button = statusItem?.button else { return }
        ModelStore.shared.ensureDaemonRunning()
        ModelStore.shared.ensureStatusStreamRunning()
        ModelStore.shared.refresh()
        NSApp.activate(ignoringOtherApps: true)
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        configurePopoverWindowIfNeeded()
    }

    private func closePopover(_ sender: Any?) {
        popover.performClose(sender)
    }

    private func configurePopoverWindowIfNeeded() {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.01) {
            guard let window = self.popover.contentViewController?.view.window else { return }
            if window.isOpaque {
                window.isOpaque = false
            }
            if window.backgroundColor != .clear {
                window.backgroundColor = .clear
            }
        }
    }
}

@main
struct TelevyBackupApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @StateObject private var model = ModelStore.shared

    var body: some Scene {
        Settings {
            EmptyView()
        }
        .commands {
            CommandGroup(replacing: .appSettings) {
                Button("Settings…") { model.openSettingsWindow() }
                    .keyboardShortcut(",", modifiers: .command)
            }
        }
    }
}
