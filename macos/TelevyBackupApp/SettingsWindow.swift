import AppKit
import Carbon.HIToolbox
import Darwin
import SwiftUI
import UniformTypeIdentifiers

struct CliSettingsGetResponse: Decodable {
    let settings: SettingsV2
    let secrets: CliSecretsPresence?
    let secretsError: CliSecretsError?
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

struct CliSecretsError: Decodable {
    let code: String
    let message: String
    let retryable: Bool?
}

struct CliSettingsExportBundleResponse: Decodable {
    let bundleKey: String
    let format: String
}

struct CliSettingsImportBundleDryRunResponse: Decodable {
    struct LocalMasterKey: Decodable {
        let state: String
    }

    struct BundleTarget: Decodable, Identifiable {
        let id: String
        let sourcePath: String
        let endpointId: String
        let label: String
    }

    struct BundleEndpoint: Decodable, Identifiable {
        let id: String
        let chatId: String
        let mode: String
    }

    struct SecretsCoverage: Decodable {
        let presentKeys: [String]
        let excludedKeys: [String]
        let missingKeys: [String]
    }

    struct Bundle: Decodable {
        let settingsVersion: Int
        let targets: [BundleTarget]
        let endpoints: [BundleEndpoint]
        let secretsCoverage: SecretsCoverage
    }

    struct Bootstrap: Decodable {
        let state: String
        let details: [String: String]?
    }

    struct RemoteLatest: Decodable {
        let state: String
        let snapshotId: String?
        let manifestObjectId: String?
    }

    struct LocalIndex: Decodable {
        let state: String
        let details: [String: String]?
    }

    struct Conflict: Decodable {
        let state: String
        let reasons: [String]
    }

    struct PreflightTarget: Decodable, Identifiable {
        var id: String { targetId }
        let targetId: String
        let sourcePathExists: Bool
        let bootstrap: Bootstrap
        let remoteLatest: RemoteLatest
        let localIndex: LocalIndex
        let conflict: Conflict
    }

    struct Preflight: Decodable {
        let targets: [PreflightTarget]
    }

    let format: String
    let localMasterKey: LocalMasterKey
    let localHasTargets: Bool
    let nextAction: String
    let bundle: Bundle
    let preflight: Preflight
}

struct CliSettingsImportBundleApplyResponse: Decodable {
    let ok: Bool
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
    case recoveryKey = "Backup Config"
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

private struct EmptyStateView: View {
    let systemImage: String
    let title: String
    var message: String? = nil

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: systemImage)
                .font(.system(size: 36, weight: .semibold))
                .foregroundStyle(.tertiary)

            VStack(spacing: 6) {
                Text(title)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(.secondary)

                if let message {
                    Text(message)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.tertiary)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: 340)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(.vertical, 18)
        .padding(.horizontal, 12)
    }
}

private enum SettingsUIDemo {
    static var enabled: Bool {
        ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_DEMO"] == "1"
    }

    static var scene: String {
        ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_DEMO_SCENE"] ?? ""
    }

    static var disableAutoSelect: Bool {
        enabled && scene.contains("unselected")
    }

    static var initialSection: SettingsSection {
        if enabled && scene.hasPrefix("backup-config") { return .recoveryKey }
        if scene.hasPrefix("endpoints") { return .endpoints }
        return .targets
    }

    static var shouldOpenBackupConfigImportSheet: Bool {
        enabled && scene.hasPrefix("backup-config-import")
    }

    static var shouldOpenBackupConfigExportPanel: Bool {
        enabled && scene == "backup-config-export"
    }

    static func makeSettings(scene: String) -> SettingsV2 {
        let epA = TelegramEndpointV2(
            id: "ep_demo_a",
            mode: "mtproto",
            chat_id: "123456",
            bot_token_key: "telegram.bot_token.ep_demo_a",
            mtproto: TelegramEndpointMtprotoV2(session_key: "telegram.mtproto.session.ep_demo_a"),
            rate_limit: TelegramRateLimitV2(max_concurrent_uploads: 2, min_delay_ms: 250)
        )
        let epB = TelegramEndpointV2(
            id: "ep_demo_b",
            mode: "mtproto",
            chat_id: "987654",
            bot_token_key: "telegram.bot_token.ep_demo_b",
            mtproto: TelegramEndpointMtprotoV2(session_key: "telegram.mtproto.session.ep_demo_b"),
            rate_limit: TelegramRateLimitV2(max_concurrent_uploads: 2, min_delay_ms: 250)
        )

        let targets: [TargetV2] = {
            if scene == "targets-empty" || scene == "endpoints-empty" { return [] }
            return [
                TargetV2(
                    id: "t_demo_a",
                    source_path: "/Users/ivan/Demo/Photos",
                    label: "photos",
                    endpoint_id: epA.id,
                    enabled: true,
                    schedule: nil
                ),
                TargetV2(
                    id: "t_demo_b",
                    source_path: "/Users/ivan/Demo/Documents",
                    label: "docs",
                    endpoint_id: epA.id,
                    enabled: true,
                    schedule: nil
                ),
            ]
        }()

        let endpoints: [TelegramEndpointV2] = {
            if scene == "endpoints-empty" { return [] }
            return [epA, epB]
        }()

        return SettingsV2(
            version: 2,
            schedule: ScheduleV2(enabled: true, kind: "hourly", hourly_minute: 0, daily_at: "02:00", timezone: "UTC"),
            retention: RetentionV2(keep_last_snapshots: 7),
            chunking: ChunkingV2(min_bytes: 1024 * 1024, avg_bytes: 8 * 1024 * 1024, max_bytes: 64 * 1024 * 1024),
            telegram: TelegramGlobalV2(mode: "mtproto", mtproto: TelegramMtprotoGlobalV2(api_id: 0, api_hash_key: "telegram.mtproto.api_hash")),
            telegram_endpoints: endpoints,
            targets: targets
        )
    }
}

struct SettingsWindowRootView: View {
    @EnvironmentObject var model: AppModel
    @State private var section: SettingsSection = .targets

    @State private var settings: SettingsV2?
    @State private var secrets: CliSecretsPresence?
    @State private var secretsError: CliSecretsError?
    @State private var loadError: String?

    @State private var selectedTargetId: String?
    @State private var selectedEndpointId: String?
    @State private var savePending: DispatchWorkItem?
    @State private var isSaving: Bool = false
    @State private var saveSeq: Int = 0
    @State private var reloadSeq: Int = 0

    private struct ImportConfigBundleSheetRequest: Identifiable {
        let id = UUID()
        let fileUrl: URL
    }

