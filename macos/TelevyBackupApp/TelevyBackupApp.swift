import AppKit
import Darwin
import SwiftUI

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

enum Tab: String, CaseIterable, Identifiable {
    case overview = "Overview"
    case logs = "Logs"
    case settings = "Settings"

    var id: String { rawValue }
}

struct LogEntry: Identifiable {
    let id = UUID()
    let timestamp: Date
    let message: String
}

final class ModelStore {
    static let shared = AppModel()
}

final class AppModel: ObservableObject {
    @Published var tab: Tab = .overview

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

    @Published var logEntries: [LogEntry] = []

    private let fileLogQueue = DispatchQueue(label: "TelevyBackup.uiLog", qos: .utility)
    private var didWriteStartupLog: Bool = false

    func defaultConfigDir() -> URL {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return home
            .appendingPathComponent("Library")
            .appendingPathComponent("Application Support")
            .appendingPathComponent("TelevyBackup")
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

    func refresh() {
        if !didWriteStartupLog {
            didWriteStartupLog = true
            appendLog("UI started")
        }
        DispatchQueue.global(qos: .utility).async {
            self.refreshSettings(withSecrets: false)
        }
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
                    self.showToast("Failed to save token (see Logs)", isError: true)
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
                    self.showToast("Migration failed (see Logs)", isError: true)
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
                            self.showToast("Failed to init master key (see Logs)", isError: true)
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
                    self.showToast("Migration failed (see Logs)", isError: true)
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
                    self.showToast("Failed to save api_hash (see Logs)", isError: true)
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
                    self.showToast("Failed to clear session (see Logs)", isError: true)
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
        do {
            try writeConfigToml()
            appendLog("Saved: \(configTomlPath().path)")
        } catch {
            appendLog("ERROR: save config failed: \(error)")
            showToast("Save failed (see Logs)", isError: true)
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
                    self.telegramValidateText = "Failed (see Logs)"
                    self.showToast("Test failed (see Logs)", isError: true)
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
        tab = .logs
    }

    private func writeConfigToml() throws {
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
        try toml.write(to: configTomlPath(), atomically: true, encoding: .utf8)
    }

    private func refreshSettings(withSecrets: Bool) {
        guard let cli = cliPath() else { return }
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
                }
            }
            return
        }

        let settings = obj["settings"] as? [String: Any]
        let sources = (settings?["sources"] as? [String]) ?? []
        let schedule = (settings?["schedule"] as? [String: Any]) ?? [:]
        let telegram = (settings?["telegram"] as? [String: Any]) ?? [:]
        let chatId = (telegram["chat_id"] as? String) ?? ""
        let mtproto = (telegram["mtproto"] as? [String: Any]) ?? [:]
        let apiIdNum = (mtproto["api_id"] as? NSNumber)?.intValue ?? 0

        let scheduleEnabled = (schedule["enabled"] as? Bool) ?? false
        let scheduleKind = (schedule["kind"] as? String) ?? "hourly"

        DispatchQueue.main.async {
            if let first = sources.first {
                self.sourcePath = first
            }
            self.scheduleEnabled = scheduleEnabled
            self.scheduleKind = scheduleKind

            self.chatId = chatId
            self.mtprotoApiId = apiIdNum > 0 ? String(apiIdNum) : ""
            if withSecrets {
                let secrets = obj["secrets"] as? [String: Any]
                let botPresent = (secrets?["telegramBotTokenPresent"] as? Bool) ?? false
                let masterPresent = (secrets?["masterKeyPresent"] as? Bool) ?? false
                let apiHashPresent = (secrets?["telegramMtprotoApiHashPresent"] as? Bool) ?? false
                let sessionPresent = (secrets?["telegramMtprotoSessionPresent"] as? Bool) ?? false
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
        }
    }

    private func appendLog(_ line: String) {
        let trimmed = sanitizeLogLine(line.trimmingCharacters(in: .newlines))
        guard !trimmed.isEmpty else { return }
        appendFileLog(trimmed)
        DispatchQueue.main.async {
            self.logEntries.append(LogEntry(timestamp: Date(), message: trimmed))
            if self.logEntries.count > 400 {
                self.logEntries.removeFirst(self.logEntries.count - 400)
            }
        }
    }

