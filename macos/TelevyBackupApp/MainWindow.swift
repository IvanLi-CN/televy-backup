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
                    .frame(minWidth: 200, idealWidth: 300, maxWidth: 460)
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
    let target: StatusTarget
    let isSelected: Bool
    let isBusy: Bool
    let onRestore: () -> Void
    let onVerify: () -> Void
    let onSelect: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            VStack(alignment: .leading, spacing: 3) {
                Text(target.label ?? target.targetId)
                    .font(.system(size: 12, weight: .semibold, design: .monospaced))
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(target.sourcePath)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            HStack(spacing: 8) {
                TargetRowActionButton(
                    title: "Restore…",
                    systemImage: "arrow.down.doc",
                    isSelected: isSelected,
                    isBusy: isBusy,
                    action: onRestore
                )

                TargetRowActionButton(
                    title: "Verify",
                    systemImage: "checkmark.seal",
                    isSelected: isSelected,
                    isBusy: isBusy,
                    action: onVerify
                )

                Spacer(minLength: 0)
            }
        }
        .contentShape(Rectangle())
        .onTapGesture { onSelect() }
        .padding(.vertical, 2)
    }
}

private struct TargetRowActionButton: View {
    let title: String
    let systemImage: String
    let isSelected: Bool
    let isBusy: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Label(title, systemImage: systemImage)
                .labelStyle(.titleOnly)
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(isSelected ? Color.white.opacity(isBusy ? 0.5 : 0.95) : Color.primary)
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(
                    Group {
                        if isSelected {
                            RoundedRectangle(cornerRadius: 7, style: .continuous)
                                .fill(Color.white.opacity(0.20))
                                .overlay(
                                    RoundedRectangle(cornerRadius: 7, style: .continuous)
                                        .strokeBorder(Color.white.opacity(0.28), lineWidth: 1)
                                )
                        } else {
                            RoundedRectangle(cornerRadius: 7, style: .continuous)
                                .fill(Color.white.opacity(0.12))
                                .overlay(
                                    RoundedRectangle(cornerRadius: 7, style: .continuous)
                                        .strokeBorder(Color.black.opacity(0.10), lineWidth: 1)
                                )
                        }
                    }
                )
        }
        .buttonStyle(.plain)
        .disabled(isBusy)
    }
}

private struct TargetDetailView: View {
    @EnvironmentObject var model: AppModel
    let target: StatusTarget

    private var runs: [RunLogSummary] {
        model.runHistory
            .filter { run in run.targetId == target.targetId }
            .sorted {
                let a = $0.finishedAt ?? $0.startedAt ?? .distantPast
                let b = $1.finishedAt ?? $1.startedAt ?? .distantPast
                return a > b
            }
    }

    private var effectiveTargetState: String {
        if target.state == "running" { return "running" }
        // Verify/restore runs are executed by the CLI and may not reflect in the daemon status
        // stream immediately; infer "running" from the presence of an in-progress run log.
        if runs.contains(where: { $0.status == "running" }) { return "running" }
        return target.state
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            Divider()
            history
        }
        .padding(16)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(target.label ?? target.targetId)
                .font(.system(size: 20, weight: .bold))
            Text(target.targetId)
                .font(.system(size: 12, design: .monospaced))
                .foregroundStyle(.secondary)
            Text(target.sourcePath)
                .font(.system(size: 12, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .truncationMode(.middle)

            HStack(spacing: 10) {
                StatusChip(text: target.enabled ? "Enabled" : "Disabled", tint: target.enabled ? .green : .gray)
                StatusChip(text: effectiveTargetState, tint: effectiveTargetState == "running" ? .blue : .gray)
                if let status = target.lastRun?.status {
                    StatusChip(text: "Last: \(status)", tint: status == "succeeded" ? .green : .red)
                }
                Spacer()
                Button("Restore…") { model.promptRestoreLatest(targetId: target.targetId) }
                    .buttonStyle(.borderedProminent)
                    .disabled(model.isRunning)
                Button("Verify") { model.verifyLatest(targetId: target.targetId) }
                    .buttonStyle(.bordered)
                    .disabled(model.isRunning)
            }
        }
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

private struct UnknownTargetListRow: View {
    let isSelected: Bool
    let count: Int
    let onSelect: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text("Unknown target")
                    .font(.system(size: 12, weight: .semibold, design: .monospaced))
                    .lineLimit(1)
                Text("\(count) run(s)")
                    .font(.system(size: 11, design: .monospaced))
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

private struct StatusChip: View {
    let text: String
    let tint: Color

    var body: some View {
        Text(text)
            .font(.system(size: 11, weight: .semibold))
            .foregroundStyle(Color.white)
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(tint.opacity(0.92), in: Capsule())
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
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            Spacer(minLength: 8)

            HStack(alignment: .center, spacing: 8) {
                if let at = run.finishedAt ?? run.startedAt {
                    Text(at.formatted(date: .abbreviated, time: .standard))
                        .font(.system(size: 11, design: .monospaced))
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
        if let d = r.durationSeconds {
            parts.append(String(format: "dur=%.1fs", d))
        }
        if let e = r.errorCode, !e.isEmpty {
            parts.append("err=\(e)")
        }
        if let b = r.bytesUploaded {
            parts.append("up=\(b)")
        }
        if let b = r.bytesWritten {
            parts.append("written=\(b)")
        }
        if let b = r.bytesChecked {
            parts.append("checked=\(b)")
        }
        if let s = r.snapshotId, !s.isEmpty {
            parts.append("snap=\(s)")
        }
        return parts.joined(separator: " ")
    }
}
