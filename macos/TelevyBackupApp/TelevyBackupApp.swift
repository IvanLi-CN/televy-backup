import AppKit
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
    @Published var scheduleEnabled: Bool = false
    @Published var scheduleKind: String = "hourly"

    @Published var telegramOk: Bool = false
    @Published var telegramStatusText: String = "Telegram Storage • Offline"
    @Published var botTokenPresent: Bool = false
    @Published var masterKeyPresent: Bool = false
    @Published var telegramValidateOk: Bool? = nil
    @Published var telegramValidateText: String = "Not validated"

    @Published var toastText: String? = nil
    @Published var toastIsError: Bool = false

    @Published var lastCommandText: String = "-"
    @Published var lastExitStatus: Int32? = nil
    @Published var lastErrorText: String? = nil

    @Published var isRunning: Bool = false
    @Published var phase: String = "idle"

    @Published var lastBytesUploaded: Int64 = 0
    @Published var lastBytesDeduped: Int64 = 0
    @Published var lastDurationSeconds: Double = 0
    @Published var lastRunAt: Date?

    @Published var logEntries: [LogEntry] = []

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
        refreshSecrets()
    }

    static func maskedTokenPlaceholder() -> String {
        String(repeating: "•", count: 18)
    }

    func saveSettings() {
        do {
            try writeConfigToml()
            appendLog("Saved: \(configTomlPath().path)")
            refreshSecrets()
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
                    self.showToast("Saved in Keychain", isError: false)
                    self.refreshSecrets()
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
        showToast("Initializing master key…", isError: false)
        runProcess(
            exe: cli,
            args: ["--json", "secrets", "init-master-key"],
            updateTaskState: false,
            onExit: { status in
                if status == 0 {
                    self.showToast("Master key saved in Keychain", isError: false)
                    self.refreshSecrets()
                } else {
                    self.showToast("Failed to init master key (see Logs)", isError: true)
                }
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
            return
        }
        runProcess(exe: cli, args: ["--events", "backup", "run", "--source", sourcePath, "--label", label])
    }

    func openLogs() {
        tab = .logs
    }

    private func writeConfigToml() throws {
        let dir = configTomlPath().deletingLastPathComponent()
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)

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
        mode = "botapi"
        chat_id = \(tomlString(chatId))
        bot_token_key = "telegram.bot_token"

        [telegram.rate_limit]
        max_concurrent_uploads = 2
        min_delay_ms = 250
        """
        try toml.write(to: configTomlPath(), atomically: true, encoding: .utf8)
    }

    private func refreshSecrets() {
        guard let cli = cliPath() else { return }
        let output = runCommandCapture(exe: cli, args: ["--json", "settings", "get"])
        guard let data = output.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            DispatchQueue.main.async {
                self.botTokenPresent = false
                self.masterKeyPresent = false
                self.telegramOk = false
                self.telegramStatusText = "Offline"
            }
            return
        }

        let secrets = obj["secrets"] as? [String: Any]
        let settings = obj["settings"] as? [String: Any]
        let telegram = (settings?["telegram"] as? [String: Any]) ?? [:]
        let chatId = (telegram["chat_id"] as? String) ?? ""

        let botPresent = (secrets?["telegramBotTokenPresent"] as? Bool) ?? false
        let masterPresent = (secrets?["masterKeyPresent"] as? Bool) ?? false

        DispatchQueue.main.async {
            self.botTokenPresent = botPresent
            self.masterKeyPresent = masterPresent
            self.chatId = chatId
            self.telegramOk = botPresent && masterPresent && !chatId.isEmpty
            self.telegramStatusText = self.telegramOk ? "Telegram Storage • Online" : "Telegram Storage • Offline"
            if !botPresent || chatId.isEmpty {
                self.telegramValidateOk = false
                self.telegramValidateText = "Missing token / chat id"
            } else if self.telegramValidateText == "Missing token / chat id" {
                self.telegramValidateOk = nil
                self.telegramValidateText = "Not validated"
            }
            if botPresent && self.botTokenDraft.isEmpty {
                self.botTokenDraft = Self.maskedTokenPlaceholder()
                self.botTokenDraftIsMasked = true
            }
            if !botPresent && self.botTokenDraftIsMasked {
                self.botTokenDraft = ""
                self.botTokenDraftIsMasked = false
            }
        }
    }

    private func appendLog(_ line: String) {
        let trimmed = line.trimmingCharacters(in: .newlines)
        guard !trimmed.isEmpty else { return }
        DispatchQueue.main.async {
            self.logEntries.append(LogEntry(timestamp: Date(), message: trimmed))
            if self.logEntries.count > 400 {
                self.logEntries.removeFirst(self.logEntries.count - 400)
            }
        }
    }

    private func runCommandCapture(exe: String, args: [String], stdin: String? = nil) -> String {
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

        do {
            try task.run()
            task.waitUntilExit()
        } catch {
            return ""
        }

        let data = out.fileHandleForReading.readDataToEndOfFile()
        return String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func handleOutputLine(_ line: String) {
        appendLog(line)

        guard let data = line.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return }

        if let code = obj["code"] as? String, let message = obj["message"] as? String {
            DispatchQueue.main.async {
                self.lastErrorText = "\(code): \(message)"
            }
            return
        }

        guard let type = obj["type"] as? String else { return }

        if type == "task.progress" {
            let phase = obj["phase"] as? String ?? "running"
            DispatchQueue.main.async { self.phase = phase }
            return
        }

        if type == "task.state" {
            let state = obj["state"] as? String ?? ""
            let kind = obj["kind"] as? String ?? ""
            DispatchQueue.main.async {
                if state == "running" {
                    self.isRunning = true
                    self.phase = kind
                } else {
                    self.isRunning = false
                    self.phase = "idle"
                }
            }

            if state == "succeeded",
               let result = obj["result"] as? [String: Any]
            {
                let bytesUploaded = (result["bytesUploaded"] as? NSNumber)?.int64Value ?? 0
                let bytesDeduped = (result["bytesDeduped"] as? NSNumber)?.int64Value ?? 0
                let duration = (result["durationSeconds"] as? NSNumber)?.doubleValue ?? 0
                DispatchQueue.main.async {
                    self.lastBytesUploaded = bytesUploaded
                    self.lastBytesDeduped = bytesDeduped
                    self.lastDurationSeconds = duration
                    self.lastRunAt = Date()
                }
                refreshSecrets()
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
        updateTaskState: Bool = true,
        onExit: ((Int32) -> Void)? = nil
    ) {
        if updateTaskState {
            DispatchQueue.main.async {
                self.isRunning = true
                self.phase = "running"
            }
        }
        let cmdLine = "$ \(exe) \(args.joined(separator: " "))"
        DispatchQueue.main.async {
            self.lastCommandText = cmdLine
            self.lastExitStatus = nil
            self.lastErrorText = nil
        }
        appendLog(cmdLine)

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
                task.waitUntilExit()
                status = task.terminationStatus
            } catch {
                self.handleOutputLine("ERROR: failed to run process: \(error)")
            }
            DispatchQueue.main.async {
                self.lastExitStatus = status
                if updateTaskState {
                    self.isRunning = false
                    self.phase = "idle"
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
        .onAppear { model.refresh() }
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
                    Text(model.isRunning ? "Syncing" : "Idle")
                        .font(.system(size: 16, weight: .heavy))
                    Text(model.isRunning ? "(\(model.phase))" : lastRunText())
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.secondary)
                }

                ProgressView()
                    .progressViewStyle(.linear)
                    .opacity(model.isRunning ? 1 : 0)

                HStack(spacing: 26) {
                    statColumn("Uploaded", formatBytes(model.lastBytesUploaded), .blue)
                    statColumn("Dedupe", formatBytes(model.lastBytesDeduped), .green)
                    statColumn("Duration", formatDuration(model.lastDurationSeconds), .primary)
                }
            }

            GlassCard(title: "DETAILS") {
                detailRow(label: "Source", value: model.sourcePath.isEmpty ? "—" : model.sourcePath)
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
                Button("Copy") { copyLogs() }
                    .buttonStyle(.bordered)
                Spacer()
                Button("Refresh") { model.refresh() }
                    .buttonStyle(.bordered)
            }
        }
    }

    private func copyLogs() {
        let text = model.logEntries
            .map { "\($0.timestamp.formatted(date: .omitted, time: .standard)) \($0.message)" }
            .joined(separator: "\n")
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}

struct SettingsView: View {
    @EnvironmentObject var model: AppModel
    @FocusState private var tokenFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            GlassCard(title: "TELEGRAM") {
                HStack {
                    Text("Bot Token")
                        .font(.system(size: 13, weight: .semibold))
                    Spacer()
                    Text(model.botTokenPresent ? "Saved in Keychain" : "Not set")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(model.botTokenPresent ? Color.green : Color.secondary)
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

                HStack(spacing: 8) {
                    SecureField("Paste new bot token (not stored here)", text: $model.botTokenDraft)
                        .focused($tokenFocused)
                    Button("Save token") { model.setBotToken() }
                        .buttonStyle(.bordered)
                }
                if let toast = model.toastText {
                    Text(toast)
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(model.toastIsError ? Color.red : Color.green)
                        .padding(.top, 2)
                }

                HStack {
                    Button("Test connection") { model.testConnection() }
                        .buttonStyle(.bordered)
                    Spacer()
                    StatusLED(ok: model.telegramValidateOk ?? false)
                    Text(model.telegramValidateText)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.secondary)
                }

                HStack {
                    Button("Init master key") { model.initMasterKey() }
                        .buttonStyle(.bordered)
                    Spacer()
                    Text(model.masterKeyPresent ? "Master key: saved" : "Master key: missing")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(model.masterKeyPresent ? Color.secondary : Color.red)
                }
            }

            GlassCard(title: "DIAGNOSTICS") {
                Text(model.lastCommandText)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(2)

                HStack {
                    Text("Exit")
                        .font(.system(size: 12, weight: .semibold))
                    Spacer()
                    Text(model.lastExitStatus.map(String.init) ?? "-")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(model.lastExitStatus == 0 ? Color.green : Color.secondary)
                }

                if let err = model.lastErrorText {
                    Text(err)
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(Color.red)
                        .lineLimit(3)
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
        .onChange(of: tokenFocused) { _, isFocused in
            if isFocused, model.botTokenDraftIsMasked {
                model.botTokenDraft = ""
                model.botTokenDraftIsMasked = false
            }
        }
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