    @State private var importConfigBundleSheetRequest: ImportConfigBundleSheetRequest?

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
        .onAppear {
            // Demo mode can force an initial Settings section/sheet to enable deterministic screenshots.
            section = SettingsUIDemo.initialSection
            if SettingsUIDemo.shouldOpenBackupConfigImportSheet {
                if let p = ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_DEMO_IMPORT_FILE"],
                   !p.isEmpty
                {
                    importConfigBundleSheetRequest = ImportConfigBundleSheetRequest(
                        fileUrl: URL(fileURLWithPath: p)
                    )
                }
            }
            if SettingsUIDemo.shouldOpenBackupConfigExportPanel {
                // Let the Settings window finish its first layout before presenting NSSavePanel.
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) {
                    exportBackupConfig()
                }
            }
            reload()
        }
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
            configBundleView
        case .schedule:
            scheduleView
        }
    }

    private var targetsView: some View {
        let targets = settings?.targets ?? []

        return HStack(spacing: 0) {
            VStack(spacing: 0) {
                ScrollViewReader { proxy in
                    List(selection: $selectedTargetId) {
                        ForEach(targets) { t in
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
                .onChange(of: targets.map(\.id)) { _, ids in
                    guard settings != nil else { return }
                    guard !SettingsUIDemo.disableAutoSelect else { return }
                    if let selectedTargetId, ids.contains(selectedTargetId) { return }
                    selectedTargetId = ids.first
                }

                Divider()

                HStack(spacing: 10) {
                    Button { addTarget() } label: {
                        Image(systemName: "plus")
                            .frame(width: 20, height: 20)
                    }
                    .buttonStyle(.bordered)
                    .disabled(settings == nil)

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
                    if settings == nil {
                        EmptyStateView(
                            systemImage: "exclamationmark.triangle",
                            title: "Cannot load settings",
                            message: loadError ?? "Please check CLI configuration and try again."
                        )
                    } else if targets.isEmpty {
                        EmptyStateView(
                            systemImage: "plus.circle",
                            title: "No targets yet",
                            message: "Create your first target with the + button."
                        )
                    } else {
                        EmptyStateView(
                            systemImage: "hand.tap",
                            title: "Select a target"
                        )
                    }
                }
            } label: {
                EmptyView()
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
                    if settings == nil {
                        EmptyStateView(
                            systemImage: "exclamationmark.triangle",
                            title: "Cannot load settings",
                            message: loadError ?? "Please check CLI configuration and try again."
                        )
                    } else if endpoints.isEmpty {
                        EmptyStateView(
                            systemImage: "plus.circle",
                            title: "No endpoints yet",
                            message: "Create your first endpoint with the + button."
                        )
                    } else {
                        EmptyStateView(
                            systemImage: "hand.tap",
                            title: "Select an endpoint"
                        )
                    }
                }
            } label: {
                EmptyView()
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
            if selectedEndpointId == nil {
                guard !SettingsUIDemo.disableAutoSelect else { return }
                selectedEndpointId = endpoints.first?.id
            }
        }
        .onChange(of: endpoints.map(\.id)) { _, ids in
            guard settings != nil else { return }
            guard !SettingsUIDemo.disableAutoSelect else { return }
            if let selectedEndpointId, ids.contains(selectedEndpointId) { return }
            selectedEndpointId = ids.first
        }
    }

    private var configBundleView: some View {
        let secretsUnavailable = secretsError != nil
        let secretsRetryable = secretsError?.retryable ?? true
        let masterKeyPresent = secrets?.masterKeyPresent ?? false

        return VStack(alignment: .leading, spacing: 14) {
            Text("Backup Config")
                .font(.system(size: 18, weight: .bold))

            GroupBox {
                VStack(spacing: 0) {
                    HStack(spacing: 12) {
                        Text("Export backup config")
                            .font(.system(size: 13, weight: .semibold))
                            .frame(width: 160, alignment: .leading)
                        Spacer()
                        Button("Export…") {
                            if secretsUnavailable && secretsRetryable {
                                model.ensureDaemonRunning()
                                reload()
                            }
                            exportBackupConfig()
                        }
                        .buttonStyle(.bordered)
                        .disabled((secretsUnavailable && !secretsRetryable) || (!secretsUnavailable && !masterKeyPresent))
                    }
                    .padding(.vertical, 10)

                    Divider()

                    HStack(spacing: 12) {
                        Text("Import backup config")
                            .font(.system(size: 13, weight: .semibold))
                            .frame(width: 160, alignment: .leading)
                        Spacer()
                        Button("Import…") {
                            if secretsUnavailable && secretsRetryable {
                                model.ensureDaemonRunning()
                                reload()
                            }
                            chooseImportBackupConfigFile()
                        }
                        .buttonStyle(.bordered)
                        .disabled(secretsUnavailable && !secretsRetryable)
                    }
                    .padding(.vertical, 10)
                }
                .padding(.vertical, 2)
                .padding(.horizontal, 12)
            } label: {
                EmptyView()
            }

            Spacer()
        }
        .padding()
        .sheet(item: $importConfigBundleSheetRequest) { req in
            ImportConfigBundleSheet(
                initialFileUrl: req.fileUrl,
                onApplied: { reload() }
            )
            .environmentObject(model)
        }
    }

    private func chooseImportBackupConfigFile() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        let tbconfig = UTType(filenameExtension: "tbconfig")
        panel.allowedContentTypes = [
            tbconfig ?? .data,
            .plainText,
        ]
        panel.prompt = "Choose"

        if panel.runModal() != .OK { return }
        guard let url = panel.url else { return }

        // Bind sheet presentation to the chosen file, so we never end up with a sheet that
        // has no file and asks the user to "choose again".
        importConfigBundleSheetRequest = ImportConfigBundleSheetRequest(fileUrl: url)
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
        if SettingsUIDemo.enabled {
            DispatchQueue.main.async {
                self.secrets = nil
                self.loadError = nil
                self.section = SettingsUIDemo.initialSection
                self.settings = SettingsUIDemo.makeSettings(scene: SettingsUIDemo.scene)

                if !SettingsUIDemo.disableAutoSelect {
                    if self.selectedTargetId == nil {
                        self.selectedTargetId = self.settings?.targets.first?.id
                    }
                    if self.selectedEndpointId == nil {
                        self.selectedEndpointId = self.sortedEndpoints().first?.id
                    }
                } else {
                    if SettingsUIDemo.scene == "targets-unselected" { self.selectedTargetId = nil }
                    if SettingsUIDemo.scene == "endpoints-unselected" { self.selectedEndpointId = nil }
                }
            }
            return
        }

        guard let cli = model.cliPath() else {
            loadError = "televybackup CLI not found (set TELEVYBACKUP_CLI_PATH)"
            return
        }

        reloadSeq += 1
        let seq = reloadSeq

        DispatchQueue.global(qos: .userInitiated).async {
            // `settings get --with-secrets` queries secrets presence via control IPC, which requires
            // the daemon to be running. Start it proactively so the Backup Config actions are not
            // incorrectly disabled on first open.
            model.ensureDaemonRunning()

            let res = model.runCommandCapture(
                exe: cli,
                args: ["--json", "settings", "get", "--with-secrets"],
                timeoutSeconds: 30
            )
            if res.status != 0 {
                DispatchQueue.main.async {
                    guard seq == self.reloadSeq else { return }
                    self.loadError = "settings get failed: exit=\(res.status)"
                }
                return
            }
            guard let data = res.stdout.data(using: .utf8) else {
                DispatchQueue.main.async {
                    guard seq == self.reloadSeq else { return }
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
                    self.loadError = "settings get: JSON decode failed"
                }
                return
            }

            DispatchQueue.main.async {
                guard seq == self.reloadSeq else { return }
                self.settings = decoded.settings
                self.secrets = decoded.secrets
                self.secretsError = decoded.secretsError
                self.loadError = nil
                if !SettingsUIDemo.disableAutoSelect {
                    if let selected = self.selectedTargetId {
                        let ids = Set(decoded.settings.targets.map(\.id))
                        if !ids.contains(selected) {
                            self.selectedTargetId = decoded.settings.targets.first?.id
                        }
                    } else {
                        self.selectedTargetId = decoded.settings.targets.first?.id
                    }
                }
                if let selected = self.selectedEndpointId {
                    let ids = Set(decoded.settings.telegram_endpoints.map(\.id))
                    if !ids.contains(selected) {
                        self.selectedEndpointId = nil
                    }
                }
                if !SettingsUIDemo.disableAutoSelect, self.selectedEndpointId == nil {
                    self.selectedEndpointId = self.sortedEndpoints(settings: decoded.settings).first?.id
                }
            }
        }
    }

    private func showToast(_ text: String, isError: Bool) {
        DispatchQueue.main.async {
            model.toastText = text
            model.toastIsError = isError
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.8) {
            if model.toastText == text {
                model.toastText = nil
            }
        }
    }

    private func exportBackupConfig() {
        model.ensureDaemonRunning()
        guard let cli = model.cliPath() else {
            showToast("televybackup CLI not found", isError: true)
            return
        }

        let panel = NSSavePanel()
        panel.title = "Export backup config"
        panel.prompt = "Export"
        panel.canCreateDirectories = true
        panel.isExtensionHidden = false
        panel.showsTagField = false

        let tbconfig = UTType(filenameExtension: "tbconfig")
        panel.allowedContentTypes = [tbconfig ?? .data]

        let fmt = DateFormatter()
        fmt.locale = Locale(identifier: "en_US_POSIX")
        fmt.dateFormat = "yyyyMMdd-HHmmss"
        let ts = fmt.string(from: Date())
        panel.nameFieldStringValue = "televybackup-backup-config-\(ts).tbconfig"

        let passphraseLabel = NSTextField(labelWithString: "Passphrase / PIN")
        passphraseLabel.font = NSFont.systemFont(ofSize: 11, weight: .semibold)

        let passphraseField = PassphraseSecureTextField()
        passphraseField.placeholderString = "Required"
        passphraseField.font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)

        let messageLabel = NSTextField(labelWithString: "Hint (optional)")
        messageLabel.font = NSFont.systemFont(ofSize: 11, weight: .semibold)

        let messageView = CaretFixedNSTextView()
        messageView.frame = NSRect(x: 0, y: 0, width: 360, height: 88)
        messageView.isRichText = false
        messageView.isEditable = true
        messageView.isSelectable = true
        messageView.drawsBackground = true
        messageView.backgroundColor = .textBackgroundColor
        messageView.textColor = .labelColor
        messageView.insertionPointColor = .labelColor
        messageView.focusRingType = .exterior
        messageView.font = NSFont.systemFont(ofSize: 12)
        messageView.textContainerInset = NSSize(width: 5, height: 6)
        messageView.isHorizontallyResizable = false
        messageView.isVerticallyResizable = true
        messageView.autoresizingMask = [.width]
        messageView.textContainer?.widthTracksTextView = true
        messageView.textContainer?.containerSize = NSSize(
            width: 360,
            height: CGFloat.greatestFiniteMagnitude
        )

        let messageScroll = NSScrollView()
        messageScroll.hasVerticalScroller = true
        messageScroll.borderType = .bezelBorder
        messageScroll.drawsBackground = true
        messageScroll.documentView = messageView

        let accessory = NSStackView()
        accessory.orientation = .vertical
        accessory.spacing = 8
        accessory.alignment = .leading
        // NSSavePanel sizes accessory views based on their frame. Keep this deterministic so
        // the fields don't end up collapsed/hidden.
        accessory.frame = NSRect(x: 0, y: 0, width: 360, height: 0)
        accessory.translatesAutoresizingMaskIntoConstraints = false

        passphraseField.translatesAutoresizingMaskIntoConstraints = false
        messageScroll.translatesAutoresizingMaskIntoConstraints = false

        accessory.addArrangedSubview(passphraseLabel)
        accessory.addArrangedSubview(passphraseField)
        accessory.addArrangedSubview(messageLabel)
        accessory.addArrangedSubview(messageScroll)

        NSLayoutConstraint.activate([
            accessory.widthAnchor.constraint(equalToConstant: 360),
            passphraseField.widthAnchor.constraint(equalTo: accessory.widthAnchor),
            messageScroll.widthAnchor.constraint(equalTo: accessory.widthAnchor),
            messageScroll.heightAnchor.constraint(equalToConstant: 88),
        ])

        accessory.layoutSubtreeIfNeeded()
        accessory.setFrameSize(NSSize(width: 360, height: accessory.fittingSize.height))

        panel.accessoryView = accessory

        let delegate = BackupConfigExportSavePanelDelegate(passphraseField: passphraseField)
        panel.delegate = delegate

        func base64UrlNoPadDecode(_ s: String) -> Data? {
            var t = s.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
            let rem = t.count % 4
            if rem == 2 {
                t.append("==")
            } else if rem == 3 {
                t.append("=")
            } else if rem != 0 {
                return nil
            }
            return Data(base64Encoded: t)
        }

        func handleResult(_ result: NSApplication.ModalResponse) {
            guard result == .OK, let url = panel.url else { return }

            let passphrase = passphraseField.stringValue
            let message = messageView.string.trimmingCharacters(in: .whitespacesAndNewlines)

            DispatchQueue.global(qos: .userInitiated).async {
                var args = ["--json", "settings", "export-bundle"]
                if !message.isEmpty {
                    args.append(contentsOf: ["--hint", message])
                }

                let res = model.runCommandCapture(
                    exe: cli,
                    args: args,
                    timeoutSeconds: 60,
                    env: ["TELEVYBACKUP_CONFIG_BUNDLE_PASSPHRASE": passphrase]
                )
                guard res.status == 0, let data = res.stdout.data(using: .utf8) else {
                    DispatchQueue.main.async {
                        let text = res.stderr.isEmpty ? "Export failed: exit=\(res.status)" : res.stderr
                        showToast(text, isError: true)
                    }
                    return
                }

                guard let decoded = try? JSONDecoder().decode(CliSettingsExportBundleResponse.self, from: data) else {
                    DispatchQueue.main.async { showToast("Export JSON decode failed", isError: true) }
                    return
                }

                let key = decoded.bundleKey.trimmingCharacters(in: .whitespacesAndNewlines)
                guard key.hasPrefix("TBC2:") else {
                    DispatchQueue.main.async { showToast("Export returned invalid bundle", isError: true) }
                    return
                }

                let body = String(key.dropFirst("TBC2:".count))
                guard let outerJson = base64UrlNoPadDecode(body) else {
                    DispatchQueue.main.async { showToast("Export returned invalid bundle", isError: true) }
                    return
                }
                guard
                    let outerObj = try? JSONSerialization.jsonObject(with: outerJson, options: []),
                    PropertyListSerialization.propertyList(outerObj, isValidFor: .binary)
                else {
                    DispatchQueue.main.async { showToast("Export returned invalid bundle", isError: true) }
                    return
                }

                let plist: Data
                do {
                    plist = try PropertyListSerialization.data(
                        fromPropertyList: outerObj,
                        format: .binary,
                        options: 0
                    )
                } catch {
                    DispatchQueue.main.async { showToast("Failed to encode export file", isError: true) }
                    return
                }

                do {
                    try plist.write(to: url, options: [.atomic])
                } catch {
                    DispatchQueue.main.async { showToast("Failed to write export file", isError: true) }
                    return
                }

                DispatchQueue.main.async {
                    showToast("Saved backup config", isError: false)
                }
            }
        }

        let hostWindow =
            NSApp.keyWindow ??
            NSApp.mainWindow ??
            NSApp.windows.first(where: { $0.title == "Settings" })
        if let hostWindow {
            panel.beginSheetModal(for: hostWindow, completionHandler: handleResult)
        } else {
            handleResult(panel.runModal())
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
        selectedEndpointId = sortedEndpoints(settings: s).first?.id
        queueAutoSave()
    }
}

private final class BackupConfigExportSavePanelDelegate: NSObject, NSOpenSavePanelDelegate {
    private enum ValidationError: LocalizedError {
        case passphraseRequired
        case extensionRequired

        var errorDescription: String? {
            switch self {
            case .passphraseRequired:
                return "Passphrase is required."
            case .extensionRequired:
                return "File extension must be .tbconfig."
            }
        }

        var recoverySuggestion: String? {
            switch self {
            case .passphraseRequired:
                return "Enter a passphrase / PIN, then try again."
            case .extensionRequired:
                return "Use a file name that ends with .tbconfig."
            }
        }
    }

    private let passphraseField: NSSecureTextField

    init(passphraseField: NSSecureTextField) {
        self.passphraseField = passphraseField
    }

    func panel(_ sender: Any, validate url: URL) throws {
        if passphraseField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            throw ValidationError.passphraseRequired
        }
        if url.pathExtension.lowercased() != "tbconfig" {
            throw ValidationError.extensionRequired
        }
    }
}

