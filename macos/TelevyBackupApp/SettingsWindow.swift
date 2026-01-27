import AppKit
import SwiftUI
import UniformTypeIdentifiers

struct CliSettingsGetResponse: Decodable {
    let settings: SettingsV2
    let secrets: CliSecretsPresence?
}

struct CliSecretsPresence: Decodable {
    let masterKeyPresent: Bool?
    let telegramMtprotoApiHashPresent: Bool?
    let telegramBotTokenPresentByEndpoint: [String: Bool]?
    let telegramMtprotoSessionPresentByEndpoint: [String: Bool]?

    enum CodingKeys: String, CodingKey {
        case masterKeyPresent
        case telegramMtprotoApiHashPresent
        case telegramBotTokenPresentByEndpoint
        case telegramMtprotoSessionPresentByEndpoint
    }
}

struct SettingsV2: Codable {
    var version: Int
    var schedule: ScheduleV2
    var retention: RetentionV2
    var chunking: ChunkingV2
    var telegram: TelegramGlobalV2
    var telegram_endpoints: [TelegramEndpointV2]
    var targets: [TargetV2]
}

struct ScheduleV2: Codable {
    var enabled: Bool
    var kind: String
    var hourly_minute: Int
    var daily_at: String
    var timezone: String
}

struct RetentionV2: Codable {
    var keep_last_snapshots: Int
}

struct ChunkingV2: Codable {
    var min_bytes: Int
    var avg_bytes: Int
    var max_bytes: Int
}

struct TelegramGlobalV2: Codable {
    var mode: String
    var mtproto: TelegramMtprotoGlobalV2
}

struct TelegramMtprotoGlobalV2: Codable {
    var api_id: Int
    var api_hash_key: String
}

struct TelegramEndpointV2: Codable, Identifiable {
    var id: String
    var mode: String
    var chat_id: String
    var bot_token_key: String
    var mtproto: TelegramEndpointMtprotoV2
    var rate_limit: TelegramRateLimitV2
}

struct TelegramEndpointMtprotoV2: Codable {
    var session_key: String
}

struct TelegramRateLimitV2: Codable {
    var max_concurrent_uploads: Int
    var min_delay_ms: Int
}

struct TargetV2: Codable, Identifiable {
    var id: String
    var source_path: String
    var label: String
    var endpoint_id: String
    var enabled: Bool
    var schedule: TargetScheduleOverrideV2?
}

struct TargetScheduleOverrideV2: Codable {
    var enabled: Bool?
    var kind: String?
    var hourly_minute: Int?
    var daily_at: String?
}

enum SettingsSection: String, CaseIterable, Identifiable {
    case targets = "Targets"
    case endpoints = "Endpoints"
    case recoveryKey = "Recovery Key"
    case schedule = "Schedule"

    var id: String { rawValue }
}

private enum EndpointHeuristicsDefaults {
    static let lastTouchedEndpointIdKey = "ui.settings.endpoints.lastTouchedEndpointId"
    static let lastTouchedAtKey = "ui.settings.endpoints.lastTouchedAt"
    static let lastSelectedEndpointIdKey = "ui.settings.targets.lastSelectedEndpointId"
    static let lastSelectedAtKey = "ui.settings.targets.lastSelectedAt"

    static func commitEndpointTouched(endpointId: String, now: Date = Date()) {
        let defaults = UserDefaults.standard
        defaults.set(endpointId, forKey: lastTouchedEndpointIdKey)
        defaults.set(now.timeIntervalSince1970, forKey: lastTouchedAtKey)
    }

    static func commitEndpointSelectedForTarget(endpointId: String, now: Date = Date()) {
        let defaults = UserDefaults.standard
        defaults.set(endpointId, forKey: lastSelectedEndpointIdKey)
        defaults.set(now.timeIntervalSince1970, forKey: lastSelectedAtKey)
    }

    static func preferredEndpointId(endpointsSorted: [TelegramEndpointV2]) -> String? {
        guard !endpointsSorted.isEmpty else { return nil }
        let defaults = UserDefaults.standard
        let lastTouchedId = defaults.string(forKey: lastTouchedEndpointIdKey)
        let lastTouchedAt = defaults.double(forKey: lastTouchedAtKey)
        let lastSelectedId = defaults.string(forKey: lastSelectedEndpointIdKey)
        let lastSelectedAt = defaults.double(forKey: lastSelectedAtKey)

        let ids = Set(endpointsSorted.map(\.id))
        let touchedOk = (lastTouchedId != nil && ids.contains(lastTouchedId!))
        let selectedOk = (lastSelectedId != nil && ids.contains(lastSelectedId!))

        if touchedOk && selectedOk {
            return (lastTouchedAt >= lastSelectedAt) ? lastTouchedId : lastSelectedId
        }
        if touchedOk { return lastTouchedId }
        if selectedOk { return lastSelectedId }
        return endpointsSorted.first?.id
    }
}

private struct EndpointListRow: View {
    let endpoint: TelegramEndpointV2

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(endpoint.id)
                .font(.system(size: 12, design: .monospaced))
                .foregroundStyle(.primary)
                .lineLimit(1)
                .truncationMode(.middle)

            Text(endpoint.chat_id.isEmpty ? "Chat: —" : "Chat: \(endpoint.chat_id)")
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .padding(.vertical, 2)
    }
}

struct SettingsWindowRootView: View {
    @EnvironmentObject var model: AppModel
    @State private var section: SettingsSection = .targets

    @State private var settings: SettingsV2?
    @State private var secrets: CliSecretsPresence?
    @State private var vaultKeyPresent: Bool = false
    @State private var loadError: String?

    @State private var selectedTargetId: String?
    @State private var selectedEndpointId: String?
    @State private var savePending: DispatchWorkItem?
    @State private var isSaving: Bool = false
    @State private var saveSeq: Int = 0
    @State private var reloadSeq: Int = 0

    @State private var goldKey: String?
    @State private var goldKeyRevealed: Bool = false
    @State private var showImportRecoveryKeySheet: Bool = false

    @State private var pendingLastTouchedEndpointId: String?
    @State private var pendingLastSelectedEndpointId: String?

