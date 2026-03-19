import AppKit
import Foundation
import SwiftUI

private let rateEstimateFreshnessSeconds: TimeInterval = 3.0

struct RunLogSummary: Identifiable {
    let id: String
    let kind: String
    let targetId: String?
    let endpointId: String?
    let sourcePath: String?
    let snapshotId: String?
    let status: String?
    let errorCode: String?
    let durationSeconds: Double?
    let startedAt: Date?
    let finishedAt: Date?
    let logURL: URL

    let bytesUploaded: Int64?
    let bytesDeduped: Int64?
    let bytesWritten: Int64?
    let bytesChecked: Int64?
    let filesRestored: Int64?
    let chunksDownloaded: Int64?
    let chunksChecked: Int64?
    let ignoreRuleFiles: Int64?
    let ignoreInvalidRules: Int64?
}

struct MainWindowRootView: View {
    @EnvironmentObject var model: AppModel
    @State private var selection: String?

    private enum Selection {
        static let unknownTarget = "__unknown_target__"
    }

    var body: some View {
        ZStack {
            VisualEffectView(material: .underWindowBackground, blendingMode: .behindWindow, state: .active)
                .ignoresSafeArea()

            NavigationSplitView {
                sidebar
                    // Default sidebar width should be comfortable enough to read status/stage without truncation.
                    .frame(minWidth: 240, idealWidth: 320, maxWidth: 480)
            } detail: {
                detail
            }
        }
        .onAppear {
            if MainWindowUIDemo.enabled {
                if selection == nil {
                    selection = MainWindowUIDemo.initialSelection(targets: model.statusSnapshot?.targets ?? [])
                }
            } else {
                model.refreshRunHistory()
            }
        }
        .toolbar {
            ToolbarItemGroup {
                Button {
                    model.refresh()
                    model.refreshRunHistory()
                } label: {
                    Label("Refresh", systemImage: "arrow.clockwise")
                }

                if let selection,
                   selection != Selection.unknownTarget,
                   let target = (model.statusSnapshot?.targets ?? []).first(where: { $0.targetId == selection })
                {
                    Menu {
                        Button("Backup now") { model.backupRun(targetId: target.targetId) }
                            .disabled(model.isRunning)
                        Divider()
                        Button("Restore…") { model.promptRestoreLatest(targetId: target.targetId) }
                            .disabled(model.isRunning)
                        Button("Verify") { model.verifyLatest(targetId: target.targetId) }
                            .disabled(model.isRunning)
                    } label: {
                        Label("Actions", systemImage: "ellipsis.circle")
                    }
                }

                Button {
                    model.openSettingsWindow()
                } label: {
                    Label("Settings", systemImage: "gearshape")
                }

                Button {
                    model.openLogs()
                } label: {
                    Label("Logs", systemImage: "doc.text.magnifyingglass")
                }
            }
        }
        .frame(minWidth: 860, minHeight: 520)
    }

    private var sidebar: some View {
        let targets = model.statusSnapshot?.targets ?? []
        let unknownCount = model.runHistory.filter { $0.targetId == nil }.count
        let now = Date()
        let nowMs = Int64(now.timeIntervalSince1970 * 1000.0)
        let snap = model.statusSnapshot
        let snapshotOffline = TargetPresentation.snapshotIsOffline(snap: snap, nowMs: nowMs)

        let sidebarStatusLine1: String? = {
            guard let snap else { return nil }
            let ageSeconds = Int(max(0, nowMs - snap.generatedAt) / 1000)
            let rel = TargetPresentation.formatRelativeSeconds(ageSeconds)
            let ageText = rel == "just now" ? rel : (rel + " ago")
            return "\(snapshotOffline ? "Offline" : "Online") · \(ageText)"
        }()

        let sidebarStatusLine2: String? = {
            guard let snap else { return nil }
            if snapshotOffline { return "No recent updates" }

            var parts: [String] = []
            if let up = snap.global.up.bytesPerSecond, up > 0 {
                parts.append("Upload \(formatBytes(up))/s")
            }
            if let down = snap.global.down.bytesPerSecond, down > 0 {
                parts.append("Download \(formatBytes(down))/s")
            }
            if parts.isEmpty { return nil }
            return parts.joined(separator: " · ")
        }()

        let sidebarStatusLine1Display = sidebarStatusLine1 ?? " "
        let sidebarStatusLine2Display = sidebarStatusLine2 ?? " "

        return VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Text("Targets")
                        .font(.system(size: 12, weight: .heavy))
                        .foregroundStyle(.secondary)
                    Spacer()
                    if model.runHistoryRefreshInFlight {
                        ProgressView()
                            .controlSize(.small)
                    }
                }