// Password/PIN fields should avoid IME candidate windows and use secure keyboard input while focused.
// - `allowedInputSourceLocales` restricts active keyboard input sources for this field's input context.
// - `EnableSecureEventInput` prevents other processes from observing keystrokes while the user is
//   entering sensitive data (must be balanced with DisableSecureEventInput).
private final class PassphraseSecureTextField: NSSecureTextField {
    private weak var forcedInputContext: NSTextInputContext?
    private var previousAllowedInputSourceLocales: [String]?
    private var didForceAllowedLocales: Bool = false

    private var didEnableSecureEventInput: Bool = false

    private var previousKeyboardInputSource: TISInputSource?
    private var forcedKeyboardInputSource: TISInputSource?
    private var didForceKeyboardInputSource: Bool = false

    override func becomeFirstResponder() -> Bool {
        let ok = super.becomeFirstResponder()
        if ok {
            enableSecureInputIfNeeded()
            forceASCIICapableKeyboardInputSourceIfNeeded()
            // Field editor may not be ready immediately; apply twice defensively.
            applyInputMethodRestrictions()
            DispatchQueue.main.async { [weak self] in
                self?.applyInputMethodRestrictions()
            }
        }
        return ok
    }

    override func resignFirstResponder() -> Bool {
        let ok = super.resignFirstResponder()
        if ok {
            restoreInputMethodRestrictions()
            restoreKeyboardInputSourceIfNeeded()
            disableSecureInputIfNeeded()
        }
        return ok
    }