    @State private var showEndpointDeleteBlockedAlert: Bool = false
    @State private var endpointDeleteBlockedTargetId: String?

    var body: some View {
        ZStack {
            VisualEffectView(material: .underWindowBackground, blendingMode: .behindWindow, state: .active)
                .ignoresSafeArea()

            VStack(spacing: 0) {
                if let loadError {
                    Text(loadError)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .padding()
                }

                content
            }
        }
        .frame(minWidth: 820, minHeight: 560)
        .overlay(alignment: .bottomTrailing) {
            if let toast = model.toastText {
                ToastPill(text: toast, isError: model.toastIsError)
                    .padding(12)
                    .allowsHitTesting(false)
                    .transition(.opacity)
            }
        }
        .toolbar {
            ToolbarItem(placement: .navigation) {
                Text("Settings")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(.primary)
            }
            ToolbarItem(placement: .principal) {
                Picker("", selection: $section) {
                    ForEach(SettingsSection.allCases) { s in
                        Text(s.rawValue).tag(s)
                    }
                }
                .pickerStyle(.segmented)
                .frame(width: 360)
            }
            ToolbarItemGroup(placement: .automatic) {
                Button {
                    model.openLogs()
                } label: {
                    Label("Open logs", systemImage: "folder")
                }
                .help("Open logs folder in Finder")

                Button {
                    model.openDeveloperWindow()
                } label: {
                    Label("Developer…", systemImage: "wrench.and.screwdriver")
                }
                .help("Open Developer window")
            }
        }
        .onAppear { reload() }
        .onChange(of: selectedTargetId) { _, _ in
            ensureSelectedTargetEndpointValid()
        }
    }

    @ViewBuilder
    private var content: some View {
        switch section {
        case .targets:
            targetsView
        case .endpoints:
            endpointsView
        case .recoveryKey:
            recoveryKeyView
        case .schedule:
            scheduleView
        }
    }