                Text(sidebarStatusLine1Display)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .opacity(sidebarStatusLine1 == nil ? 0 : 1)
                    .accessibilityHidden(sidebarStatusLine1 == nil)

                Text(sidebarStatusLine2Display)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .opacity(sidebarStatusLine2 == nil ? 0 : 1)
                    .accessibilityHidden(sidebarStatusLine2 == nil)
            }
            .padding(.horizontal, 12)
            .padding(.top, 10)
            .padding(.bottom, 6)

            List(selection: $selection) {
                ForEach(targets) { target in
                    TargetListRow(
                        target: target,
                        isSelected: selection == target.targetId,
                        isBusy: model.isRunning,
                        onBackup: {
                            model.backupRun(targetId: target.targetId)
                        },
                        onRestore: {
                            model.promptRestoreLatest(targetId: target.targetId)
                        },
                        onVerify: {
                            model.verifyLatest(targetId: target.targetId)
                        },
                        onSelect: {
                            selection = target.targetId
                        }
                    )
                    .tag(target.targetId)
                }

                if unknownCount > 0 {
                    UnknownTargetListRow(
                        isSelected: selection == Selection.unknownTarget,
                        count: unknownCount,
                        onSelect: {
                            selection = Selection.unknownTarget
                        }
                    )
                    .tag(Selection.unknownTarget)
                }
            }
        }
    }

    @ViewBuilder
    private var detail: some View {
        let targets = model.statusSnapshot?.targets ?? []
        if selection == Selection.unknownTarget {
            UnknownTargetDetailView()
        } else if let selection,
           let target = targets.first(where: { $0.targetId == selection })
        {
            TargetDetailView(target: target)
        } else {
            VStack(spacing: 12) {
                Image(systemName: "sidebar.left")
                    .font(.system(size: 44, weight: .semibold))
                    .foregroundStyle(.tertiary)

                Text("Select a target")
                    .font(.system(size: 18, weight: .bold))

                Text("Pick a target on the left to see backup / restore / verify history.")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 360)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

private enum MainWindowUIDemo {
    static var enabled: Bool {
        ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_DEMO"] == "1"
    }

    static var scene: String {
        ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_DEMO_SCENE"] ?? ""
    }

    static func initialSelection(targets: [StatusTarget]) -> String? {
        guard enabled else { return nil }
        if scene.hasPrefix("main-window-target-") {
            return targets.first?.targetId
        }
        return nil
    }
}

private struct TargetListRow: View {
    @EnvironmentObject var model: AppModel
    let target: StatusTarget
    let isSelected: Bool
    let isBusy: Bool
    let onBackup: () -> Void
    let onRestore: () -> Void
    let onVerify: () -> Void
    let onSelect: () -> Void

    private var runs: [RunLogSummary] {
        model.runHistory.filter { run in run.targetId == target.targetId }
    }

    private var hasInProgressRunLog: Bool {
        runs.contains(where: { $0.status == "running" })
    }

    private var activeForTarget: AppModel.ActiveTask? {
        guard let t = model.activeTask else { return nil }
        if t.state == "running" && t.targetId == target.targetId { return t }
        return nil
    }

    private var effectiveProgress: StatusProgress? {
        if let t = activeForTarget {
            return t.progress ?? target.progress
        }
        if let daemonProgress = target.progress {
            return daemonProgress
        }
        return nil
    }

    private func filesProgressText(_ p: StatusProgress?) -> String? {
        guard let p else { return nil }
        if let done = p.filesDone, let total = p.filesTotal, total > 0 {
            return "\(done)/\(total) files"
        }
        if let total = p.filesTotal, total > 0 {
            return "\(total) files"
        }
        if let done = p.filesDone, done > 0 {
            return "\(done) files"
        }
        return nil
    }

    var body: some View {
        let now = Date()
        let nowMs = Int64(now.timeIntervalSince1970 * 1000.0)
        let snap = model.statusSnapshot
        let status = TargetPresentation.userStatus(
            target: target,
            activeTask: model.activeTask,
            hasInProgressRunLog: hasInProgressRunLog,
            snap: snap,
            nowMs: nowMs
        )

        let runLogKind = runs.first(where: { $0.status == "running" })?.kind
        let kind = TargetPresentation.workKind(
            activeKind: activeForTarget?.kind,
            runLogKind: runLogKind,
            targetIsRunningInDaemon: target.state == "running"
        )

        let stage = TargetPresentation.stageText(effectiveProgress?.phase)

        let uploadBps: Int64? = {
            guard status == .running else { return nil }
            let fallback: Int64? = {
                guard let estimate = model.targetRateEstimates[target.targetId] else { return nil }
                guard now.timeIntervalSince(estimate.updatedAt) <= rateEstimateFreshnessSeconds else {
                    return nil
                }
                return estimate.uploadBytesPerSecond
            }()
            let bps = (target.up.bytesPerSecond ?? 0) > 0
                ? target.up.bytesPerSecond
                : fallback
            if let bps, bps > 0 { return bps }
            return nil
        }()

        let downloadBps: Int64? = {
            guard status == .running else { return nil }
            guard let estimate = model.targetRateEstimates[target.targetId] else { return nil }
            guard now.timeIntervalSince(estimate.updatedAt) <= rateEstimateFreshnessSeconds else {
                return nil
            }
            let bps = estimate.downloadBytesPerSecond
            if let bps, bps > 0 { return bps }
            return nil
        }()

        let rowSecondaryLeft: String = {
            switch status {
            case .running:
                var parts: [String] = []
                if let stage { parts.append(stage) }
                if let fp = filesProgressText(effectiveProgress) { parts.append(fp) }
                return parts.isEmpty ? "Working…" : parts.joined(separator: " · ")
            case .idle:
                return TargetPresentation.lastRunCompact(target: target, now: now) ?? "No recent runs."
            case .failed:
                return TargetPresentation.lastRunCompact(target: target, now: now) ?? "Last run: Failed"
            case .offline:
                return "No recent updates."
            }
        }()

        let rowSecondaryRight: (systemImage: String, text: String)? = {
            guard status == .running else { return nil }
            switch kind {
            case .backup:
                if let bps = uploadBps {
                    return ("arrow.up", "\(formatBytes(bps))/s")
                }
                return nil
            case .restore:
                if let bps = downloadBps {
                    return ("arrow.down", "\(formatBytes(bps))/s")
                }
                return nil
            case .verify, .unknown:
                return nil
            }
        }()

        let progressVisual: BackupProgressVisual = {
            if kind == .backup {
                return TargetPresentation.backupProgressVisual(effectiveProgress)
            }
            return .indeterminate
        }()

        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(target.label ?? target.targetId)
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                    .truncationMode(.tail)

                if !target.enabled {
                    Text("Disabled")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.secondary)
                }

                Spacer(minLength: 0)
            }

            HStack(spacing: 8) {
                StatusBadge(status: status, style: .inline, isSelected: isSelected)
                Text(rowSecondaryLeft)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(isSelected ? Color.primary.opacity(0.92) : .secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)

                Spacer(minLength: 0)

                if let rowSecondaryRight {
                    HStack(spacing: 4) {
                        Image(systemName: rowSecondaryRight.systemImage)
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(isSelected ? Color.primary.opacity(0.85) : .secondary)
                        Text(rowSecondaryRight.text)
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(isSelected ? Color.primary.opacity(0.85) : .secondary)
                    }
                    .lineLimit(1)
                }
            }

            if status == .running {
                BackupUnifiedProgressBar(visual: progressVisual, tint: status.tint)
            }
        }
        .contentShape(Rectangle())
        .onTapGesture { onSelect() }
        .contextMenu {
            Button("Backup now") { onBackup() }
                .disabled(isBusy)
            Divider()
            Button("Restore…") { onRestore() }
                .disabled(isBusy)
            Button("Verify") { onVerify() }
                .disabled(isBusy)
        }
        .help(target.sourcePath)
        .padding(.vertical, 4)
    }
}