    deinit {
        restoreInputMethodRestrictions()
        restoreKeyboardInputSourceIfNeeded()
        disableSecureInputIfNeeded()
    }

    private func enableSecureInputIfNeeded() {
        guard !didEnableSecureEventInput else { return }
        // `EnableSecureEventInput` is ref-counted; we track our own balance to avoid leaks.
        if EnableSecureEventInput() == noErr {
            didEnableSecureEventInput = true
        }
    }

    private func disableSecureInputIfNeeded() {
        guard didEnableSecureEventInput else { return }
        DisableSecureEventInput()
        didEnableSecureEventInput = false
    }

    private func forceASCIICapableKeyboardInputSourceIfNeeded() {
        guard !didForceKeyboardInputSource else { return }
        let current = TISCopyCurrentKeyboardInputSource().takeRetainedValue()
        let ascii = TISCopyCurrentASCIICapableKeyboardInputSource().takeRetainedValue()
        // If the user is already on an ASCII-capable layout, don't disturb their input source.
        if CFEqual(current, ascii) { return }

        let err = TISSelectInputSource(ascii)
        guard err == noErr else { return }

        previousKeyboardInputSource = current
        forcedKeyboardInputSource = ascii
        didForceKeyboardInputSource = true
    }

    private func restoreKeyboardInputSourceIfNeeded() {
        guard didForceKeyboardInputSource else { return }
        defer {
            didForceKeyboardInputSource = false
            previousKeyboardInputSource = nil
            forcedKeyboardInputSource = nil
        }

        guard let previous = previousKeyboardInputSource, let forced = forcedKeyboardInputSource else { return }

        // If the user manually changed the input source while focused, don't override their choice.
        let current = TISCopyCurrentKeyboardInputSource().takeRetainedValue()
        guard CFEqual(current, forced) else { return }

        _ = TISSelectInputSource(previous)
    }

    private func applyInputMethodRestrictions() {
        // Prefer the field editor's input context when present.
        let ctx = (currentEditor() as? NSTextView)?.inputContext
            ?? inputContext
            ?? NSTextInputContext.current
        guard let ctx else { return }

        // Text fields often swap the active input context once the field editor is created.
        // If that happens, move the restriction to the new context.
        if didForceAllowedLocales, forcedInputContext === ctx {
            return
        }
        if didForceAllowedLocales, forcedInputContext !== ctx {
            restoreInputMethodRestrictions()
        }

        forcedInputContext = ctx
        previousAllowedInputSourceLocales = ctx.allowedInputSourceLocales
        // Force a Latin/English input source so IME candidate windows don't show up for passwords.
        // (Locale identifiers are BCP-47-ish; "en_US" is a safe default.)
        ctx.allowedInputSourceLocales = ["en_US"]
        didForceAllowedLocales = true
    }

    private func restoreInputMethodRestrictions() {
        guard didForceAllowedLocales else { return }
        defer {
            didForceAllowedLocales = false
            forcedInputContext = nil
            previousAllowedInputSourceLocales = nil
        }

        guard let ctx = forcedInputContext else { return }
        ctx.allowedInputSourceLocales = previousAllowedInputSourceLocales
    }
}

private final class CaretFixedNSTextView: NSTextView {
    override func becomeFirstResponder() -> Bool {
        let ok = super.becomeFirstResponder()
        if ok && string.isEmpty {
            // Workaround: in some contexts (e.g. NSSavePanel accessory views), an empty NSTextView
            // may accept input but not show the insertion point until after the first character.
            string = ""
            setSelectedRange(NSRange(location: 0, length: 0))
            needsDisplay = true
        }
        return ok
    }
}

private struct WindowAccessor: NSViewRepresentable {
    let onResolve: (NSWindow) -> Void

    func makeNSView(context: Context) -> NSView {
        let v = NSView()
        DispatchQueue.main.async {
            if let w = v.window {
                onResolve(w)
            }
        }
        return v
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        DispatchQueue.main.async {
            if let w = nsView.window {
                onResolve(w)
            }
        }
    }
}

private struct SecurePassphraseField: NSViewRepresentable {
    @Binding var text: String
    @Binding var isFocused: Bool
    let placeholder: String
    let onSubmit: () -> Void

    final class Coordinator: NSObject, NSTextFieldDelegate {
        @Binding var text: String
        @Binding var isFocused: Bool
        let onSubmit: () -> Void

        init(text: Binding<String>, isFocused: Binding<Bool>, onSubmit: @escaping () -> Void) {
            _text = text
            _isFocused = isFocused
            self.onSubmit = onSubmit
        }

        func controlTextDidBeginEditing(_ obj: Notification) {
            isFocused = true
        }

        func controlTextDidEndEditing(_ obj: Notification) {
            isFocused = false
        }

        func controlTextDidChange(_ obj: Notification) {
            guard let field = obj.object as? NSTextField else { return }
            text = field.stringValue
        }

        @objc func onAction(_ sender: Any?) {
            onSubmit()
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(text: $text, isFocused: $isFocused, onSubmit: onSubmit)
    }

    func makeNSView(context: Context) -> PassphraseSecureTextField {
        let field = PassphraseSecureTextField()
        field.delegate = context.coordinator
        field.target = context.coordinator
        field.action = #selector(Coordinator.onAction(_:))
        field.placeholderString = placeholder
        field.font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
        field.isBezeled = true
        field.bezelStyle = .roundedBezel
        field.isEditable = true
        field.isSelectable = true
        field.drawsBackground = true
        field.backgroundColor = .textBackgroundColor
        field.textColor = .labelColor
        return field
    }

    func updateNSView(_ nsView: PassphraseSecureTextField, context: Context) {
        if nsView.stringValue != text {
            nsView.stringValue = text
        }

        // For NSTextField/NSSecureTextField, once editing begins the window's firstResponder is the
        // shared field editor (NSTextView), not the text field itself. Only request focus when we
        // are not already editing; otherwise we can disrupt typing (e.g. only the first character).
        if isFocused, nsView.currentEditor() == nil {
            DispatchQueue.main.async {
                nsView.window?.makeFirstResponder(nsView)
            }
        }
    }
}

private struct ImportConfigBundleSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject var model: AppModel

    let initialFileUrl: URL?
    let onApplied: () -> Void

    @State private var passphraseFocused: Bool = false

    @State private var fileEncrypted: Bool = true
    @State private var fileUrl: URL?
    @State private var bundleKey: String = ""
    @State private var hintPreview: String?
    @State private var passphrase: String = ""
    @State private var inspecting: Bool = false
    @State private var inspection: CliSettingsImportBundleDryRunResponse?
    @State private var inspectError: String?
    @State private var didAutoSnapshot: Bool = false
    @State private var didLoadInitialFile: Bool = false
    @State private var sheetWindow: NSWindow?
    @State private var contentHeight: CGFloat = 0
    @State private var targetsContentHeight: CGFloat = 0

    @State private var selectedTargetIds: Set<String> = []
    @State private var resolutions: [String: ResolutionState] = [:]

    @State private var applying: Bool = false
    @State private var applyError: String?

    private struct ConfigBundleOuterPreview: Decodable {
        let hint: String?
    }

    private func base64UrlNoPadDecode(_ s: String) -> Data? {
        var t = s.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        let rem = t.count % 4
        if rem == 2 {
            t.append("==")
        } else if rem == 3 {
            t.append("=")
        } else if rem != 0 {
            return nil
        }
        return Data(base64Encoded: t)
    }

