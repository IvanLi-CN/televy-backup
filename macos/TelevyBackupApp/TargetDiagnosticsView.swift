import Foundation
import SwiftUI

struct TargetDiagnosticsView: View {
    @EnvironmentObject var model: AppModel
    let target: StatusTarget

    @State private var activityFilter: String = ""

    private static let isoFormatter: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                globalCard
                targetDetailsCard
                activityCard
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var globalCard: some View {
        GlassCard(title: "GLOBAL") {
            let nowMs = Int64(Date().timeIntervalSince1970 * 1000.0)
            let snap = model.statusSnapshot

            keyValue("schemaVersion", snap.map { String($0.schemaVersion) } ?? "—")
            keyValue("generatedAt", snap.map { isoFromMs($0.generatedAt) } ?? "—")
            keyValue("receivedAt", isoFromDate(model.statusSnapshotReceivedAt) ?? "—")
            keyValue("source", sourceText(snap))
            keyValue("freshness", freshnessText(snap: snap, nowMs: nowMs))

            let up = snap?.global.up.bytesPerSecond.map { "\(formatBytes($0))/s" } ?? "—"
            let down = snap?.global.down.bytesPerSecond.map { "\(formatBytes($0))/s" } ?? "—"
            keyValue("rates", "up=\(up) down=\(down)")

            let upTot = snap?.global.upTotal.bytes.map { formatBytes($0) } ?? "—"
            let downTot = snap?.global.downTotal.bytes.map { formatBytes($0) } ?? "—"
            keyValue("session totals", "up=\(upTot) down=\(downTot)")
        }
    }

    private var targetDetailsCard: some View {
        GlassCard(title: "TARGET DETAILS") {
            let nowMs = Int64(Date().timeIntervalSince1970 * 1000.0)

            keyValue("targetId", target.targetId)
            keyValue("endpointId", target.endpointId)
            keyValue("enabled", target.enabled ? "true" : "false")
            keyValue("state", target.state)
            keyValue("phase", target.progress?.phase ?? "—")

            if let p = target.progress {
                let filesTotal = p.sourceFilesTotal ?? p.filesTotal
                keyValue(
                    "progress",
                    "chunks \(p.chunksDone ?? -1)/\(p.chunksTotal ?? -1)  files \(p.filesDone ?? -1)/\(filesTotal ?? -1)"
                )
                keyValue("bytesRead", p.bytesRead.map(String.init) ?? "—")
                keyValue(
                    "bytesUploaded",
                    "current=\(p.bytesUploaded.map(String.init) ?? "—")  source=\(p.bytesUploadedSource.map(String.init) ?? "—")  confirmed=\(p.bytesUploadedConfirmed.map(String.init) ?? "—")"
                )
                keyValue("bytesDownloaded", p.bytesDownloaded.map(String.init) ?? "—")
                keyValue("bytesDeduped", p.bytesDeduped.map(String.init) ?? "—")
            } else {
                keyValue("progress", "—")
            }

            if let since = target.runningSince {
                let elapsedSeconds = max(0, (nowMs - since)) / 1000
                keyValue("runningSince", "\(since)  elapsed=\(formatDuration(Double(elapsedSeconds)))")
            } else {
                keyValue("runningSince", "—")
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
    }

    private var activityCard: some View {
        GlassCard(title: "ACTIVITY") {
            HStack {
                Spacer()
                TextField("filter: target", text: $activityFilter)
                    .textFieldStyle(.roundedBorder)
                    .font(.system(size: 12, weight: .semibold, design: .monospaced))
                    .frame(width: 240)
            }
            Divider().opacity(0.25)

            let items = filteredActivityItems()
            if items.isEmpty {
                Text(activityFilter.isEmpty ? "No activity for this target yet." : "No matching activity.")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .padding(.vertical, 4)
            } else {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(items) { item in
                        Text("\(isoFromDate(item.at))  \(item.text)")
                            .font(.system(size: 11, weight: .semibold, design: .monospaced))
                            .foregroundStyle(.primary.opacity(0.9))
                    }
                }
            }
        }
    }

    private func filteredActivityItems() -> [AppModel.StatusActivityItem] {
        let all = Array(model.statusActivity.suffix(200).reversed())
        let needle = activityFilter.trimmingCharacters(in: .whitespacesAndNewlines)
        if !needle.isEmpty {
            return all.filter { $0.text.localizedCaseInsensitiveContains(needle) }
        }
        return all.filter { $0.text.contains(target.targetId) }
    }

    private func sourceText(_ snap: StatusSnapshot?) -> String {
        guard let snap else { return "—" }
        if let detail = snap.source.detail, !detail.isEmpty {
            return "\(snap.source.kind) (\(detail))"
        }
        return snap.source.kind
    }

    private func freshnessText(snap: StatusSnapshot?, nowMs: Int64) -> String {
        guard let snap else { return "disconnected" }
        let ageMs = max(0, nowMs - snap.generatedAt)
        let isDaemon = (snap.source.kind == "daemon")

        let status: String
        if !isDaemon { status = "stale" }
        else if ageMs > StatusFreshness.disconnectedMs { status = "disconnected" }
        else if ageMs > StatusFreshness.staleMs { status = "stale" }
        else { status = "live" }

        return "\(status)  age=\(formatAge(ms: ageMs))"
    }

    private func formatAge(ms: Int64) -> String {
        if ms < 1_000 { return "\(ms)ms" }
        if ms < 60_000 { return "\(ms / 1000)s" }
        if ms < 3_600_000 { return "\(ms / 60_000)m" }
        return "\(ms / 3_600_000)h"
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
        Self.isoFormatter.string(from: date)
    }

    private func isoFromDate(_ date: Date?) -> String? {
        guard let date else { return nil }
        return isoFromDate(date)
    }
}