private struct TargetDetailView: View {
    @EnvironmentObject var model: AppModel
    let target: StatusTarget

    private struct OverviewMetricItem: Identifiable {
        let id = UUID()
        let title: String
        let value: String
        let systemImage: String
    }

    private enum Tab: String, CaseIterable, Identifiable {
        case history = "History"
        case diagnostics = "Diagnostics"

        var id: String { rawValue }
    }

    @State private var tab: Tab = {
        if ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_DEMO"] == "1" {
            let scene = ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_DEMO_SCENE"] ?? ""
            if scene == "main-window-target-diagnostics" {
                return .diagnostics
            }
        }
        return .history
    }()

    private var runs: [RunLogSummary] {
        model.runHistory
            .filter { run in run.targetId == target.targetId }
            .sorted {
                let a = $0.finishedAt ?? $0.startedAt ?? .distantPast
                let b = $1.finishedAt ?? $1.startedAt ?? .distantPast
                return a > b
            }
    }

    private var hasInProgressRunLog: Bool {
        runs.contains(where: { $0.status == "running" })
    }

    private var activeForTarget: AppModel.ActiveTask? {
        guard let t = model.activeTask else { return nil }
        if t.state == "running" && t.targetId == target.targetId { return t }
        return nil
    }

    private var effectiveProgress: StatusProgress? {
        if let t = activeForTarget {
            return t.progress ?? target.progress
        }
        if let daemonProgress = target.progress {
            return daemonProgress
        }
        return nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            controlsRow
            Divider()
            if tab == .history {
                history
            } else {
                TargetDiagnosticsView(target: target)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private var controlsRow: some View {
        HStack(spacing: 12) {
            Picker("", selection: $tab) {
                ForEach(Tab.allCases) { t in
                    Text(t.rawValue).tag(t)
                }
            }
            .pickerStyle(.segmented)
            .controlSize(.small)
            .fixedSize()

            Spacer(minLength: 0)

            if tab == .diagnostics {
                Button {
                    model.copyStatusSnapshotJsonToClipboard()
                } label: {
                    Label("Copy JSON", systemImage: "doc.on.doc")
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .disabled(model.statusSnapshot == nil)

                Button {
                    model.revealStatusSourceInFinder()
                } label: {
                    Label("Reveal…", systemImage: "folder")
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
            }
        }
    }

    private var header: some View {
        let now = Date()
        let nowMs = Int64(now.timeIntervalSince1970 * 1000.0)
        let snap = model.statusSnapshot
        let status = TargetPresentation.userStatus(
            target: target,
            activeTask: model.activeTask,
            hasInProgressRunLog: hasInProgressRunLog,
            snap: snap,
            nowMs: nowMs
        )

        let runLogKind = runs.first(where: { $0.status == "running" })?.kind
        let kind = TargetPresentation.workKind(
            activeKind: activeForTarget?.kind,
            runLogKind: runLogKind,
            targetIsRunningInDaemon: target.state == "running"
        )

        let p = effectiveProgress

        let stageText = TargetPresentation.stageText(p?.phase)
            ?? {
                switch kind {
                case .backup: return "Backing up"
                case .restore: return "Restoring"
                case .verify: return "Verifying"
                case .unknown: return "Working"
                }
            }()

        let elapsedText: String? = {
            guard status == .running else { return nil }
            let nowMs = Int64(now.timeIntervalSince1970 * 1000.0)
            if let t = activeForTarget {
                return formatDuration(Date().timeIntervalSince(t.startedAt))
            }
            if let since = target.runningSince {
                let secs = max(0, (nowMs - since)) / 1000
                return formatDuration(Double(secs))
            }
            return nil
        }()

        let stageAndElapsed: String? = {
            guard status == .running else { return nil }
            var parts: [String] = [stageText]
            if let elapsedText, !elapsedText.isEmpty {
                parts.append(elapsedText)
            }
            return parts.joined(separator: " · ")
        }()

        return VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .center, spacing: 10) {
                Text(target.label ?? target.targetId)
                    .font(.system(size: 18, weight: .bold))
                    .lineLimit(1)
                    .truncationMode(.tail)

                StatusBadge(status: status, style: .pill)
                    .fixedSize(horizontal: true, vertical: false)

                if let stageAndElapsed {
                    Text(stageAndElapsed)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                }

                if !target.enabled {
                    Text("Disabled")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }

                Spacer(minLength: 0)
            }

            Text(target.sourcePath)
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)

            sourceIgnoreHintView

            overviewStats(now: now, status: status, kind: kind)
        }
    }

    private var sourceIgnoreHintView: some View {
        let rootIgnorePath = URL(fileURLWithPath: target.sourcePath, isDirectory: true)
            .appendingPathComponent(".televyignore", isDirectory: false)
        let rootIgnoreExists = FileManager.default.fileExists(atPath: rootIgnorePath.path)

        return Label(
            rootIgnoreExists
                ? "Ignore active · root .televyignore detected"
                : "Ignore active · root .televyignore not found",
            systemImage: rootIgnoreExists ? "checkmark.shield" : "shield"
        )
        .font(.system(size: 11, weight: .semibold))
        .foregroundStyle(.secondary)
        .lineLimit(1)
        .truncationMode(.tail)
        .help("Backup scanning reads .televyignore from this source tree (root and nested folders).")
    }

    private func overviewStats(now: Date, status: TargetUserStatus, kind: TargetWorkKind) -> some View {
        let p = effectiveProgress

        if status != .running {
            return AnyView(
                VStack(alignment: .leading, spacing: 8) {
                    let last = TargetPresentation.lastRunSummary(target: target, now: now)
                    if let last {
                        Text(last)
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.secondary)
                            .lineLimit(2)
                            .truncationMode(.tail)
                    } else if status != .offline {
                        Text("No runs yet.")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.secondary)
                    }

                    if status == .offline {
                        Text("No recent updates.")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.secondary)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            )
        }

        let speedText: String? = {
            switch kind {
            case .backup:
                let fallback: Int64? = {
                    guard let estimate = model.targetRateEstimates[target.targetId] else { return nil }
                    guard now.timeIntervalSince(estimate.updatedAt) <= rateEstimateFreshnessSeconds else {
                        return nil
                    }
                    return estimate.uploadBytesPerSecond
                }()
                let bps = (target.up.bytesPerSecond ?? 0) > 0
                    ? target.up.bytesPerSecond
                    : fallback
                if let bps, bps > 0 { return "Upload \(formatBytes(bps))/s" }
                return nil
            case .restore:
                if let estimate = model.targetRateEstimates[target.targetId],
                   now.timeIntervalSince(estimate.updatedAt) <= rateEstimateFreshnessSeconds,
                   let bps = estimate.downloadBytesPerSecond, bps > 0
                {
                    return "Download \(formatBytes(bps))/s"
                }
                return "Measuring…"
            case .verify, .unknown:
                return nil
            }
        }()

        let filesText = doneTotalText(done: p?.filesDone, total: p?.filesTotal)

        func bytesText(_ value: Int64?) -> String {
            guard let value else { return "Waiting…" }
            return formatBytes(value)
        }

        func percentText(_ value: Double) -> String {
            let clamped = min(1.0, max(0.0, value))
            return "\(Int(floor(clamped * 100.0)))%"
        }

        let bytesReadValue = p?.bytesRead ?? 0
        let savedBytes = p?.bytesDeduped ?? 0

        func rowView(_ items: [OverviewMetricItem]) -> some View {
            LazyVGrid(
                columns: [GridItem(.adaptive(minimum: 160), spacing: 14, alignment: .leading)],
                alignment: .leading,
                spacing: 10
            ) {
                ForEach(items) { item in
                    OverviewMetric(title: item.title, value: item.value, systemImage: item.systemImage)
                }
            }
        }

        var items: [OverviewMetricItem] = []
        if kind == .backup || kind == .restore {
            let stableSpeedText = speedText ?? (kind == .backup ? "Upload 0 B/s" : "Download 0 B/s")
            items.append(.init(title: "Speed", value: stableSpeedText, systemImage: "arrow.up.arrow.down"))
        }

        switch kind {
        case .backup:
            items.append(
                .init(
                    title: "Uploaded",
                    value: bytesText(BackupProgressProjection.displayUploadedBytes(p)),
                    systemImage: "arrow.up.circle"
                )
            )
            items.append(.init(title: "Files", value: filesText, systemImage: "doc.on.doc"))
            items.append(.init(title: "Read", value: bytesText(p?.bytesRead), systemImage: "internaldrive"))
            items.append(.init(title: "Saved", value: bytesText(p?.bytesDeduped ?? (savedBytes > 0 ? savedBytes : 0)), systemImage: "leaf"))

            if let fractions = TargetPresentation.backupFractions(p) {
                let needUploadScope = BackupProgressProjection.needUploadScope(p)
                let needUploadTitle = needUploadScope == .final ? "Need Upload (Final)" : "Need Upload (Disc.)"
                let remainingUploadTitle = needUploadScope == .final ? "Remaining (Final)" : "Remaining (Disc.)"
                let needUploadBytes = BackupProgressProjection.displayNeedUploadBytes(p)
                let remainingUploadBytes = BackupProgressProjection.displayRemainingUploadBytes(p)
                items.append(.init(title: "Uploading", value: percentText(fractions.uploadCurrent), systemImage: "arrow.up.circle"))
                items.append(.init(title: "Backed Up", value: percentText(fractions.backedUp), systemImage: "checkmark.circle"))
                items.append(.init(title: needUploadTitle, value: bytesText(needUploadBytes), systemImage: "arrow.up.circle.badge.clock"))
                items.append(.init(title: remainingUploadTitle, value: bytesText(remainingUploadBytes), systemImage: "hourglass"))
                items.append(.init(title: "Scanned", value: percentText(fractions.scan), systemImage: "magnifyingglass"))
            } else {
                items.append(.init(title: "Uploading", value: "Waiting…", systemImage: "arrow.up.circle"))
                items.append(.init(title: "Backed Up", value: "Waiting…", systemImage: "checkmark.circle"))
                let needUploadScope = BackupProgressProjection.needUploadScope(p)
                let needUploadTitle = needUploadScope == .final ? "Need Upload (Final)" : "Need Upload (Disc.)"
                let remainingUploadTitle = needUploadScope == .final ? "Remaining (Final)" : "Remaining (Disc.)"
                items.append(.init(title: needUploadTitle, value: "Waiting…", systemImage: "arrow.up.circle.badge.clock"))
                items.append(.init(title: remainingUploadTitle, value: "Waiting…", systemImage: "hourglass"))
                items.append(.init(title: "Scanned", value: "Waiting…", systemImage: "magnifyingglass"))
            }
        case .restore:
            items.append(.init(title: "Downloaded", value: bytesText(p?.bytesDownloaded), systemImage: "arrow.down.circle"))
            items.append(.init(title: "Files", value: filesText, systemImage: "doc.on.doc"))
            if p?.bytesRead != nil || bytesReadValue > 0 {
                items.append(.init(title: "Written", value: bytesText(p?.bytesRead), systemImage: "square.and.arrow.down.on.square"))
            }
        case .verify:
            items.append(.init(title: "Checked", value: bytesText(p?.bytesRead), systemImage: "checkmark.seal"))
            items.append(.init(title: "Files", value: filesText, systemImage: "doc.on.doc"))
        case .unknown:
            items.append(.init(title: "Files", value: filesText, systemImage: "doc.on.doc"))
        }

        return AnyView(
            VStack(alignment: .leading, spacing: 6) {
                rowView(items)

                BackupUnifiedProgressBar(
                    visual: {
                        if kind == .backup {
                            return TargetPresentation.backupProgressVisual(p)
                        }
                        return .indeterminate
                    }(),
                    tint: status.tint
                )
            }
        )
    }

    private func doneTotalText(done: Int64?, total: Int64?) -> String {
        if let done, let total, total > 0 {
            return "\(done)/\(total)"
        }
        if let total, total > 0 {
            return "\(total)"
        }
        if let done, done > 0 {
            return "\(done)"
        }
        return "Waiting…"
    }

    private func activeTaskSummary(_ t: AppModel.ActiveTask) -> String {
        let stage = TargetPresentation.stageText(t.progress?.phase) ?? "Working"
        let elapsed = Date().timeIntervalSince(t.startedAt)
        let elapsedText = formatDuration(elapsed)

        var parts: [String] = []
        parts.append("\(t.kind.uppercased()) · \(stage)")

        let bytesUploaded = t.progress?.bytesUploaded ?? t.progress?.bytesUploadedSource ?? 0
        let bytesDownloaded = t.progress?.bytesDownloaded ?? 0
        let bytesRead = t.progress?.bytesRead ?? 0
        let bytesDeduped = t.progress?.bytesDeduped ?? 0

        switch t.kind {
        case "backup":
            if bytesUploaded > 0 { parts.append("Uploaded \(formatBytes(bytesUploaded))") }
            if bytesUploaded == 0 && bytesRead > 0 { parts.append("Read \(formatBytes(bytesRead))") }
            if bytesDeduped > 0 { parts.append("Saved \(formatBytes(bytesDeduped))") }
        case "restore":
            if bytesDownloaded > 0 { parts.append("Downloaded \(formatBytes(bytesDownloaded))") }
            if bytesRead > 0 { parts.append("Written \(formatBytes(bytesRead))") }
        case "verify":
            if bytesRead > 0 { parts.append("Checked \(formatBytes(bytesRead))") }
        default:
            if bytesRead > 0 { parts.append("Read \(formatBytes(bytesRead))") }
        }

        if let done = t.progress?.chunksDone {
            if let total = t.progress?.chunksTotal, total > 0 {
                parts.append("Chunks \(done)/\(total)")
            } else {
                parts.append("Chunks \(done)")
            }
        } else if let done = t.progress?.filesDone {
            if let total = t.progress?.filesTotal, total > 0 {
                parts.append("Files \(done)/\(total)")
            } else {
                parts.append("Files \(done)")
            }
        }

        parts.append("Elapsed \(elapsedText)")
        return parts.joined(separator: " · ")
    }

    private func activeTaskFraction(_ t: AppModel.ActiveTask) -> Double? {
        guard let p = t.progress else { return nil }
        if let done = p.chunksDone, let total = p.chunksTotal, total > 0 {
            if done == total && (p.phase == "scan" || p.phase == "scan_upload" || p.phase == "upload" || p.phase == "index" || p.phase == "index_sync") {
                return nil
            }
            return min(1.0, Double(done) / Double(total))
        }
        if let done = p.filesDone, let total = p.filesTotal, total > 0 {
            if done == total && (p.phase == "scan" || p.phase == "scan_upload" || p.phase == "upload" || p.phase == "index" || p.phase == "index_sync") {
                return nil
            }
            return min(1.0, Double(done) / Double(total))
        }
        return nil
    }

    @ViewBuilder
    private var history: some View {
        if runs.isEmpty {
            historyEmptyState
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        } else {
            List(runs) { run in
                RunLogRow(run: run)
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .frame(maxHeight: .infinity)
        }
    }

    private var historyEmptyState: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 10) {
                    Image(systemName: "clock")
                        .font(.system(size: 18, weight: .semibold))
                        .foregroundStyle(.secondary)
                    Text("No history yet")
                        .font(.system(size: 14, weight: .bold))
                }

                Text("Run logs will appear here after the first completed backup / restore / verify.")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.secondary)

                if let last = TargetPresentation.lastRunSummary(target: target, now: Date()) {
                    Text(last)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                        .truncationMode(.tail)
                }

                Button {
                    model.openLogs()
                } label: {
                    Label("Open logs", systemImage: "folder")
                }
                .buttonStyle(.borderless)
                .controlSize(.small)
            }
            .padding(14)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .strokeBorder(Color.primary.opacity(0.06), lineWidth: 1)
            )
            .frame(maxWidth: 560)

            let updates = recentTargetActivity(limit: 6)
            if !updates.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Recent updates")
                        .font(.system(size: 11, weight: .heavy))
                        .foregroundStyle(.secondary)

                    ForEach(updates) { item in
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Text(item.relativeWhen)
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(.secondary)
                                .frame(width: 72, alignment: .leading)

                            Text(item.text)
                                .font(.system(size: 11, weight: .medium))
                                .foregroundStyle(.secondary)
                                .lineLimit(2)
                                .truncationMode(.tail)
                        }
                    }
                }
                .padding(.top, 2)
                .frame(maxWidth: 760, alignment: .leading)
            }

            Spacer(minLength: 0)
        }
        .padding(.top, 6)
    }

    private struct RecentUpdateItem: Identifiable {
        let id = UUID()
        let relativeWhen: String
        let text: String
    }

    private func recentTargetActivity(limit: Int) -> [RecentUpdateItem] {
        let all = Array(model.statusActivity.suffix(200).reversed())
        let filtered = all.filter { $0.text.contains(target.targetId) }
        let now = Date()
        return filtered.prefix(limit).map { item in
            let ageSeconds = Int(now.timeIntervalSince(item.at))
            let rel = TargetPresentation.formatRelativeSeconds(max(0, ageSeconds))
            return RecentUpdateItem(relativeWhen: rel, text: presentActivityText(item.text))
        }
    }

    private func presentActivityText(_ raw: String) -> String {
        let prefix = "Target \(target.targetId) "
        var s = raw
        if s.hasPrefix(prefix) {
            s = String(s.dropFirst(prefix.count))
        }

        if s.hasPrefix("state ") {
            s = "State " + String(s.dropFirst("state ".count))
        }
        if s.hasPrefix("lastRun ") {
            s = "Last run " + String(s.dropFirst("lastRun ".count))
        }
        s = s.replacingOccurrences(of: " lastRun ", with: " Last run ")
        return s
    }
}

