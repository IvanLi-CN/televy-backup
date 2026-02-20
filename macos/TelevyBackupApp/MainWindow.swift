import AppKit
import Foundation
import SwiftUI

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
                    .frame(minWidth: 200, idealWidth: 260, maxWidth: 380)
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
        return VStack(spacing: 0) {
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
            .padding(.horizontal, 12)
            .padding(.top, 10)
            .padding(.bottom, 6)

            List(selection: $selection) {
                ForEach(targets) { target in
                    TargetListRow(
                        target: target,
                        isSelected: selection == target.targetId,
                        isBusy: model.isRunning,
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
        if scene == "main-window-target-detail" {
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
    let onRestore: () -> Void
    let onVerify: () -> Void
    let onSelect: () -> Void

    @State private var isHovering: Bool = false

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
        return target.progress
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

        let speedText: String? = {
            guard status == .running else { return nil }
            switch kind {
            case .backup:
                let bps = (target.up.bytesPerSecond ?? 0) > 0
                    ? target.up.bytesPerSecond
                    : model.targetRateEstimates[target.targetId]?.uploadBytesPerSecond
                if let bps, bps > 0 { return "Upload \(formatBytes(bps))/s" }
                return nil
            case .restore:
                if let bps = model.targetRateEstimates[target.targetId]?.downloadBytesPerSecond, bps > 0 {
                    return "Download \(formatBytes(bps))/s"
                }
                return "Download Waiting…"
            case .verify, .unknown:
                return nil
            }
        }()

        let secondaryText: String = {
            switch status {
            case .running:
                var parts: [String] = []
                if let stage { parts.append(stage) }
                if let speedText { parts.append(speedText) }
                if let fp = filesProgressText(effectiveProgress) { parts.append(fp) }
                return parts.isEmpty ? "Working…" : parts.joined(separator: " · ")
            case .idle:
                if let last = TargetPresentation.lastRunCompact(target: target, now: now) {
                    return last
                }
                return "No recent runs."
            case .failed:
                if let last = TargetPresentation.lastRunCompact(target: target, now: now) {
                    return last
                }
                return "Last run: Failed"
            case .offline:
                return "No recent updates."
            }
        }()

        let progressFrac = TargetPresentation.progressFraction(effectiveProgress)

        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                StatusMark(color: status.tint)
                Text(target.label ?? target.targetId)
                    .font(.system(size: 12, weight: .semibold))
                    .lineLimit(1)
                    .truncationMode(.tail)
                Spacer(minLength: 0)

                HStack(spacing: 10) {
                    Button(action: onRestore) {
                        Image(systemName: "arrow.down.doc")
                            .symbolRenderingMode(.hierarchical)
                    }
                    .buttonStyle(.borderless)
                    .help("Restore…")
                    .disabled(isBusy)

                    Button(action: onVerify) {
                        Image(systemName: "checkmark.seal")
                            .symbolRenderingMode(.hierarchical)
                    }
                    .buttonStyle(.borderless)
                    .help("Verify")
                    .disabled(isBusy)
                }
                .opacity((isSelected || isHovering) ? 1.0 : 0.6)
            }

            HStack(spacing: 8) {
                Text(status.title)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(status == .idle ? Color.secondary : Color.primary.opacity(0.9))

                Text(secondaryText)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)

                Spacer(minLength: 0)
            }

            if status == .running {
                if let progressFrac {
                    ProgressView(value: progressFrac)
                        .progressViewStyle(.linear)
                        .controlSize(.mini)
                        .tint(status.tint)
                } else {
                    ProgressView()
                        .progressViewStyle(.linear)
                        .controlSize(.mini)
                        .tint(status.tint)
                }
            }
        }
        .contentShape(Rectangle())
        .onTapGesture { onSelect() }
        .onHover { isHovering = $0 }
        .help(target.sourcePath)
        .padding(.vertical, 4)
    }
}

private struct TargetDetailView: View {
    @EnvironmentObject var model: AppModel
    let target: StatusTarget

    private enum Tab: String, CaseIterable, Identifiable {
        case history = "History"
        case diagnostics = "Diagnostics"

        var id: String { rawValue }
    }

    @State private var tab: Tab = .history

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
        return target.progress
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            HStack {
                Picker("", selection: $tab) {
                    ForEach(Tab.allCases) { t in
                        Text(t.rawValue).tag(t)
                    }
                }
                .pickerStyle(.segmented)
                .controlSize(.small)

                Spacer(minLength: 0)
            }
            Divider()
            if tab == .history {
                history
            } else {
                TargetDiagnosticsView(target: target)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
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

        return VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .top, spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(target.label ?? target.targetId)
                        .font(.system(size: 20, weight: .bold))

                    Text(target.sourcePath)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)

                    HStack(spacing: 10) {
                        StatusMark(color: status.tint)
                        Text(status.title)
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.primary.opacity(0.92))

                        if !target.enabled {
                            Text("Disabled")
                                .font(.system(size: 12, weight: .semibold))
                                .foregroundStyle(.secondary)
                        }