    private var targetsView: some View {
        HStack(spacing: 0) {
            VStack(spacing: 0) {
                ScrollViewReader { proxy in
                    List(selection: $selectedTargetId) {
                        ForEach(settings?.targets ?? []) { t in
                            VStack(alignment: .leading, spacing: 2) {
                                Text(t.label.isEmpty ? t.id : t.label)
                                    .font(.system(size: 13, weight: .semibold))
                                Text(t.source_path)
                                    .font(.system(size: 11, design: .monospaced))
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                            }
                            .id(t.id)
                            .tag(t.id as String?)
                        }
                    }
                    .onChange(of: selectedTargetId) { _, id in
                        guard let id else { return }
                        proxy.scrollTo(id, anchor: .center)
                    }
                    .onAppear {
                        guard let id = selectedTargetId else { return }
                        proxy.scrollTo(id, anchor: .center)
                    }
                }
                .scrollContentBackground(.hidden)
                .background(Color.clear)

                Divider()

                HStack(spacing: 10) {
                    Button { addTarget() } label: {
                        Image(systemName: "plus")
                            .frame(width: 20, height: 20)
                    }
                    .buttonStyle(.bordered)

                    Button { deleteSelectedTarget() } label: {
                        Image(systemName: "minus")
                            .frame(width: 20, height: 20)
                    }
                    .buttonStyle(.bordered)
                    .disabled(selectedTargetId == nil)

                    Spacer()
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
        }
        .frame(minWidth: 220, idealWidth: 240, maxWidth: 280)

        Divider()

            GroupBox {
                if let idx = selectedTargetIndex(), settings != nil {
                    ScrollView {
                        TargetEditor(
                            settings: Binding(
                                get: { self.settings! },
                                set: { new in
                                    self.settings = new
                                    self.queueAutoSave()
                                }
                            ),
                            secrets: secrets,
                            targetIndex: idx,
                            embedded: false,
                            onEndpointSelected: { endpointId in
                                pendingLastSelectedEndpointId = endpointId
                            },
                            onEditEndpoint: { endpointId in
                                section = .endpoints
                                selectedEndpointId = endpointId
                            },
                            onReload: { self.reload() }
                        )
                        .padding(.vertical, 10)
                        .padding(.horizontal, 12)
                    }
                    .overlay(alignment: .topTrailing) {
                        if isSaving {
                            Text("Saving…")
                                .font(.system(size: 12, weight: .semibold))
                                .foregroundStyle(.secondary)
                                .padding()
                        }
                    }
                } else {
                    Text("Select a target")
                        .foregroundStyle(.secondary)
                        .padding(.vertical, 14)
                        .padding(.horizontal, 12)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private var endpointsView: some View {
        let endpoints = sortedEndpoints()

        return HStack(spacing: 0) {
            VStack(spacing: 0) {
                ScrollViewReader { proxy in
                    List(selection: $selectedEndpointId) {
                        ForEach(endpoints) { ep in
                            EndpointListRow(endpoint: ep)
                            .id(ep.id)
                            .tag(ep.id as String?)
                        }
                    }
                    .onChange(of: selectedEndpointId) { _, id in
                        guard let id else { return }
                        proxy.scrollTo(id, anchor: .center)
                    }
                    .onAppear {
                        guard let id = selectedEndpointId else { return }
                        proxy.scrollTo(id, anchor: .center)
                    }
                }
                .scrollContentBackground(.hidden)
                .background(Color.clear)

                Divider()

                HStack(spacing: 10) {
                    Button { addEndpoint() } label: {
                        Image(systemName: "plus")
                            .frame(width: 20, height: 20)
                    }
                    .buttonStyle(.bordered)
                    .disabled(settings == nil)

                    Button { deleteSelectedEndpoint() } label: {
                        Image(systemName: "minus")
                            .frame(width: 20, height: 20)
                    }
                    .buttonStyle(.bordered)
                    .disabled(selectedEndpointId == nil || settings == nil)

                    Spacer()
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
            }
            .frame(minWidth: 220, idealWidth: 240, maxWidth: 280)

            Divider()

            GroupBox {
                if let endpointId = selectedEndpointId, settings != nil {
                    ScrollView {
                        EndpointEditor(
                            settings: Binding(
                                get: { self.settings! },
                                set: { new in
                                    self.settings = new
                                    self.queueAutoSave()
                                }
                            ),
                            secrets: secrets,
                            endpointId: endpointId,
                            onEndpointTouchedPending: { pendingLastTouchedEndpointId = $0 },
                            onEndpointTouchedCommitted: { EndpointHeuristicsDefaults.commitEndpointTouched(endpointId: $0) },
                            onReload: { self.reload() }
                        )
                        .padding(.vertical, 10)
                        .padding(.horizontal, 12)
                    }
                    .overlay(alignment: .topTrailing) {
                        if isSaving {
                            Text("Saving…")
                                .font(.system(size: 12, weight: .semibold))
                                .foregroundStyle(.secondary)
                                .padding()
                        }
                    }
                } else {
                    Text("Select an endpoint")
                        .foregroundStyle(.secondary)
                        .padding(.vertical, 14)
                        .padding(.horizontal, 12)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .alert("Cannot delete endpoint", isPresented: $showEndpointDeleteBlockedAlert) {
            Button("Cancel", role: .cancel) {}
            Button("Go to Targets") {
                if let endpointDeleteBlockedTargetId {
                    selectedTargetId = endpointDeleteBlockedTargetId
                }
                section = .targets
            }
        } message: {
            Text("This endpoint is still referenced by at least one target. Rebind/unbind it in Targets, then retry deletion.")
        }
        .onAppear {
            if selectedEndpointId == nil, let s = settings {
                selectedEndpointId = EndpointHeuristicsDefaults.preferredEndpointId(endpointsSorted: sortedEndpoints(settings: s))
            }
        }
    }

    private var recoveryKeyView: some View {
        let masterKeyPresent = secrets?.masterKeyPresent ?? false

        return VStack(alignment: .leading, spacing: 14) {
            Text("Recovery Key")
                .font(.system(size: 18, weight: .bold))
            Text("Export/import the gold key (TBK1) to move restore capability across devices.")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(.secondary)

            GroupBox {
                VStack(spacing: 0) {
                    HStack(spacing: 12) {
                        Text("Vault key")
                            .font(.system(size: 13, weight: .semibold))
                            .frame(width: 110, alignment: .leading)
                        Spacer()
                        Text(vaultKeyPresent ? "Present" : "Missing")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(vaultKeyPresent ? .green : .red)
                        Text("· Keychain")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.secondary)
                    }
                    .padding(.vertical, 10)

                    Divider()

                    HStack(spacing: 12) {
                        Text("Recovery key")
                            .font(.system(size: 13, weight: .semibold))
                            .frame(width: 110, alignment: .leading)

                        Text(recoveryKeyDisplayText())
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 6)
                            .background(.background)
                            .clipShape(RoundedRectangle(cornerRadius: 7))
                            .overlay(RoundedRectangle(cornerRadius: 7).stroke(.quaternary))

                        Button(goldKeyRevealed ? "Hide" : "Reveal") { toggleRevealRecoveryKey() }
                            .buttonStyle(.bordered)
                            .disabled(!masterKeyPresent)

                        Button("Copy") { copyRecoveryKeyToClipboard() }
                            .buttonStyle(.borderedProminent)
                            .disabled(!masterKeyPresent)

                        if !masterKeyPresent {
                            Text("Missing")
                                .font(.system(size: 12, weight: .semibold))
                                .foregroundStyle(.red)
                                .frame(width: 64, alignment: .trailing)
                        }
                    }
                    .padding(.vertical, 10)

                    Divider()

                    HStack(spacing: 12) {
                        Text("Export")
                            .font(.system(size: 13, weight: .semibold))
                            .frame(width: 110, alignment: .leading)
                        Text("Format: TBK1:<base64url_no_pad>")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        Button("Export…") { exportRecoveryKeyToFile() }
                            .buttonStyle(.bordered)
                            .disabled(!masterKeyPresent)
                    }
                    .padding(.vertical, 10)

                    Divider()

                    HStack(spacing: 12) {
                        Text("Import")
                            .font(.system(size: 13, weight: .semibold))
                            .frame(width: 110, alignment: .leading)
                        Text("Import opens a sheet and requires confirmation.")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        Button("Import…") { showImportRecoveryKeySheet = true }
                            .buttonStyle(.bordered)
                    }
                    .padding(.vertical, 10)
                }
                .padding(.vertical, 2)
                .padding(.horizontal, 12)
            }

            Text("New Mac restore requires: recovery key + bot token + chat_id.")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(.secondary)

            Spacer()
        }
        .padding()
        .sheet(isPresented: $showImportRecoveryKeySheet) {
            ImportRecoveryKeySheet(
                masterKeyPresent: secrets?.masterKeyPresent ?? false,
                onImport: { key, force in
                    importRecoveryKey(key: key, force: force)
                }
            )
            .environmentObject(model)
        }
    }

    private var scheduleView: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Schedule")
                .font(.system(size: 18, weight: .bold))
            Text("Global schedule (targets inherit by default).")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(.secondary)

            if let s = settings {
                Toggle("Enable", isOn: Binding(
                    get: { s.schedule.enabled },
                    set: { v in
                        settings?.schedule.enabled = v
                        queueAutoSave()
                    }
                ))

                HStack {
                    Text("Frequency")
                    Spacer()
                    Picker("", selection: Binding(
                        get: { s.schedule.kind },
                        set: { v in
                            settings?.schedule.kind = v
                            queueAutoSave()
                        }
                    )) {
                        Text("Hourly").tag("hourly")
                        Text("Daily").tag("daily")
                    }
                    .pickerStyle(.menu)
                    .frame(width: 140)
                }

                if s.schedule.kind == "hourly" {
                    HStack {
                        Text("Minute")
                        Spacer()
                        Stepper(
                            value: Binding(
                                get: { s.schedule.hourly_minute },
                                set: { v in
                                    settings?.schedule.hourly_minute = v
                                    queueAutoSave()
                                }
                            ),
                            in: 0...59
                        ) {
                            Text(String(format: "%02d", s.schedule.hourly_minute))
                                .font(.system(.body, design: .monospaced))
                        }
                    }
                } else {
                    HStack {
                        Text("Daily at")
                        Spacer()
                        TextField("02:00", text: Binding(
                            get: { s.schedule.daily_at },
                            set: { v in
                                settings?.schedule.daily_at = v
                                queueAutoSave()
                            }
                        ))
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 120)
                    }
                }
            }

            Spacer()
        }
        .padding()
    }

    private func selectedTargetIndex() -> Int? {
        guard let selectedTargetId, let s = settings else { return nil }
        return s.targets.firstIndex(where: { $0.id == selectedTargetId })
    }

    private func reload() {
        guard let cli = model.cliPath() else {
            loadError = "televybackup CLI not found (set TELEVYBACKUP_CLI_PATH)"
            return
        }

        reloadSeq += 1
        let seq = reloadSeq

        DispatchQueue.global(qos: .userInitiated).async {
            let res = model.runCommandCapture(
                exe: cli,
                args: ["--json", "settings", "get", "--with-secrets"],
                timeoutSeconds: 30
            )
            if res.status != 0 {
                DispatchQueue.main.async {
                    guard seq == self.reloadSeq else { return }
                    self.vaultKeyPresent = false
                    self.loadError = "settings get failed: exit=\(res.status)"
                }
                return
            }
            guard let data = res.stdout.data(using: .utf8) else {
                DispatchQueue.main.async {
                    guard seq == self.reloadSeq else { return }
                    self.vaultKeyPresent = false
                    self.loadError = "settings get: bad output"
                }
                return
            }

            let decoded: CliSettingsGetResponse
            do {
                decoded = try JSONDecoder().decode(CliSettingsGetResponse.self, from: data)
            } catch {
                DispatchQueue.main.async {
                    guard seq == self.reloadSeq else { return }
                    self.vaultKeyPresent = false
                    self.loadError = "settings get: JSON decode failed"
                }
                return
            }

            DispatchQueue.main.async {
                guard seq == self.reloadSeq else { return }
                self.vaultKeyPresent = true
                self.settings = decoded.settings
                self.secrets = decoded.secrets
                self.loadError = nil
                if self.selectedTargetId == nil {
                    self.selectedTargetId = decoded.settings.targets.first?.id
                }
                if let selected = self.selectedEndpointId {
                    let ids = Set(decoded.settings.telegram_endpoints.map(\.id))
                    if !ids.contains(selected) {
                        self.selectedEndpointId = nil
                    }
                }
                if self.selectedEndpointId == nil {
                    self.selectedEndpointId = EndpointHeuristicsDefaults.preferredEndpointId(
                        endpointsSorted: self.sortedEndpoints(settings: decoded.settings)
                    )
                }
            }
        }
    }

    private func exportRecoveryKeyToFile() {
        loadRecoveryKeyIfNeeded()
        guard let goldKey else { return }

        let panel = NSSavePanel()
        panel.canCreateDirectories = true
        panel.nameFieldStringValue = "televybackup-recovery-key.txt"
        panel.allowedContentTypes = [UTType.plainText]
        panel.prompt = "Export"

        if panel.runModal() != .OK { return }
        guard let url = panel.url else { return }

        do {
            try goldKey.write(to: url, atomically: true, encoding: .utf8)
        } catch {
            // Best-effort UX: keep the view responsive; user can retry.
        }
    }

    private func queueAutoSave() {
        savePending?.cancel()
        let work = DispatchWorkItem { saveNow() }
        savePending = work
        DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 0.35, execute: work)
    }

    private func saveNow() {
        guard let cli = model.cliPath(), let settings else { return }
        saveSeq += 1
        let seq = saveSeq
        DispatchQueue.main.async {
            self.isSaving = true
        }

        let toml = renderToml(settings: settings)
        DispatchQueue.global(qos: .userInitiated).async {
            let res = model.runCommandCapture(
                exe: cli,
                args: ["--json", "settings", "set"],
                stdin: toml + "\n",
                timeoutSeconds: 30
            )
            DispatchQueue.main.async {
                guard seq == self.saveSeq else { return }
                self.isSaving = false
                if res.status != 0 {
                    self.loadError = "settings set failed: exit=\(res.status)"
                    return
                }
                if let id = self.pendingLastTouchedEndpointId {
                    EndpointHeuristicsDefaults.commitEndpointTouched(endpointId: id)
                    self.pendingLastTouchedEndpointId = nil
                }
                if let id = self.pendingLastSelectedEndpointId {
                    EndpointHeuristicsDefaults.commitEndpointSelectedForTarget(endpointId: id)
                    self.pendingLastSelectedEndpointId = nil
                }
                self.reload()
            }
        }
    }

    private func renderToml(settings: SettingsV2) -> String {
        var out: [String] = []
        out.append("version = \(settings.version)")
        out.append("")

        out.append("[schedule]")
        out.append("enabled = \(settings.schedule.enabled ? "true" : "false")")
        out.append("kind = \(tomlString(settings.schedule.kind))")
        out.append("hourly_minute = \(settings.schedule.hourly_minute)")
        out.append("daily_at = \(tomlString(settings.schedule.daily_at))")
        out.append("timezone = \(tomlString(settings.schedule.timezone))")
        out.append("")

        out.append("[retention]")
        out.append("keep_last_snapshots = \(settings.retention.keep_last_snapshots)")
        out.append("")

        out.append("[chunking]")
        out.append("min_bytes = \(settings.chunking.min_bytes)")
        out.append("avg_bytes = \(settings.chunking.avg_bytes)")
        out.append("max_bytes = \(settings.chunking.max_bytes)")
        out.append("")

        out.append("[telegram]")
        out.append("mode = \(tomlString(settings.telegram.mode))")
        out.append("")

        out.append("[telegram.mtproto]")
        out.append("api_id = \(settings.telegram.mtproto.api_id)")
        out.append("api_hash_key = \(tomlString(settings.telegram.mtproto.api_hash_key))")
        out.append("")

        for ep in settings.telegram_endpoints {
            out.append("[[telegram_endpoints]]")
            out.append("id = \(tomlString(ep.id))")
            out.append("mode = \(tomlString(ep.mode))")
            out.append("chat_id = \(tomlString(ep.chat_id))")
            out.append("bot_token_key = \(tomlString(ep.bot_token_key))")
            out.append("")

            out.append("[telegram_endpoints.mtproto]")
            out.append("session_key = \(tomlString(ep.mtproto.session_key))")
            out.append("")

            out.append("[telegram_endpoints.rate_limit]")
            out.append("max_concurrent_uploads = \(ep.rate_limit.max_concurrent_uploads)")
            out.append("min_delay_ms = \(ep.rate_limit.min_delay_ms)")
            out.append("")
        }

        for t in settings.targets {
            out.append("[[targets]]")
            out.append("id = \(tomlString(t.id))")
            out.append("source_path = \(tomlString(t.source_path))")
            out.append("label = \(tomlString(t.label))")
            out.append("endpoint_id = \(tomlString(t.endpoint_id))")
            out.append("enabled = \(t.enabled ? "true" : "false")")
            out.append("")

            if let s = t.schedule {
                out.append("[targets.schedule]")
                if let v = s.enabled { out.append("enabled = \(v ? "true" : "false")") }
                if let v = s.kind { out.append("kind = \(tomlString(v))") }
                if let v = s.hourly_minute { out.append("hourly_minute = \(v)") }
                if let v = s.daily_at { out.append("daily_at = \(tomlString(v))") }
                out.append("")
            }
        }

        return out.joined(separator: "\n")
    }

    private func tomlString(_ s: String) -> String {
        let escaped = s
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
            .replacingOccurrences(of: "\n", with: "\\n")
        return "\"\(escaped)\""
    }

    private func sortedEndpoints(settings: SettingsV2) -> [TelegramEndpointV2] {
        settings.telegram_endpoints.sorted { a, b in
            a.id.localizedStandardCompare(b.id) == .orderedAscending
        }
    }

    private func sortedEndpoints() -> [TelegramEndpointV2] {
        guard let settings else { return [] }
        return sortedEndpoints(settings: settings)
    }

    private func preferredEndpointId(settings: SettingsV2) -> String? {
        EndpointHeuristicsDefaults.preferredEndpointId(endpointsSorted: sortedEndpoints(settings: settings))
    }

    private func ensureSelectedTargetEndpointValid() {
        guard var s = settings else { return }
        guard let idx = selectedTargetIndex() else { return }

        let current = s.targets[idx].endpoint_id
        let exists = s.telegram_endpoints.contains(where: { $0.id == current })
        if exists { return }

        guard let preferred = preferredEndpointId(settings: s) else { return }
        s.targets[idx].endpoint_id = preferred
        settings = s
        queueAutoSave()
    }

    private func addTarget() {
        guard settings != nil else { return }
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.prompt = "Choose"
        if panel.runModal() != .OK { return }
        guard let url = panel.url else { return }

        var s = settings!
        let targetId = "t_" + UUID().uuidString.lowercased().prefix(8)

        let endpointId: String
        if let preferred = preferredEndpointId(settings: s) {
            endpointId = preferred
        } else {
            let newEndpointId = "ep_" + UUID().uuidString.lowercased().prefix(8)
            endpointId = String(newEndpointId)
            s.telegram_endpoints.append(
                TelegramEndpointV2(
                    id: endpointId,
                    mode: "mtproto",
                    chat_id: "",
                    bot_token_key: "telegram.bot_token.\(endpointId)",
                    mtproto: TelegramEndpointMtprotoV2(session_key: "telegram.mtproto.session.\(endpointId)"),
                    rate_limit: TelegramRateLimitV2(max_concurrent_uploads: 2, min_delay_ms: 250)
                )
            )
            pendingLastTouchedEndpointId = endpointId
        }
        s.targets.append(
            TargetV2(
                id: String(targetId),
                source_path: url.path,
                label: "manual",
                endpoint_id: String(endpointId),
                enabled: true,
                schedule: nil
            )
        )
        settings = s
        selectedTargetId = String(targetId)
        queueAutoSave()
    }

    private func deleteSelectedTarget() {
        guard var s = settings, let selectedTargetId else { return }
        s.targets.removeAll { $0.id == selectedTargetId }
        settings = s
        self.selectedTargetId = s.targets.first?.id
        queueAutoSave()
    }

    private func addEndpoint() {
        guard settings != nil else { return }
        var s = settings!
        let endpointId = "ep_" + UUID().uuidString.lowercased().prefix(8)
        s.telegram_endpoints.append(
            TelegramEndpointV2(
                id: String(endpointId),
                mode: "mtproto",
                chat_id: "",
                bot_token_key: "telegram.bot_token.\(endpointId)",
                mtproto: TelegramEndpointMtprotoV2(session_key: "telegram.mtproto.session.\(endpointId)"),
                rate_limit: TelegramRateLimitV2(max_concurrent_uploads: 2, min_delay_ms: 250)
            )
        )
        settings = s
        selectedEndpointId = String(endpointId)
        pendingLastTouchedEndpointId = String(endpointId)
        queueAutoSave()
    }

    private func deleteSelectedEndpoint() {
        guard var s = settings, let endpointId = selectedEndpointId else { return }
        if let referencing = s.targets.first(where: { $0.endpoint_id == endpointId }) {
            endpointDeleteBlockedTargetId = referencing.id
            showEndpointDeleteBlockedAlert = true
            return
        }

        s.telegram_endpoints.removeAll { $0.id == endpointId }
        settings = s
        selectedEndpointId = preferredEndpointId(settings: s)
        queueAutoSave()
    }

    private func recoveryKeyDisplayText() -> String {
        guard let goldKey else {
            return (secrets?.masterKeyPresent ?? false) ? "TBK1:••••••••••••••••" : "—"
        }
        return goldKeyRevealed ? goldKey : "TBK1:••••••••••••••••"
    }

    private func loadRecoveryKeyIfNeeded() {
        if goldKey != nil { return }
        guard let cli = model.cliPath() else { return }
        let res = model.runCommandCapture(
            exe: cli,
            args: ["--json", "secrets", "export-master-key", "--i-understand"],
            timeoutSeconds: 30
        )
        guard res.status == 0, let data = res.stdout.data(using: .utf8) else { return }
        guard
            let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let key = obj["goldKey"] as? String
        else { return }
        goldKey = key
    }

    private func toggleRevealRecoveryKey() {
        loadRecoveryKeyIfNeeded()
        if goldKey == nil { return }
        goldKeyRevealed.toggle()
    }

    private func copyRecoveryKeyToClipboard() {
        loadRecoveryKeyIfNeeded()
        guard let goldKey else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(goldKey, forType: .string)
        reload()
    }

    private func importRecoveryKey(key: String, force: Bool) {
        guard let cli = model.cliPath() else { return }
        var args = ["--json", "secrets", "import-master-key"]
        if force { args.append("--force") }
        _ = model.runCommandCapture(
            exe: cli,
            args: args,
            stdin: key + "\n",
            timeoutSeconds: 30
        )
        goldKey = nil
        goldKeyRevealed = false
        reload()
    }
}

private struct ImportRecoveryKeySheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var draft: String = ""
    @State private var confirmShown: Bool = false

    let masterKeyPresent: Bool
    let onImport: (_ key: String, _ force: Bool) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Import Recovery Key")
                .font(.system(size: 18, weight: .bold))
            Text("Paste TBK1:… and confirm to import. This will enable restores on this device.")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(.secondary)

            TextField("TBK1:…", text: $draft)
                .textFieldStyle(.roundedBorder)
                .font(.system(size: 12, design: .monospaced))

            Spacer()

            HStack {
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Spacer()
                Button("Import…") { confirmShown = true }
                    .buttonStyle(.borderedProminent)
                    .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding()
        .frame(width: 520, height: 220)
        .alert("Confirm Import", isPresented: $confirmShown) {
            Button("Cancel", role: .cancel) {}
            Button("Import", role: .destructive) {
                let key = draft.trimmingCharacters(in: .whitespacesAndNewlines)
                if key.isEmpty { return }
                onImport(key, masterKeyPresent)
                dismiss()
            }
        } message: {
            if masterKeyPresent {
                Text("This will overwrite the existing master key. Existing encrypted backups may become unreadable if the key does not match.")
            } else {
                Text("Import this recovery key?")
            }
        }
    }
}

struct TargetEditor: View {
    @EnvironmentObject var model: AppModel
    @Binding var settings: SettingsV2
    let secrets: CliSecretsPresence?
    let targetIndex: Int
    let embedded: Bool
    let onEndpointSelected: (_ endpointId: String) -> Void
    let onEditEndpoint: (_ endpointId: String) -> Void
    let onReload: () -> Void

    @State private var validateText: String = "Not validated"
    @State private var validateOk: Bool? = nil

    init(
        settings: Binding<SettingsV2>,
        secrets: CliSecretsPresence?,
        targetIndex: Int,
        embedded: Bool = false,
        onEndpointSelected: @escaping (_ endpointId: String) -> Void,
        onEditEndpoint: @escaping (_ endpointId: String) -> Void,
        onReload: @escaping () -> Void
    ) {
        self._settings = settings
        self.secrets = secrets
        self.targetIndex = targetIndex
        self.embedded = embedded
        self.onEndpointSelected = onEndpointSelected
        self.onEditEndpoint = onEditEndpoint
        self.onReload = onReload
    }

    var body: some View {
        let target = settings.targets[targetIndex]
        let epIndex = settings.telegram_endpoints.firstIndex(where: { $0.id == target.endpoint_id })
        let endpointsSorted = settings.telegram_endpoints.sorted { a, b in
            a.id.localizedStandardCompare(b.id) == .orderedAscending
        }

        VStack(alignment: .leading, spacing: 12) {
            if !embedded {
                Text("Target")
                    .font(.system(size: 18, weight: .bold))
            }

            HStack {
                Text("ID")
                Spacer()
                Text(target.id).font(.system(.body, design: .monospaced))
                    .foregroundStyle(.secondary)
            }

            Toggle("Enabled", isOn: Binding(
                get: { settings.targets[targetIndex].enabled },
                set: { v in settings.targets[targetIndex].enabled = v }
            ))

            HStack {
                Text("Source")
                Spacer()
                Text(target.source_path)
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            HStack {
                Text("Label")
                Spacer()
                TextField("manual", text: Binding(
                    get: { settings.targets[targetIndex].label },
                    set: { v in settings.targets[targetIndex].label = v }
                ))
                .textFieldStyle(.roundedBorder)
                .frame(width: 220)
            }

            Divider()

            HStack {
                Text("Endpoint")
                Spacer()
                Picker("", selection: Binding(
                    get: { settings.targets[targetIndex].endpoint_id },
                    set: { v in
                        settings.targets[targetIndex].endpoint_id = v
                        onEndpointSelected(v)
                    }
                )) {
                    ForEach(endpointsSorted) { ep in
                        Text(ep.id).tag(ep.id)
                    }
                }
                .pickerStyle(.menu)
                .frame(width: 260)
            }

            if let epIndex {
                endpointSummary(epIndex: epIndex)
            } else {
                Text("Endpoint not found in config")
                    .foregroundStyle(.secondary)
            }

            Divider()

            scheduleOverrideEditor

            if !embedded {
                Spacer()
            }
        }
        .padding(embedded ? 0 : 16)
        .onChange(of: settings.targets[targetIndex].endpoint_id) { _, _ in
            validateOk = nil
            validateText = "Not validated"
        }
    }

    @ViewBuilder
    private func endpointSummary(epIndex: Int) -> some View {
        let ep = settings.telegram_endpoints[epIndex]
        let botPresent = secrets?.telegramBotTokenPresentByEndpoint?[ep.id] ?? false
        let sessionPresent = secrets?.telegramMtprotoSessionPresentByEndpoint?[ep.id] ?? false
        let apiHashPresent = secrets?.telegramMtprotoApiHashPresent ?? false

        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("Chat ID")
                Spacer()
                Text(ep.chat_id.isEmpty ? "—" : ep.chat_id)
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            HStack {
                Text("Bot token")
                Spacer()
                Text(botPresent ? "Saved (encrypted)" : "Not set")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(botPresent ? .green : .secondary)
            }

            HStack {
                Text("API ID")
                Spacer()
                Text(String(settings.telegram.mtproto.api_id))
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(.secondary)
            }

            HStack {
                Text("API hash")
                Spacer()
                Text(apiHashPresent ? "Saved (encrypted)" : "Not set")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(apiHashPresent ? .green : .secondary)
            }

            HStack {
                Text("Session")
                Spacer()
                Text(sessionPresent ? "Saved" : "None")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.secondary)
            }

            HStack(spacing: 10) {
                Button("Edit endpoint…") { onEditEndpoint(ep.id) }
                    .buttonStyle(.bordered)

                Button("Test connection") { testConnection(endpointId: ep.id) }
                    .buttonStyle(.borderedProminent)

                Spacer()
                Text(validateText)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(validateOk == true ? .green : (validateOk == false ? .red : .secondary))
            }
        }
    }

    private var scheduleOverrideEditor: some View {
        let hasOverride = settings.targets[targetIndex].schedule != nil

        return VStack(alignment: .leading, spacing: 10) {
            Text("Schedule")
                .font(.system(size: 13, weight: .semibold))

            Toggle("Override global schedule", isOn: Binding(
                get: { settings.targets[targetIndex].schedule != nil },
                set: { v in
                    if v {
                        let global = settings.schedule
                        settings.targets[targetIndex].schedule = TargetScheduleOverrideV2(
                            enabled: global.enabled,
                            kind: global.kind,
                            hourly_minute: global.hourly_minute,
                            daily_at: global.daily_at
                        )
                    } else {
                        settings.targets[targetIndex].schedule = nil
                    }
                }
            ))

            if hasOverride, settings.targets[targetIndex].schedule != nil {
                Toggle("Enable", isOn: Binding(
                    get: { settings.targets[targetIndex].schedule?.enabled ?? settings.schedule.enabled },
                    set: { v in settings.targets[targetIndex].schedule?.enabled = v }
                ))

                HStack {
                    Text("Frequency")
                    Spacer()
                    Picker("", selection: Binding(
                        get: { settings.targets[targetIndex].schedule?.kind ?? settings.schedule.kind },
                        set: { v in
                            settings.targets[targetIndex].schedule?.kind = v
                            if v == "hourly" {
                                if settings.targets[targetIndex].schedule?.hourly_minute == nil {
                                    settings.targets[targetIndex].schedule?.hourly_minute = settings.schedule.hourly_minute
                                }
                                settings.targets[targetIndex].schedule?.daily_at = nil
                            } else {
                                if settings.targets[targetIndex].schedule?.daily_at == nil {
                                    settings.targets[targetIndex].schedule?.daily_at = settings.schedule.daily_at
                                }
                                settings.targets[targetIndex].schedule?.hourly_minute = nil
                            }
                        }
                    )) {
                        Text("Hourly").tag("hourly")
                        Text("Daily").tag("daily")
                    }
                    .pickerStyle(.menu)
                    .frame(width: 140)
                }

                if (settings.targets[targetIndex].schedule?.kind ?? settings.schedule.kind) == "hourly" {
                    HStack {
                        Text("Minute")
                        Spacer()
                        Stepper(
                            value: Binding(
                                get: { settings.targets[targetIndex].schedule?.hourly_minute ?? settings.schedule.hourly_minute },
                                set: { v in settings.targets[targetIndex].schedule?.hourly_minute = v }
                            ),
                            in: 0...59
                        ) {
                            Text(String(format: "%02d", settings.targets[targetIndex].schedule?.hourly_minute ?? settings.schedule.hourly_minute))
                                .font(.system(.body, design: .monospaced))
                        }
                    }
                } else {
                    HStack {
                        Text("Daily at")
                        Spacer()
                        TextField("02:00", text: Binding(
                            get: { settings.targets[targetIndex].schedule?.daily_at ?? settings.schedule.daily_at },
                            set: { v in settings.targets[targetIndex].schedule?.daily_at = v }
                        ))
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 120)
                    }
                }

                HStack {
                    Spacer()
                    Button("Reset to global") { settings.targets[targetIndex].schedule = nil }
                        .buttonStyle(.bordered)
                }
            }
        }
    }

