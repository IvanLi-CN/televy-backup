import AppKit
import SwiftUI

final class AppModel: ObservableObject {
    @Published var sourcePath: String = ""
    @Published var label: String = "manual"
    @Published var snapshotId: String = ""
    @Published var restoreTargetPath: String = ""
    @Published var chatId: String = ""
    @Published var botToken: String = ""
    @Published var logs: String = ""
    @Published var status: String = "idle"

    func appendLog(_ line: String) {
        DispatchQueue.main.async {
            if !self.logs.isEmpty { self.logs += "\n" }
            self.logs += line
        }
    }

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
            let out = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
            return out.isEmpty ? nil : out
        } catch {
            return nil
        }
    }

    func writeConfigToml() throws {
        let dir = configTomlPath().deletingLastPathComponent()
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)

        let toml = """
        sources = [\(tomlStringArray([sourcePath].filter { !$0.isEmpty }))]

        [schedule]
        enabled = false
        kind = "hourly"
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

    func setBotToken() {
        guard let cli = cliPath() else {
            appendLog("ERROR: televybackup not found (set TELEVYBACKUP_CLI_PATH or install it)")
            return
        }
        guard !botToken.isEmpty else {
            appendLog("ERROR: bot token is empty")
            return
        }

        runProcess(exe: cli, args: ["secrets", "set-telegram-bot-token", "--json"], stdin: botToken + "\n")
    }

    func initMasterKey() {
        guard let cli = cliPath() else {
            appendLog("ERROR: televybackup not found (set TELEVYBACKUP_CLI_PATH or install it)")
            return
        }
        runProcess(exe: cli, args: ["secrets", "init-master-key", "--json"])
    }

    func validateTelegram() {
        guard let cli = cliPath() else {
            appendLog("ERROR: televybackup not found (set TELEVYBACKUP_CLI_PATH or install it)")
            return
        }
        runProcess(exe: cli, args: ["telegram", "validate", "--json"])
    }

    func runBackup() {
        guard let cli = cliPath() else {
            appendLog("ERROR: televybackup not found (set TELEVYBACKUP_CLI_PATH or install it)")
            return
        }
        guard !sourcePath.isEmpty else {
            appendLog("ERROR: source path is empty")
            return
        }
        runProcess(exe: cli, args: ["backup", "run", "--source", sourcePath, "--label", label, "--events"])
    }

    func listSnapshots() {
        guard let cli = cliPath() else {
            appendLog("ERROR: televybackup not found (set TELEVYBACKUP_CLI_PATH or install it)")
            return
        }
        runProcess(exe: cli, args: ["snapshots", "list", "--json"])
    }

    func getStats() {
        guard let cli = cliPath() else {
            appendLog("ERROR: televybackup not found (set TELEVYBACKUP_CLI_PATH or install it)")
            return
        }
        runProcess(exe: cli, args: ["stats", "get", "--json"])
    }

    func runRestore() {
        guard let cli = cliPath() else {
            appendLog("ERROR: televybackup not found (set TELEVYBACKUP_CLI_PATH or install it)")
            return
        }
        guard !snapshotId.isEmpty else {
            appendLog("ERROR: snapshot id is empty")
            return
        }
        guard !restoreTargetPath.isEmpty else {
            appendLog("ERROR: restore target path is empty")
            return
        }
        runProcess(exe: cli, args: ["restore", "run", "--snapshot-id", snapshotId, "--target", restoreTargetPath, "--events"])
    }

    func runVerify() {
        guard let cli = cliPath() else {
            appendLog("ERROR: televybackup not found (set TELEVYBACKUP_CLI_PATH or install it)")
            return
        }
        guard !snapshotId.isEmpty else {
            appendLog("ERROR: snapshot id is empty")
            return
        }
        runProcess(exe: cli, args: ["verify", "run", "--snapshot-id", snapshotId, "--events"])
    }

    private func runProcess(exe: String, args: [String], stdin: String? = nil) {
        status = "running"
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

        out.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            if data.isEmpty { return }
            self.appendLog(String(decoding: data, as: UTF8.self).trimmingCharacters(in: .newlines))
        }
        err.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            if data.isEmpty { return }
            self.appendLog(String(decoding: data, as: UTF8.self).trimmingCharacters(in: .newlines))
        }

        DispatchQueue.global(qos: .userInitiated).async {
            do {
                try task.run()
                task.waitUntilExit()
            } catch {
                self.appendLog("ERROR: failed to run process: \(error)")
            }
            DispatchQueue.main.async {
                self.status = "idle"
                out.fileHandleForReading.readabilityHandler = nil
                err.fileHandleForReading.readabilityHandler = nil
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

struct ContentView: View {
    @EnvironmentObject var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            GroupBox("Settings (write config.toml)") {
                VStack(alignment: .leading) {
                    TextField("Source path", text: $model.sourcePath)
                    TextField("Telegram chat_id", text: $model.chatId)
                    SecureField("Telegram bot token", text: $model.botToken)
                    HStack {
                        Button("Save config") {
                            do {
                                try model.writeConfigToml()
                                model.appendLog("Saved: \(model.configTomlPath().path)")
                            } catch {
                                model.appendLog("ERROR: save config failed: \(error)")
                            }
                        }
                        Button("Set bot token") { model.setBotToken() }
                        Button("Init master key") { model.initMasterKey() }
                        Button("Validate") { model.validateTelegram() }
                    }
                }
            }

            GroupBox("Actions") {
                VStack(alignment: .leading) {
                    TextField("Label", text: $model.label)
                    HStack {
                        Button("List snapshots") { model.listSnapshots() }
                        Button("Stats") { model.getStats() }
                        Button("Run backup") { model.runBackup() }
                    }

                    Divider()

                    TextField("Snapshot id", text: $model.snapshotId)
                    TextField("Restore target path (empty dir)", text: $model.restoreTargetPath)
                    HStack {
                        Button("Restore") { model.runRestore() }
                        Button("Verify") { model.runVerify() }
                    }
                }
            }

            GroupBox("Logs") {
                ScrollView {
                    Text(model.logs)
                        .font(.system(.caption, design: .monospaced))
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(width: 420, height: 240)
            }

            Text("Status: \(model.status)")
                .font(.footnote)
        }
        .padding(12)
    }
}

@main
struct TelevyBackupApp: App {
    @StateObject private var model = AppModel()

    var body: some Scene {
        MenuBarExtra("TelevyBackup", systemImage: "externaldrive") {
            ContentView().environmentObject(model)
        }
        Settings {
            ContentView().environmentObject(model)
        }
    }
}