    private func base64UrlNoPadEncode(_ data: Data) -> String {
        data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    private func hintFromBundleKey(_ key: String) -> String? {
        let trimmed = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("TBC2:") else { return nil }
        let body = String(trimmed.dropFirst("TBC2:".count))
        guard let data = base64UrlNoPadDecode(body) else { return nil }
        guard let preview = try? JSONDecoder().decode(ConfigBundleOuterPreview.self, from: data) else { return nil }
        return preview.hint
    }

    private func readFirstNonEmptyLine(url: URL) -> String? {
        guard let content = try? String(contentsOf: url, encoding: .utf8) else { return nil }
        let line = content
            .split(whereSeparator: \.isNewline)
            .map { String($0).trimmingCharacters(in: .whitespacesAndNewlines) }
            .first(where: { !$0.isEmpty })
        return line
    }

    private func normalizeBundleKey(_ raw: String) -> String? {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { return nil }
        if trimmed.hasPrefix("TBC2:") { return trimmed }
        return "TBC2:" + trimmed
    }

    init(initialFileUrl: URL? = nil, onApplied: @escaping () -> Void) {
        self.initialFileUrl = initialFileUrl
        self.onApplied = onApplied
        _fileUrl = State(initialValue: initialFileUrl)
    }

    private func loadSelectedFile(url: URL) {
        fileUrl = url
        inspection = nil
        applyError = nil
        passphrase = ""
        hintPreview = nil
        bundleKey = ""
        inspectError = nil

        if let data = try? Data(contentsOf: url),
           let plist = try? PropertyListSerialization.propertyList(from: data, options: [], format: nil),
           let dict = plist as? [String: Any],
           PropertyListSerialization.propertyList(dict, isValidFor: .binary),
           let json = try? JSONSerialization.data(withJSONObject: dict, options: [])
        {
            hintPreview = dict["hint"] as? String
            fileEncrypted = (dict["payloadEnc"] != nil) || (dict["goldKeyEnc"] != nil)
            bundleKey = "TBC2:" + base64UrlNoPadEncode(json)
            inspectError = nil
            if fileEncrypted {
                DispatchQueue.main.async { passphraseFocused = true }
            }
            return
        }

        if let raw = readFirstNonEmptyLine(url: url), let key = normalizeBundleKey(raw) {
            bundleKey = key
            hintPreview = hintFromBundleKey(key)
            fileEncrypted = true
            inspectError = nil
            DispatchQueue.main.async { passphraseFocused = true }
            return
        }

        bundleKey = ""
        hintPreview = nil
        inspectError = "Invalid backup config file"
    }

    private struct ResolutionState {
        var mode: ResolutionMode
        var newSourcePath: String
    }

    private enum ResolutionMode: String, CaseIterable, Identifiable {
        case overwrite_local
        case overwrite_remote
        case rebind
        case skip

        var id: String { rawValue }

        var title: String {
            switch self {
            case .overwrite_local: return "Use remote latest (replace local)"
            case .overwrite_remote: return "Use local (update remote pin)"
            case .rebind: return "Choose a different folder"
            case .skip: return "Skip this target"
            }
        }
    }

    private var preflightByTargetId: [String: CliSettingsImportBundleDryRunResponse.PreflightTarget] {
        guard let inspection else { return [:] }
        return Dictionary(uniqueKeysWithValues: inspection.preflight.targets.map { ($0.targetId, $0) })
    }

    private func needsResolution(targetId: String) -> Bool {
        guard let pf = preflightByTargetId[targetId] else { return false }
        return pf.conflict.state == "needs_resolution"
    }

    private func resolveState(targetId: String) -> ResolutionState {
        if let v = resolutions[targetId] { return v }
        return ResolutionState(mode: .overwrite_local, newSourcePath: "")
    }

    private func setResolveState(targetId: String, _ state: ResolutionState) {
        resolutions[targetId] = state
    }

    private func targetToggleBinding(id: String) -> Binding<Bool> {
        Binding(
            get: { selectedTargetIds.contains(id) },
            set: { on in
                if on {
                    selectedTargetIds.insert(id)
                } else {
                    selectedTargetIds.remove(id)
                }
            }
        )
    }

    private func chooseFolder(targetId: String) {
        let panel = NSOpenPanel()
        panel.title = "Choose folder"
        panel.prompt = "Choose"
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false

        func applyUrl(_ url: URL) {
            let current = resolveState(targetId: targetId)
            setResolveState(
                targetId: targetId,
                ResolutionState(mode: current.mode, newSourcePath: url.path)
            )
        }

        if let hostWindow = NSApp.keyWindow ?? NSApp.mainWindow {
            panel.beginSheetModal(for: hostWindow) { res in
                guard res == .OK, let url = panel.url else { return }
                applyUrl(url)
            }
        } else {
            let res = panel.runModal()
            guard res == .OK, let url = panel.url else { return }
            applyUrl(url)
        }
    }

    private func applyDefaults(from inspection: CliSettingsImportBundleDryRunResponse) {
        selectedTargetIds = Set(inspection.bundle.targets.map(\.id))

        var next: [String: ResolutionState] = [:]
        for pf in inspection.preflight.targets {
            guard pf.conflict.state == "needs_resolution" else { continue }
            if pf.conflict.reasons.contains("bootstrap_invalid") {
                next[pf.targetId] = ResolutionState(mode: .skip, newSourcePath: "")
            } else if pf.conflict.reasons.contains("missing_path") {
                next[pf.targetId] = ResolutionState(mode: .rebind, newSourcePath: "")
            } else if pf.conflict.reasons.contains("local_vs_remote_mismatch") {
                next[pf.targetId] = ResolutionState(mode: .overwrite_local, newSourcePath: "")
            } else {
                next[pf.targetId] = ResolutionState(mode: .overwrite_local, newSourcePath: "")
            }
        }
        resolutions = next
    }

    private func inspectBundle() {
        let key = bundleKey.trimmingCharacters(in: .whitespacesAndNewlines)
        if key.isEmpty { return }
        if fileEncrypted && passphrase.isEmpty {
            inspectError = "Passphrase is required"
            return
        }

        inspecting = true
        inspectError = nil
        applyError = nil
        inspection = nil

        model.ensureDaemonRunning()
        guard let cli = model.cliPath() else {
            inspectError = "televybackup CLI not found"
            inspecting = false
            return
        }

        var env: [String: String] = [:]
        if fileEncrypted {
            env["TELEVYBACKUP_CONFIG_BUNDLE_PASSPHRASE"] = passphrase
        }
        let res = model.runCommandCapture(
            exe: cli,
            args: ["--json", "settings", "import-bundle", "--dry-run"],
            stdin: key + "\n",
            timeoutSeconds: 120,
            env: env
        )
        guard res.status == 0, let data = res.stdout.data(using: .utf8) else {
            inspectError = res.stderr.isEmpty ? "Inspect failed: exit=\(res.status)" : res.stderr
            inspecting = false
            return
        }

        do {
            let decoded = try JSONDecoder().decode(CliSettingsImportBundleDryRunResponse.self, from: data)
            inspection = decoded
            applyDefaults(from: decoded)
        } catch {
            inspectError = "Inspect JSON decode failed"
        }

        inspecting = false
    }

    private func canApply() -> Bool {
        guard let inspection else { return false }
        if inspection.nextAction == "start_key_rotation" { return false }
        if selectedTargetIds.isEmpty { return false }
        if fileEncrypted && passphrase.isEmpty { return false }

        for id in selectedTargetIds {
            if needsResolution(targetId: id) {
                guard let pf = preflightByTargetId[id] else { return false }
                let s = resolveState(targetId: id)

                if pf.conflict.reasons.contains("bootstrap_invalid") && s.mode != .skip {
                    return false
                }

                if pf.conflict.reasons.contains("missing_path") {
                    if s.mode == .rebind {
                        if s.newSourcePath.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                            return false
                        }
                    } else if s.mode == .skip {
                        // ok
                    } else {
                        return false
                    }
                }
            }
        }
        return true
    }

    private func applyBundle() {
        guard canApply() else { return }
        guard let cli = model.cliPath() else { return }

        applying = true
        applyError = nil

        let key = bundleKey.trimmingCharacters(in: .whitespacesAndNewlines)
        let selected = Array(selectedTargetIds).sorted()

        var resolutionsObj: [String: Any] = [:]
        for id in selected {
            guard needsResolution(targetId: id) else { continue }
            let state = resolveState(targetId: id)
            var obj: [String: Any] = ["mode": state.mode.rawValue]
            if state.mode == .rebind {
                obj["newSourcePath"] = state.newSourcePath.trimmingCharacters(in: .whitespacesAndNewlines)
            }
            resolutionsObj[id] = obj
        }

        let payload: [String: Any] = [
            "bundleKey": key,
            "selectedTargetIds": selected,
            "confirm": [
                // UI already gates apply behind an explicit click; avoid a second typed confirmation.
                "phrase": "IMPORT",
            ],
            "resolutions": resolutionsObj,
        ]

        guard let data = try? JSONSerialization.data(withJSONObject: payload, options: []),
              let stdin = String(data: data, encoding: .utf8)
        else {
            applyError = "Failed to encode apply JSON"
            applying = false
            return
        }

        model.ensureDaemonRunning()
        let res = model.runCommandCapture(
            exe: cli,
            args: ["--json", "settings", "import-bundle", "--apply"],
            stdin: stdin + "\n",
            timeoutSeconds: 180,
            env: ["TELEVYBACKUP_CONFIG_BUNDLE_PASSPHRASE": passphrase]
        )

        guard res.status == 0, let outData = res.stdout.data(using: .utf8) else {
            applyError = res.stderr.isEmpty ? "Apply failed: exit=\(res.status)" : res.stderr
            applying = false
            return
        }

        do {
            let decoded = try JSONDecoder().decode(CliSettingsImportBundleApplyResponse.self, from: outData)
            if decoded.ok {
                onApplied()
                dismiss()
            } else {
                applyError = "Apply failed"
            }
        } catch {
            applyError = "Apply JSON decode failed"
        }

        applying = false
    }