                        Spacer(minLength: 0)
                    }
                }

                Spacer(minLength: 0)

                HStack(spacing: 10) {
                    Button("Restore…") { model.promptRestoreLatest(targetId: target.targetId) }
                        .buttonStyle(.borderedProminent)
                        .controlSize(.small)
                        .disabled(model.isRunning)
                    Button("Verify") { model.verifyLatest(targetId: target.targetId) }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                        .disabled(model.isRunning)
                }
                .padding(.top, 2)
            }

            overviewStats(now: now, status: status, kind: kind)
        }
    }

    private func overviewStats(now: Date, status: TargetUserStatus, kind: TargetWorkKind) -> some View {
        let p = effectiveProgress

        if status != .running {
            return AnyView(
                VStack(alignment: .leading, spacing: 8) {
                    if status == .offline {
                        Text("No recent updates.")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.secondary)
                    } else if let last = TargetPresentation.lastRunSummary(target: target, now: now) {
                        Text(last)
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.secondary)
                            .lineLimit(2)
                            .truncationMode(.tail)
                    } else {
                        Text("No runs yet.")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.secondary)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            )
        }

        let stageText = TargetPresentation.stageText(p?.phase) ?? "Working"

        let nowMs = Int64(now.timeIntervalSince1970 * 1000.0)
        let elapsedText: String = {
            if let t = activeForTarget {
                return formatDuration(Date().timeIntervalSince(t.startedAt))
            }
            if let since = target.runningSince {
                let secs = max(0, (nowMs - since)) / 1000
                return formatDuration(Double(secs))
            }
            return "—"
        }()

        let speedText: String? = {
            switch kind {
            case .backup:
                let bps = (target.up.bytesPerSecond ?? 0) > 0
                    ? target.up.bytesPerSecond
                    : model.targetRateEstimates[target.targetId]?.uploadBytesPerSecond
                if let bps, bps > 0 { return "Upload \(formatBytes(bps))/s" }
                return nil
            case .restore:
                if let bps = model.targetRateEstimates[target.targetId]?.downloadBytesPerSecond, bps > 0 {
                    return "Download \(formatBytes(bps))/s"
                }
                return "Download Waiting…"
            case .verify, .unknown:
                return nil
            }
        }()

        let filesText = doneTotalText(done: p?.filesDone, total: p?.filesTotal)

        func metric(_ title: String, _ value: String, _ systemImage: String) -> OverviewMetric {
            OverviewMetric(title: title, value: value, systemImage: systemImage)
        }

        let uploadedText = formatBytes(p?.bytesUploaded ?? 0)
        let downloadedText = formatBytes(p?.bytesDownloaded ?? 0)
        let bytesReadValue = p?.bytesRead ?? 0
        let bytesReadText = formatBytes(bytesReadValue)
        let savedBytes = p?.bytesDeduped ?? 0
        let savedText = formatBytes(savedBytes)

        let columns: [GridItem] = [
            GridItem(.adaptive(minimum: 140, maximum: 220), spacing: 12, alignment: .leading)
        ]

        return AnyView(
            VStack(alignment: .leading, spacing: 8) {
                LazyVGrid(columns: columns, alignment: .leading, spacing: 10) {
                    metric("Stage", stageText, "bolt.fill")
                    if elapsedText != "—" {
                        metric("Elapsed", elapsedText, "clock")
                    }
                    if let speedText {
                        metric("Speed", speedText, "arrow.up.arrow.down")
                    }

                    switch kind {
                    case .backup:
                        metric("Uploaded", uploadedText, "arrow.up.circle")
                        metric("Files", filesText, "doc.on.doc")
                        if bytesReadValue > 0 {
                            metric("Read", bytesReadText, "internaldrive")
                        }
                        if savedBytes > 0 {
                            metric("Saved", savedText, "leaf")
                        }
                    case .restore:
                        metric("Downloaded", downloadedText, "arrow.down.circle")
                        metric("Files", filesText, "doc.on.doc")
                        if bytesReadValue > 0 {
                            metric("Written", bytesReadText, "square.and.arrow.down.on.square")
                        }
                    case .verify:
                        metric("Checked", bytesReadText, "checkmark.seal")
                        metric("Files", filesText, "doc.on.doc")
                    case .unknown:
                        metric("Files", filesText, "doc.on.doc")
                    }
                }

                if let frac = TargetPresentation.progressFraction(p) {
                    ProgressView(value: frac)
                        .progressViewStyle(.linear)
                } else {
                    ProgressView()
                        .progressViewStyle(.linear)
                }
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

        let bytesUploaded = t.progress?.bytesUploaded ?? 0
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
            if done == total && (p.phase == "scan" || p.phase == "upload" || p.phase == "index" || p.phase == "index_sync") {
                return nil
            }
            return min(1.0, Double(done) / Double(total))
        }
        if let done = p.filesDone, let total = p.filesTotal, total > 0 {
            if done == total && (p.phase == "scan" || p.phase == "upload" || p.phase == "index" || p.phase == "index_sync") {
                return nil
            }
            return min(1.0, Double(done) / Double(total))
        }
        return nil
    }

    private var history: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("History")
                    .font(.system(size: 12, weight: .heavy))
                    .foregroundStyle(.secondary)
                Spacer()
                Button("Refresh") { model.refreshRunHistory() }
                    .buttonStyle(.borderless)
            }

            if runs.isEmpty {
                Text("No run logs for this target yet.")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.secondary)
                    .padding(.vertical, 6)
            } else {
                List(runs) { run in
                    RunLogRow(run: run)
                }
                .listStyle(.inset)
            }
        }
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
                .listStyle(.inset)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}

private struct StatusMark: View {
    let color: Color

    var body: some View {
        ZStack {
            Circle()
                .fill(color)
                .opacity(0.20)
                .frame(width: 16, height: 16)
            Circle()
                .fill(color)
                .opacity(0.92)
                .frame(width: 8, height: 8)
        }
        .accessibilityHidden(true)
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
        if parts.isEmpty {
            return "—"
        }
        return parts.joined(separator: " · ")
    }
}