    private func testConnection(endpointId: String) {
        guard let cli = model.cliPath() else { return }
        let res = model.runCommandCapture(
            exe: cli,
            args: ["--json", "telegram", "validate", "--endpoint-id", endpointId],
            timeoutSeconds: 180
        )
        if res.status == 0 {
            validateOk = true
            validateText = "Connected"
            onReload()
        } else {
            validateOk = false
            validateText = "Failed (see ui.log)"
        }
    }
}

struct EndpointEditor: View {
    @EnvironmentObject var model: AppModel
    @Binding var settings: SettingsV2
    let secrets: CliSecretsPresence?
    let endpointId: String
    let onEndpointTouchedPending: (_ endpointId: String) -> Void
    let onEndpointTouchedCommitted: (_ endpointId: String) -> Void
    let onReload: () -> Void

    @FocusState private var tokenFocused: Bool
    @FocusState private var apiHashFocused: Bool

    @State private var botTokenDraft: String = ""
    @State private var botTokenDraftMasked: Bool = false
    @State private var apiHashDraft: String = ""
    @State private var apiHashDraftMasked: Bool = false
    @State private var validateText: String = "Not validated"
    @State private var validateOk: Bool? = nil

    var body: some View {
        let epIndex = settings.telegram_endpoints.firstIndex(where: { $0.id == endpointId })
        let botPresent = secrets?.telegramBotTokenPresentByEndpoint?[endpointId] ?? false
        let sessionPresent = secrets?.telegramMtprotoSessionPresentByEndpoint?[endpointId] ?? false
        let apiHashPresent = secrets?.telegramMtprotoApiHashPresent ?? false

        VStack(alignment: .leading, spacing: 12) {
            Text("Endpoint")
                .font(.system(size: 18, weight: .bold))

            HStack {
                Text("ID")
                Spacer()
                Text(endpointId)
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(.secondary)
            }

            if let epIndex {
                HStack {
                    Text("Mode")
                    Spacer()
                    Text(settings.telegram_endpoints[epIndex].mode)
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(.secondary)
                }

                Divider()
                    .padding(.vertical, 10)

                HStack {
                    Text("Chat ID")
                    Spacer()
                    TextField("-100123…", text: Binding(
                        get: { settings.telegram_endpoints[epIndex].chat_id },
                        set: { v in
                            settings.telegram_endpoints[epIndex].chat_id = v
                            onEndpointTouchedPending(endpointId)
                        }
                    ))
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 260)
                }

                HStack {
                    Text("Bot token")
                    Spacer()
                    Text(botPresent ? "Saved (encrypted)" : "Not set")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(botPresent ? .green : .secondary)
                }

                HStack(spacing: 8) {
                    SecureField("Paste new bot token", text: $botTokenDraft)
                        .focused($tokenFocused)
                    Button("Save token") { saveBotToken(endpointId: endpointId) }
                        .buttonStyle(.bordered)
                }

                Divider()
                    .padding(.vertical, 14)

                Text("Telegram MTProto (global)")
                    .font(.system(size: 13, weight: .semibold))
                    .padding(.bottom, 2)

                HStack {
                    Text("API ID")
                    Spacer()
                    TextField("123456", value: Binding(
                        get: { settings.telegram.mtproto.api_id },
                        set: { v in
                            settings.telegram.mtproto.api_id = v
                            onEndpointTouchedPending(endpointId)
                        }
                    ), formatter: NumberFormatter())
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 180)
                }

                HStack {
                    Text("API hash")
                    Spacer()
                    Text(apiHashPresent ? "Saved (encrypted)" : "Not set")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(apiHashPresent ? .green : .secondary)
                }

                HStack(spacing: 8) {
                    SecureField("Paste api_hash", text: $apiHashDraft)
                        .focused($apiHashFocused)
                    Button("Save api_hash") { saveApiHash(endpointId: endpointId) }
                        .buttonStyle(.bordered)
                }

                Divider()
                    .padding(.vertical, 14)

                HStack {
                    Button("Clear sessions") { clearSessions(endpointId: endpointId) }
                        .buttonStyle(.bordered)
                    Spacer()
                    Text(sessionPresent ? "Session: saved" : "Session: none")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.secondary)
                }