    private func missingSecretLabels(keys: [String]) -> [String] {
        var out: [String] = []
        for k in keys {
            if k.contains("telegram.bot_token") {
                if !out.contains("Telegram bot token") { out.append("Telegram bot token") }
            } else if k.contains("telegram.mtproto.api_hash") || k.contains("telegram.api_hash") {
                if !out.contains("Telegram API hash") { out.append("Telegram API hash") }
            } else {
                if !out.contains("Other secrets") { out.append("Other secrets") }
            }
        }
        return out
    }

    private func conflictReasonLines(_ reasons: [String]) -> [String] {
        var out: [String] = []
        for r in reasons {
            switch r {
            case "missing_path":
                out.append("Folder not found on this Mac.")
            case "bootstrap_invalid":
                out.append("Remote bootstrap cannot be decrypted (wrong key or corrupted).")
            case "local_vs_remote_mismatch":
                out.append("Local index does not match the remote latest.")
            default:
                out.append("Needs review (\(r)).")
            }
        }
        return out
    }

    private func demoInspection(targetsCount: Int) -> CliSettingsImportBundleDryRunResponse {
        let n = max(1, targetsCount)
        let includeConflicts = ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_DEMO_IMPORT_CONFLICTS"] == "1"
        let epA = CliSettingsImportBundleDryRunResponse.BundleEndpoint(
            id: "ep_demo_a",
            chatId: "123456",
            mode: "mtproto"
        )

        let targets: [CliSettingsImportBundleDryRunResponse.BundleTarget] = (0..<n).map { i in
            let id = String(format: "t_demo_%02d", i + 1)
            return CliSettingsImportBundleDryRunResponse.BundleTarget(
                id: id,
                sourcePath: "/Users/ivan/Demo/Folder\(i + 1)",
                endpointId: epA.id,
                label: i == 0 ? "photos" : "target-\(i + 1)"
            )
        }

        let preflightTargets: [CliSettingsImportBundleDryRunResponse.PreflightTarget] = targets.enumerated().map { i, t in
            var sourcePathExists = true
            var bootstrap = CliSettingsImportBundleDryRunResponse.Bootstrap(state: "ok", details: nil)
            let remoteLatest = CliSettingsImportBundleDryRunResponse.RemoteLatest(
                state: "ok",
                snapshotId: "s_demo_latest",
                manifestObjectId: "m_demo_latest"
            )
            var localIndex = CliSettingsImportBundleDryRunResponse.LocalIndex(state: "match", details: nil)
            var reasons: [String] = []

            if includeConflicts {
                // Cover the common cases we need to design around.
                if i == 0 {
                    sourcePathExists = false
                    reasons = ["missing_path"]
                } else if i == 1 {
                    bootstrap = CliSettingsImportBundleDryRunResponse.Bootstrap(
                        state: "invalid",
                        details: ["error": "decrypt failed"]
                    )
                    reasons = ["bootstrap_invalid"]
                } else if i == 2 {
                    localIndex = CliSettingsImportBundleDryRunResponse.LocalIndex(state: "stale", details: nil)
                    reasons = ["local_vs_remote_mismatch"]
                }
            }

            let conflictState = reasons.isEmpty ? "none" : "needs_resolution"

            return CliSettingsImportBundleDryRunResponse.PreflightTarget(
                targetId: t.id,
                sourcePathExists: sourcePathExists,
                bootstrap: bootstrap,
                remoteLatest: remoteLatest,
                localIndex: localIndex,
                conflict: CliSettingsImportBundleDryRunResponse.Conflict(state: conflictState, reasons: reasons)
            )
        }

        let secretsCoverage = CliSettingsImportBundleDryRunResponse.SecretsCoverage(
            presentKeys: [],
            excludedKeys: [],
            missingKeys: []
        )

        return CliSettingsImportBundleDryRunResponse(
            format: "tbc2",
            localMasterKey: CliSettingsImportBundleDryRunResponse.LocalMasterKey(state: "match"),
            localHasTargets: true,
            nextAction: "apply",
            bundle: CliSettingsImportBundleDryRunResponse.Bundle(
                settingsVersion: 2,
                targets: targets,
                endpoints: [epA],
                secretsCoverage: secretsCoverage
            ),
            preflight: CliSettingsImportBundleDryRunResponse.Preflight(targets: preflightTargets)
        )
    }

    private struct SheetContentHeightPreferenceKey: PreferenceKey {
        static var defaultValue: CGFloat = 0