    private func sanitizeLogLine(_ line: String) -> String {
        // Redact any api.telegram.org URL segment to avoid leaking secrets.
        let needle = "api.telegram.org"
        guard let r = line.range(of: needle) else { return line }
        let start = r.lowerBound
        var end = line.endIndex
        if let ws = line[start...].firstIndex(where: { $0.isWhitespace }) {
            end = ws
        }
        return line.replacingCharacters(in: start..<end, with: "[redacted_url]")
    }

    private func uiLogFileURL() -> URL {
        defaultConfigDir().appendingPathComponent("ui.log")
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
                // Do not recurse into appendLog; ignore file logging failures.
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

    private func runCommandCapture(
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
        appendLog(line)

        guard let data = line.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return }

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
    let ok: Bool

    var body: some View {
        ZStack {
            Circle()
                .fill(ok ? Color.green : Color.red)
                .opacity(0.95)
                .frame(width: 10, height: 10)
            Circle()
                .fill(ok ? Color.green : Color.red)
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

struct SegmentedTabs: View {
    @Binding var tab: Tab

    var body: some View {
        HStack(spacing: 0) {
            tabButton(.overview)
            divider
            tabButton(.logs)
            divider
            tabButton(.settings)
        }
        .padding(1)
        .background(Color.white.opacity(0.16), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(Color.white.opacity(0.18), lineWidth: 1)
        )
        .frame(height: 32)
    }

    private var divider: some View {
        Rectangle()
            .fill(Color.black.opacity(0.10))
            .frame(width: 1)
            .padding(.vertical, 3)
    }

    private func tabButton(_ t: Tab) -> some View {
        Button {
            tab = t
        } label: {
            Text(t.rawValue)
                .font(.system(size: 12, weight: .bold))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .foregroundStyle(tab == t ? Color.primary : Color.secondary)
        }
        .buttonStyle(.plain)
        .background(
            Group {
                if tab == t {
                    RoundedRectangle(cornerRadius: 9, style: .continuous)
                        .fill(Color.white.opacity(0.30))
                        .padding(1)
                }
            }
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
                SegmentedTabs(tab: $model.tab)

                switch model.tab {
                case .overview:
                    OverviewView()
                case .logs:
                    LogsView()
                case .settings:
                    SettingsView()
                }
            }
            .padding(12)
        }
        .frame(width: 360, height: 460)
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
        HStack(alignment: .center, spacing: 10) {
            ZStack {
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .fill(Color.white.opacity(0.25))
                    .frame(width: 28, height: 28)
                    .overlay(
                        RoundedRectangle(cornerRadius: 9, style: .continuous)
                            .strokeBorder(Color.white.opacity(0.22), lineWidth: 1)
                    )
                Circle()
                    .fill(Color.blue)
                    .frame(width: 12, height: 12)
            }

            VStack(alignment: .leading, spacing: 2) {
                Text("TelevyBackup")
                    .font(.system(size: 15, weight: .bold))
                Text(model.tab == .settings ? "Settings" : model.telegramStatusText)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.secondary)
            }

            Spacer()

            StatusLED(ok: model.telegramOk)

            Button {
                model.refresh()
            } label: {
                Image(systemName: "ellipsis")
                    .font(.system(size: 13, weight: .semibold))
                    .frame(width: 22, height: 22)
                    .background(Color.white.opacity(0.22), in: RoundedRectangle(cornerRadius: 10))
            }
            .buttonStyle(.plain)
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
        VStack(alignment: .leading, spacing: 12) {
            GlassCard(title: "STATUS") {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(statusTitle())
                        .font(.system(size: 16, weight: .heavy))
                    Text(statusSubtitle())
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.secondary)
                }

                ProgressView()
                    .progressViewStyle(.linear)
                    .opacity(model.isRunning ? 1 : 0)

                HStack(spacing: 26) {
                    statColumn("Uploaded", formatBytes(displayBytesUploaded()), .blue)
                    statColumn("Dedupe", formatBytes(displayBytesDeduped()), .green)
                    statColumn("Duration", formatDuration(displayDurationSeconds()), .primary)
                }
            }

            GlassCard(title: "DETAILS") {
                HStack(alignment: .firstTextBaseline) {
                    Text("Source")
                        .font(.system(size: 13, weight: .semibold))
                    Spacer()
                    Text(model.sourcePath.isEmpty ? "—" : model.sourcePath)
                        .font(.system(size: 13, weight: .semibold, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Button("Choose…") { model.chooseSourceFolder() }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                }
                Divider().opacity(0.35)
                detailRow(label: "Next schedule", value: nextScheduleText())
                Divider().opacity(0.35)
                detailRow(label: "Index", value: "Encrypted • Synced")
            }

            Button {
                model.runBackupNow()
            } label: {
                Text("Run backup now")
                    .font(.system(size: 13, weight: .heavy))
                    .frame(maxWidth: .infinity, minHeight: 34)
            }
            .buttonStyle(.borderedProminent)
            .tint(.blue)
        }
    }

    private func statColumn(_ label: String, _ value: String, _ color: Color) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label)
                .font(.system(size: 12, weight: .bold))
                .foregroundStyle(.primary.opacity(0.85))
            Text(value)
                .font(.system(.body, design: .monospaced).weight(.bold))
                .foregroundStyle(color)
        }
    }