private struct OverviewMetric: View {
    let title: String
    let value: String
    let systemImage: String

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 6) {
                Image(systemName: systemImage)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.secondary)
                Text(title)
                    .font(.system(size: 10, weight: .heavy))
                    .foregroundStyle(.secondary)
            }
            Text(value)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(.primary)
                .lineLimit(1)
                .truncationMode(.tail)
        }
    }
}

private struct UnknownTargetListRow: View {
    let isSelected: Bool
    let count: Int
    let onSelect: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text("Unknown target")
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                Text("\(count) run(s)")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

            Spacer(minLength: 0)

            Image(systemName: "questionmark.circle")
                .foregroundStyle(isSelected ? Color.white.opacity(0.95) : Color.secondary)
        }
        .contentShape(Rectangle())
        .onTapGesture { onSelect() }
        .padding(.vertical, 2)
    }
}

private struct UnknownTargetDetailView: View {
    @EnvironmentObject var model: AppModel

    private var runs: [RunLogSummary] {
        model.runHistory
            .filter { $0.targetId == nil }
            .sorted { ($0.finishedAt ?? .distantPast) > ($1.finishedAt ?? .distantPast) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Unknown target")
                    .font(.system(size: 20, weight: .bold))
                Text("Runs with missing target_id (legacy logs).")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.secondary)
            }