        static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
            value = max(value, nextValue())
        }
    }

    private struct TargetsContentHeightPreferenceKey: PreferenceKey {
        static var defaultValue: CGFloat = 0

        static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
            value = max(value, nextValue())
        }
    }

    // Prefer sizing the sheet to its content; fixed heights create large empty regions
    // in the pre-inspect (password entry) stage.
    private var sheetWidth: CGFloat { 720 }

    private func resizeSheet(to height: CGFloat, animated: Bool) {
        guard let sheetWindow else { return }
        let clamped = min(900, max(220, height))
        // Add a tiny safety pad so we don't end up clipping the last pixel row due to rounding.
        let padded = ceil(clamped + 2)
        let size = NSSize(width: sheetWidth, height: padded)
        DispatchQueue.main.async {
            if animated {
                sheetWindow.animator().setContentSize(size)
            } else {
                sheetWindow.setContentSize(size)
            }
        }
    }

    private var targetsListHeightFallback: CGFloat {
        guard let inspection else { return 0 }
        let count = inspection.bundle.targets.count
        let rowApprox: CGFloat = 72
        let listMax: CGFloat = 340
        // We want the sheet to shrink-wrap when there are only a few items.
        // Keep a tiny floor to avoid a zero-height ScrollView while still allowing
        // the sheet to collapse tightly around content.
        let listMin: CGFloat = 1
        return min(listMax, max(listMin, CGFloat(max(count, 1)) * rowApprox))
    }

    private var targetsScrollHeight: CGFloat {
        let maxHeight: CGFloat = 340
        let measured = targetsContentHeight
        let h = measured > 1 ? measured : targetsListHeightFallback
        return min(maxHeight, max(1, ceil(h)))
    }

	    private var shouldShowHeader: Bool { inspection != nil || fileUrl != nil }

        @ViewBuilder
        private var headerView: some View {
            VStack(alignment: .leading, spacing: 6) {
                Text("Import backup config")
                    .font(.system(size: 18, weight: .bold))
                Text("Choose a backup config file, then inspect before apply.")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.secondary)
            }
        }

        @ViewBuilder
        private var missingFileView: some View {
            VStack(spacing: 16) {
                EmptyStateView(
                    systemImage: "tray.and.arrow.down",
                    title: "Import backup config",
                    message: "Close this window and choose a file again."
                )
                .frame(maxWidth: .infinity)
                HStack(spacing: 8) {
                    Button("Cancel") { dismiss() }
                        .keyboardShortcut(.cancelAction)
                    Spacer()
                }
            }
            .frame(maxWidth: .infinity)
        }

        @ViewBuilder
        private func preInspectView(fileUrl: URL) -> some View {
            GroupBox {
                VStack(alignment: .leading, spacing: 12) {
                    HStack(spacing: 8) {
                        Image(systemName: "doc")
                            .foregroundStyle(.secondary)
                        Text(fileUrl.lastPathComponent)
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Spacer()
                    }

                    let h = hintPreview?.trimmingCharacters(in: .whitespacesAndNewlines)
                    VStack(alignment: .leading, spacing: 6) {
                        Text("Hint")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(.secondary)
                        Text(h?.isEmpty == false ? h! : "—")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .fixedSize(horizontal: false, vertical: true)
                    }

                    if fileEncrypted {
                        VStack(alignment: .leading, spacing: 6) {
                            Text("Passphrase / PIN")
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(.secondary)
                            SecurePassphraseField(
                                text: $passphrase,
                                isFocused: $passphraseFocused,
                                placeholder: "",
                                onSubmit: { inspectBundle() }
                            )
                        }
                    }

                    if let inspectError {
                        Label(inspectError, systemImage: "exclamationmark.triangle.fill")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.red)
                    }
                }
                .padding(.vertical, 2)
            }

            HStack(spacing: 8) {
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Spacer()
                Button(inspecting ? "Inspecting…" : "Inspect") { inspectBundle() }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(
                        (fileEncrypted && passphrase.isEmpty)
                            || bundleKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                            || inspecting
                    )
            }
        }

        @ViewBuilder
        private func resultView(inspection: CliSettingsImportBundleDryRunResponse) -> some View {
            let targetsCount = inspection.bundle.targets.count
            let endpointsCount = inspection.bundle.endpoints.count
            let missingKeys = inspection.bundle.secretsCoverage.missingKeys
            let missingLabels = missingSecretLabels(keys: missingKeys)
            let canImport = inspection.nextAction != "start_key_rotation"

            let primaryStatusText: String = {
                if canImport { return "Ready to import" }
                return "Import blocked"
            }()

            let statusDetailsText: String = {
                if canImport {
                    if inspection.localHasTargets {
                        return "This Mac already has backup config."
                    }
                    return "No existing backup config detected on this Mac."
                }
                return "This Mac is using a different encryption key. Import is disabled to prevent mixing unrelated backups."
            }()

            let secretsText: String = {
                if missingKeys.isEmpty { return "Credentials: complete" }
                if missingLabels.isEmpty { return "Credentials: missing" }
                return "Credentials missing: \(missingLabels.joined(separator: ", "))"
            }()

            GroupBox {
                VStack(alignment: .leading, spacing: 10) {
                    Text("Summary")
                        .font(.system(size: 13, weight: .semibold))
                    HStack(spacing: 10) {
                        Image(systemName: canImport ? "checkmark.seal.fill" : "exclamationmark.triangle.fill")
                            .foregroundStyle(canImport ? Color.green : Color.red)
                        Text(primaryStatusText)
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.primary)
                        Spacer()
                        Text("\(targetsCount) target\(targetsCount == 1 ? "" : "s") · \(endpointsCount) endpoint\(endpointsCount == 1 ? "" : "s")")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.secondary)
                    }

                    Text(statusDetailsText)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.secondary)

                    Text(secretsText)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(missingKeys.isEmpty ? Color.secondary : Color.red)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Text("Targets")
                .font(.system(size: 13, weight: .semibold))

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    ForEach(inspection.bundle.targets.sorted { a, b in
                        a.id.localizedStandardCompare(b.id) == .orderedAscending
                    }) { t in
                        let pf = preflightByTargetId[t.id]

                        VStack(alignment: .leading, spacing: 6) {
                            Toggle(isOn: targetToggleBinding(id: t.id)) {
                                HStack(spacing: 10) {
                                    Text(t.id)
                                        .font(.system(size: 12, design: .monospaced))
                                    Text(t.label)
                                        .font(.system(size: 12, weight: .semibold))
                                        .foregroundStyle(.secondary)
                                    Spacer()
                                    if let pf {
                                        Text(pf.conflict.state == "needs_resolution" ? "Needs action" : "OK")
                                            .font(.system(size: 12, weight: .semibold))
                                            .foregroundStyle(pf.conflict.state == "needs_resolution" ? .red : .secondary)
                                    }
                                }
                            }

                            Text(t.sourcePath)
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                                .truncationMode(.middle)

                            if let pf, pf.conflict.state == "needs_resolution", selectedTargetIds.contains(t.id) {
                                let state = resolveState(targetId: t.id)

                                HStack(spacing: 10) {
                                    Text("Action")
                                        .font(.system(size: 12, weight: .semibold))
                                        .frame(width: 80, alignment: .leading)

                                    Picker("", selection: Binding(
                                        get: { state.mode },
                                        set: { newMode in
                                            setResolveState(targetId: t.id, ResolutionState(mode: newMode, newSourcePath: state.newSourcePath))
                                        }
                                    )) {
                                        ForEach(ResolutionMode.allCases) { m in
                                            Text(m.title).tag(m)
                                        }
                                    }
                                    .pickerStyle(.menu)
                                    .controlSize(.regular)
                                    .frame(width: 260)

                                    if state.mode == .rebind {
                                        HStack(spacing: 8) {
                                            TextField(
                                                "",
                                                text: Binding(
                                                    get: { state.newSourcePath },
                                                    set: { _ in }
                                                ),
                                                prompt: Text("No folder selected")
                                            )
                                            .textFieldStyle(.roundedBorder)
                                            .font(.system(size: 12, design: .monospaced))
                                            .controlSize(.regular)
                                            .disabled(true)

                                            Button("Choose…") { chooseFolder(targetId: t.id) }
                                                .buttonStyle(.bordered)
                                                .controlSize(.regular)
                                        }
                                        .frame(maxWidth: 360)
                                    }
                                }

                                VStack(alignment: .leading, spacing: 3) {
                                    Text("Why it needs action")
                                        .font(.system(size: 11, weight: .semibold))
                                        .foregroundStyle(.secondary)
                                    ForEach(conflictReasonLines(pf.conflict.reasons), id: \.self) { line in
                                        Text("• \(line)")
                                            .font(.system(size: 11, weight: .semibold))
                                            .foregroundStyle(.secondary)
                                    }
                                }
                            }
                        }
                        .padding(12)
                        .background(.background)
                        .clipShape(RoundedRectangle(cornerRadius: 10))
                        .overlay(RoundedRectangle(cornerRadius: 10).stroke(.quaternary))
                    }
                }
                .background(
                    GeometryReader { proxy in
                        Color.clear.preference(
                            key: TargetsContentHeightPreferenceKey.self,
                            value: proxy.size.height
                        )
                    }
                )
            }
            .scrollIndicators(.visible)
            // Fit the scroll area to the actual list content to avoid blank space.
            .frame(height: targetsScrollHeight)
            .onPreferenceChange(TargetsContentHeightPreferenceKey.self) { h in
                // Avoid thrashing on minuscule changes due to text rendering/rounding.
                guard h > 1 else { return }
                if abs(h - targetsContentHeight) < 0.5 { return }
                targetsContentHeight = h
            }

            Divider()

            if let applyError {
                Text(applyError)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.red)
            }

            HStack {
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Spacer()
                Button(applying ? "Applying…" : "Apply") { applyBundle() }
                    .buttonStyle(.borderedProminent)
                    .disabled(inspection.nextAction == "start_key_rotation" || applying || !canApply())
            }
        }

	        var body: some View {
	        VStack(alignment: .leading, spacing: 12) {
	            if shouldShowHeader {
                    headerView
	            }

	            if let inspection {
                    resultView(inspection: inspection)
	            } else if let fileUrl {
                    preInspectView(fileUrl: fileUrl)
                } else {
                    missingFileView
                }
	        }
	        .padding(18)
	        .frame(width: sheetWidth, alignment: .topLeading)
            .fixedSize(horizontal: false, vertical: true)
            .animation(.easeInOut(duration: 0.15), value: inspection != nil)
            .background(
                WindowAccessor { w in
                    if sheetWindow !== w {
                        sheetWindow = w
                        if contentHeight > 1 {
                            resizeSheet(to: contentHeight, animated: false)
                        }
                    }
                }
            )
            .background(
                GeometryReader { proxy in
                    Color.clear.preference(key: SheetContentHeightPreferenceKey.self, value: proxy.size.height)
                }
            )
            .onPreferenceChange(SheetContentHeightPreferenceKey.self) { h in
                guard h > 1 else { return }
                if abs(h - contentHeight) < 0.5 { return }
                contentHeight = h
                resizeSheet(to: h, animated: true)
            }
	        .onChange(of: inspection != nil) { _, ready in
	            guard ready else { return }
	            guard SettingsUIDemo.enabled else { return }
	            guard SettingsUIDemo.scene == "backup-config-import-result" else { return }
	            guard !didAutoSnapshot else { return }

            guard let dirPath = ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_SNAPSHOT_DIR"],
                  !dirPath.isEmpty
            else { return }

            let mode = ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_SNAPSHOT_MODE"] ?? "timer"
            // Timer-based snapshots are handled centrally in `AppDelegate`. Keep this path for
            // the result page where we want to snapshot exactly after inspection finished.
            guard mode == "manual" else { return }

            didAutoSnapshot = true
	            let prefix = ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_SNAPSHOT_PREFIX"] ?? "snapshot"
	            let dir = URL(fileURLWithPath: dirPath, isDirectory: true)
	            let delayMs = Int(ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_SNAPSHOT_DELAY_MS"] ?? "") ?? 300
	            DispatchQueue.main.asyncAfter(deadline: .now() + Double(delayMs) / 1000.0) {
	                UISnapshot.captureVisibleWindows(to: dir, prefix: prefix)
	                Darwin.exit(0)
	            }
	        }
        .onAppear {
            if !didLoadInitialFile, let initialFileUrl {
                didLoadInitialFile = true
                loadSelectedFile(url: initialFileUrl)
            }

            guard SettingsUIDemo.enabled else { return }
            guard SettingsUIDemo.scene.hasPrefix("backup-config-import") else { return }

            // Allow deterministic import UI screenshots without Accessibility scripting.
            if inspection == nil,
               fileUrl == nil,
               let p = ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_DEMO_IMPORT_FILE"],
               !p.isEmpty
            {
                loadSelectedFile(url: URL(fileURLWithPath: p))
            }

            if SettingsUIDemo.scene == "backup-config-import-result",
               inspection == nil,
               fileUrl != nil,
               let s = ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_DEMO_IMPORT_TARGETS_COUNT"],
               let count = Int(s)
            {
                let decoded = demoInspection(targetsCount: count)
                inspection = decoded
                applyDefaults(from: decoded)
                return
            }

            if SettingsUIDemo.scene == "backup-config-import-result",
               inspection == nil,
               fileUrl != nil,
               fileEncrypted,
               passphrase.isEmpty,
               let pp = ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_DEMO_IMPORT_PASSPHRASE"],
               !pp.isEmpty
            {
                passphrase = pp
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) {
                    inspectBundle()
                }
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
    @State private var dialogsLoading: Bool = false
    @State private var dialogsError: String? = nil
    @State private var dialogs: [TelegramDialogItem] = []
    @State private var dialogsSearch: String = ""
    @State private var dialogsPickerShown: Bool = false

    struct TelegramWaitChatResponse: Decodable {
        let chat: TelegramDialogItem
    }

    struct CliErrorEnvelope: Decodable {
        let code: String
        let message: String
    }

    struct TelegramDialogItem: Decodable {
        let kind: String
        let title: String
        let username: String?
        let peerId: Int64
        let configChatId: String
        let bootstrapHint: Bool

        var id: String { "\(kind):\(peerId)" }
        var displayTitle: String {
            let u = username.map { "@\($0)" } ?? "-"
            return "\(configChatId)  \(title)  (\(kind), \(u))"
        }
    }

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

                    Button(dialogsLoading ? "Listening…" : "Listen…") {
                        dialogsError = nil
                        dialogsSearch = ""
                        dialogsPickerShown = true
                    }
                        .buttonStyle(.bordered)
                        .disabled(dialogsLoading)
                }

                let chatIdTrimmed = settings.telegram_endpoints[epIndex].chat_id.trimmingCharacters(in: .whitespacesAndNewlines)
                let chatIdInt = Int64(chatIdTrimmed)
                let isLikelyPrivateChat = (chatIdInt ?? 0) > 0
                if isLikelyPrivateChat {
                    Text("Heads up: private 1:1 chats don’t support the pinned bootstrap catalog, so cross-device restore / remote-first index sync won’t work. Use a group/channel (e.g. -100...) or @username.")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.orange)
                }
                if let dialogsError {
                    Text(dialogsError)
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.red)
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
        .sheet(isPresented: $dialogsPickerShown) {
            dialogPickerSheet(endpointId: endpointId)
        }
    }

    private func saveBotToken(endpointId: String) {
        guard let cli = model.cliPath() else { return }
        model.ensureDaemonRunning()
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
        model.ensureDaemonRunning()
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
        model.ensureDaemonRunning()
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
        model.ensureDaemonRunning()
        let epIndex = settings.telegram_endpoints.firstIndex(where: { $0.id == endpointId })
        let chatIdTrimmed = epIndex.map { settings.telegram_endpoints[$0].chat_id.trimmingCharacters(in: .whitespacesAndNewlines) } ?? ""
        let chatIdInt = Int64(chatIdTrimmed)
        let isLikelyPrivateChat = (chatIdInt ?? 0) > 0

        let res = model.runCommandCapture(
            exe: cli,
            args: ["--json", "telegram", "validate", "--endpoint-id", endpointId],
            timeoutSeconds: 180
        )
        if res.status == 0 {
            validateOk = true
            validateText = isLikelyPrivateChat ? "Connected (bootstrap unsupported: private chat)" : "Connected"
            onReload()
        } else {
            validateOk = false
            validateText = "Failed (see ui.log)"
        }
    }

    @ViewBuilder
    private func dialogPickerSheet(endpointId: String) -> some View {
        let filtered = dialogs
            .filter { $0.bootstrapHint }
            .filter { dialogsSearch.isEmpty || $0.displayTitle.localizedCaseInsensitiveContains(dialogsSearch) }

        VStack(alignment: .leading, spacing: 12) {
            Text("Discover chat_id for storage / bootstrap")
                .font(.system(size: 16, weight: .bold))

            Text("Telegram bots cannot list all joined chats. To discover a group/channel, click Listen and then send a message in the target chat (mention the bot if needed).")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(.secondary)

            TextField("Search…", text: $dialogsSearch)
                .textFieldStyle(.roundedBorder)

            HStack {
                Button(dialogsLoading ? "Listening…" : "Listen (60s)") { loadDialogs(endpointId: endpointId) }
                    .buttonStyle(.borderedProminent)
                    .disabled(dialogsLoading)
                Spacer()
                if let dialogsError {
                    Text(dialogsError)
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.red)
                }
            }

            List(filtered, id: \.id) { item in
                Button(item.displayTitle) {
                    if let epIndex = settings.telegram_endpoints.firstIndex(where: { $0.id == endpointId }) {
                        settings.telegram_endpoints[epIndex].chat_id = item.configChatId
                        onEndpointTouchedPending(endpointId)
                    }
                    dialogsPickerShown = false
                }
                .buttonStyle(.plain)
            }
            .frame(minWidth: 640, minHeight: 420)

            HStack {
                Text("Only group/channel chats are shown (bootstrapHint=true).")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.secondary)
                Spacer()
                Button("Close") { dialogsPickerShown = false }
                    .buttonStyle(.bordered)
            }
        }
        .padding(16)
    }

    private func loadDialogs(endpointId: String) {
        guard let cli = model.cliPath() else { return }
        dialogsLoading = true
        dialogsError = nil
        dialogsSearch = ""

        DispatchQueue.global(qos: .userInitiated).async {
            let res = model.runCommandCapture(
                exe: cli,
                args: ["--json", "telegram", "wait-chat", "--endpoint-id", endpointId, "--timeout-secs", "60"],
                timeoutSeconds: 75
            )

            DispatchQueue.main.async {
                dialogsLoading = false
                if res.status != 0 {
                    if let data = res.stderr.data(using: .utf8),
                       let decoded = try? JSONDecoder().decode(CliErrorEnvelope.self, from: data) {
                        dialogsError = decoded.message
                    } else {
                        dialogsError = "Failed to discover chat (see ui.log)"
                    }
                    return
                }
                guard let data = res.stdout.data(using: .utf8) else {
                    dialogsError = "Failed to parse chat (empty output)"
                    return
                }
                do {
                    let decoded = try JSONDecoder().decode(TelegramWaitChatResponse.self, from: data)
                    let item = decoded.chat
                    if !dialogs.contains(where: { $0.id == item.id }) {
                        dialogs.append(item)
                    }
                } catch {
                    dialogsError = "Failed to parse chat (see ui.log)"
                }
            }
        }
    }
}