                HStack(spacing: 10) {
                    Button("Test connection") { testConnection(endpointId: endpointId) }
                        .buttonStyle(.borderedProminent)
                    Spacer()
                    Text(validateText)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(validateOk == true ? .green : (validateOk == false ? .red : .secondary))
                }

                Text("Delete: go to Targets to unbind if referenced")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .padding(.top, 6)
            } else {
                Text("Endpoint not found in config")
                    .foregroundStyle(.secondary)
            }
        }
        .onChange(of: botPresent) { _, isPresent in
            if isPresent && botTokenDraft.isEmpty {
                botTokenDraft = String(repeating: "•", count: 18)
                botTokenDraftMasked = true
            } else if !isPresent && botTokenDraftMasked {
                botTokenDraft = ""
                botTokenDraftMasked = false
            }
        }
        .onChange(of: apiHashPresent) { _, isPresent in
            if isPresent && apiHashDraft.isEmpty {
                apiHashDraft = String(repeating: "•", count: 18)
                apiHashDraftMasked = true
            } else if !isPresent && apiHashDraftMasked {
                apiHashDraft = ""
                apiHashDraftMasked = false
            }
        }
        .onAppear {
            if botPresent && botTokenDraft.isEmpty {
                botTokenDraft = String(repeating: "•", count: 18)
                botTokenDraftMasked = true
            }
            if apiHashPresent && apiHashDraft.isEmpty {
                apiHashDraft = String(repeating: "•", count: 18)
                apiHashDraftMasked = true
            }
        }
        .onChange(of: tokenFocused) { _, isFocused in
            if isFocused, botTokenDraftMasked {
                botTokenDraft = ""
                botTokenDraftMasked = false
            }
        }
        .onChange(of: apiHashFocused) { _, isFocused in
            if isFocused, apiHashDraftMasked {
                apiHashDraft = ""
                apiHashDraftMasked = false
            }
        }
        .onChange(of: endpointId) { _, _ in
            validateOk = nil
            validateText = "Not validated"
            botTokenDraft = ""
            botTokenDraftMasked = false
            apiHashDraft = ""
            apiHashDraftMasked = false
        }
        .onSubmit {
            if tokenFocused {
                saveBotToken(endpointId: endpointId)
            } else if apiHashFocused {
                saveApiHash(endpointId: endpointId)
            }
        }
    }

    private func saveBotToken(endpointId: String) {
        guard let cli = model.cliPath() else { return }
        let token = botTokenDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        if token.isEmpty { return }
        let res = model.runCommandCapture(
            exe: cli,
            args: ["--json", "secrets", "set-telegram-bot-token", "--endpoint-id", endpointId],
            stdin: token + "\n",
            timeoutSeconds: 30
        )
        if res.status == 0 {
            botTokenDraft = String(repeating: "•", count: 18)
            botTokenDraftMasked = true
            onEndpointTouchedCommitted(endpointId)
            onReload()
        }
    }

    private func saveApiHash(endpointId: String) {
        guard let cli = model.cliPath() else { return }
        if apiHashDraftMasked { return }
        let apiHash = apiHashDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        if apiHash.isEmpty { return }
        let res = model.runCommandCapture(
            exe: cli,
            args: ["--json", "secrets", "set-telegram-api-hash"],
            stdin: apiHash + "\n",
            timeoutSeconds: 30
        )
        if res.status == 0 {
            apiHashDraft = String(repeating: "•", count: 18)
            apiHashDraftMasked = true
            onEndpointTouchedCommitted(endpointId)
            onReload()
        }
    }

    private func clearSessions(endpointId: String) {
        guard let cli = model.cliPath() else { return }
        let res = model.runCommandCapture(
            exe: cli,
            args: ["--json", "secrets", "clear-telegram-mtproto-session"],
            timeoutSeconds: 30
        )
        if res.status == 0 {
            onEndpointTouchedCommitted(endpointId)
            onReload()
        }
    }

    private func testConnection(endpointId: String) {
        guard let cli = model.cliPath() else { return }
        let res = model.runCommandCapture(
            exe: cli,
            args: ["--json", "telegram", "validate", "--endpoint-id", endpointId],
            timeoutSeconds: 180
        )
        if res.status == 0 {
            validateOk = true
            validateText = "Connected"
            onReload()
        } else {
            validateOk = false
            validateText = "Failed (see ui.log)"
        }
    }
}