            Divider()

            if runs.isEmpty {
                Text("No legacy run logs.")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.secondary)
                    .padding(.vertical, 6)
            } else {
                List(runs) { run in
                    RunLogRow(run: run)
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
                .frame(maxHeight: .infinity)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}

private struct StatusBadge: View {
    enum Style {
        case pill
        case inline
    }

    let status: TargetUserStatus
    let style: Style
    var isSelected: Bool = false

    var body: some View {
        let content = HStack(alignment: .center, spacing: 6) {
            Circle()
                .fill(status.tint)
                .frame(width: 6, height: 6)
                // Selected list rows may use accent-colored backgrounds; add a light outline so the
                // status dot doesn't get swallowed by the selection highlight.
                .overlay(
                    Circle()
                        .strokeBorder(Color.white.opacity(isSelected ? 0.85 : 0.0), lineWidth: 1)
                )
                .shadow(color: Color.black.opacity(isSelected ? 0.18 : 0.0), radius: 1, x: 0, y: 0)
            Text(status.title)
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle({
                    switch style {
                    case .pill:
                        return status.tint.opacity(0.92)
                    case .inline:
                        return isSelected ? Color.primary.opacity(0.92) : Color.secondary
                    }
                }())
        }

        switch style {
        case .pill:
            content
                .padding(.vertical, 3)
                .padding(.horizontal, 8)
                .background(status.tint.opacity(0.12), in: Capsule())
                .overlay(
                    Capsule()
                        .strokeBorder(status.tint.opacity(0.18), lineWidth: 1)
                )
        case .inline:
            content
        }
    }
}

private struct RunLogRow: View {
    let run: RunLogSummary

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 8) {
                    Text(run.kind.uppercased())
                        .font(.system(size: 11, weight: .heavy))
                    let statusText = run.status ?? "—"
                    Text(statusText)
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle({
                            switch run.status {
                            case "succeeded": return Color.green
                            case "failed": return Color.red
                            case "running": return Color.blue
                            default: return Color.secondary
                            }
                        }())
                    Spacer()
                }

                Text(summaryLine(run))
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            Spacer(minLength: 8)

            HStack(alignment: .center, spacing: 8) {
                if let at = run.finishedAt ?? run.startedAt {
                    Text(at.formatted(date: .abbreviated, time: .standard))
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .frame(minWidth: 160, alignment: .trailing)
                }

                Button {
                    NSWorkspace.shared.activateFileViewerSelecting([run.logURL])
                } label: {
                    Image(systemName: "doc.text")
                }
                .buttonStyle(.borderless)
                .help("Reveal log file in Finder")
            }
        }
    }

    private func summaryLine(_ r: RunLogSummary) -> String {
        var parts: [String] = []
        if let d = r.durationSeconds, d > 0 {
            parts.append(formatDuration(d))
        }
        if let e = r.errorCode, !e.isEmpty {
            parts.append("Error \(e)")
        }
        if let b = r.bytesUploaded, b > 0 {
            parts.append("Uploaded \(formatBytes(b))")
        }
        if let b = r.bytesDeduped, b > 0 {
            parts.append("Saved \(formatBytes(b))")
        }
        if let b = r.bytesWritten, b > 0 {
            parts.append("Written \(formatBytes(b))")
        }
        if let b = r.bytesChecked, b > 0 {
            parts.append("Checked \(formatBytes(b))")
        }
        if let f = r.filesRestored, f > 0 {
            parts.append("Restored \(f) files")
        }
        if let c = r.chunksDownloaded, c > 0 {
            parts.append("Downloaded \(c) chunks")
        }
        if let c = r.chunksChecked, c > 0 {
            parts.append("Checked \(c) chunks")
        }
        if r.kind == "backup" {
            if let files = r.ignoreRuleFiles, files > 0 {
                parts.append("Ignore files \(files)")
            }
            if let invalid = r.ignoreInvalidRules, invalid > 0 {
                parts.append("Bad ignore rules \(invalid)")
            }
        }
        if parts.isEmpty {
            return "—"
        }
        return parts.joined(separator: " · ")
    }
}