    private func displayBytesUploaded() -> Int64 {
        model.isRunning ? model.currentBytesUploaded : model.lastBytesUploaded
    }

    private func displayBytesDeduped() -> Int64 {
        model.isRunning ? model.currentBytesDeduped : model.lastBytesDeduped
    }

    private func displayDurationSeconds() -> Double {
        if model.isRunning, let startedAt = model.taskStartedAt {
            return Date().timeIntervalSince(startedAt)
        }
        return model.lastDurationSeconds
    }

    private func detailRow(label: String, value: String) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(label)
                .font(.system(size: 13, weight: .semibold))
            Spacer()
            Text(value)
                .font(.system(size: 13, weight: .semibold, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
        }
    }

    private func nextScheduleText() -> String {
        if !model.scheduleEnabled { return "—" }
        return model.scheduleKind == "daily" ? "Daily 02:00" : "Hourly :00"
    }

    private func lastRunText() -> String {
        guard let last = model.lastRunAt else { return "(ready)" }
        let seconds = Date().timeIntervalSince(last)
        if seconds < 60 { return "(last run just now)" }
        if seconds < 3600 { return "(last run \(Int(seconds / 60))m ago)" }
        return "(last run \(Int(seconds / 3600))h ago)"
    }

    private func statusTitle() -> String {
        if model.isRunning { return "Syncing" }
        if model.lastRunOk == false { return "Failed" }
        return "Idle"
    }

    private func statusSubtitle() -> String {
        if model.isRunning { return "(\(model.phase))" }
        if model.lastRunOk == false {
            return "(\(model.lastRunErrorCode ?? "error"))"
        }
        return lastRunText()
    }
}

struct LogsView: View {
    @EnvironmentObject var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            List(model.logEntries.reversed()) { item in
                VStack(alignment: .leading, spacing: 3) {
                    Text(item.message)
                        .font(.system(size: 12, design: .monospaced))
                        .lineLimit(3)
                    Text(item.timestamp.formatted(date: .omitted, time: .standard))
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
                .padding(.vertical, 3)
            }
            .listStyle(.plain)

            HStack {
                Button("Refresh") { model.refresh() }
                    .buttonStyle(.bordered)
                Spacer()
            }
        }
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
                        SecureField("Paste new bot token (not stored here)", text: $model.botTokenDraft)
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
                        SecureField("Paste api_hash (not stored here)", text: $model.mtprotoApiHashDraft)
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

final class AppDelegate: NSObject, NSApplicationDelegate {
    private let popover = NSPopover()
    private var statusItem: NSStatusItem?

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
    }
}
