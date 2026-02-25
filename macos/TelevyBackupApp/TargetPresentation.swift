import Foundation
import SwiftUI

enum TargetWorkKind: String {
    case backup
    case restore
    case verify
    case unknown
}

enum BackupProgressVisual {
    case indeterminate
    case determinate(scan: Double, success: Double)
}

struct TargetRateEstimate: Equatable {
    var uploadBytesPerSecond: Int64?
    var downloadBytesPerSecond: Int64?
    var updatedAt: Date
}

enum TargetUserStatus: String {
    case running
    case idle
    case failed
    case offline

    var title: String {
        switch self {
        case .running: return "Running"
        case .idle: return "Idle"
        case .failed: return "Failed"
        case .offline: return "Offline"
        }
    }

    var tint: Color {
        switch self {
        case .running: return .blue
        case .idle: return .gray
        case .failed: return .red
        case .offline: return .orange
        }
    }
}

enum TargetPresentation {
    private static let isoFormatter: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()

    private static let preparePhases: Set<String> = ["preflight", "prepare", "index_sync"]

    static func isPreparePhase(_ phase: String?) -> Bool {
        guard let phase else { return false }
        let normalized = phase.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if normalized.isEmpty { return false }
        return preparePhases.contains(normalized)
    }

    static func stageText(_ phase: String?) -> String? {
        guard let phase else { return nil }
        let p = phase.trimmingCharacters(in: .whitespacesAndNewlines)
        let normalized = p.lowercased()
        if p.isEmpty { return nil }
        switch normalized {
        case "preflight", "prepare", "index_sync": return "Preparing"
        case "scan": return "Scanning"
        case "upload": return "Uploading"
        case "index": return "Indexing"
        case "verify": return "Verifying"
        case "restore": return "Restoring"
        default: return p.prefix(1).uppercased() + p.dropFirst()
        }
    }

    static func workKind(activeKind: String?, runLogKind: String?, targetIsRunningInDaemon: Bool) -> TargetWorkKind {
        let candidates = [activeKind, runLogKind]
        for raw in candidates {
            guard let raw else { continue }
            let k = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            if k.isEmpty { continue }
            if k == "backup" { return .backup }
            if k == "restore" { return .restore }
            if k == "verify" { return .verify }
        }
        if targetIsRunningInDaemon { return .backup }
        return .unknown
    }

    static func snapshotIsOffline(snap: StatusSnapshot?, nowMs: Int64) -> Bool {
        guard let snap else { return true }
        if snap.source.kind != "daemon" { return true }
        let ageMs = max(0, nowMs - snap.generatedAt)
        return ageMs > StatusFreshness.staleMs
    }

    static func userStatus(
        target: StatusTarget,
        activeTask: AppModel.ActiveTask?,
        hasInProgressRunLog: Bool,
        snap: StatusSnapshot?,
        nowMs: Int64
    ) -> TargetUserStatus {
        let activeRunning = (activeTask?.state == "running") && (activeTask?.targetId == target.targetId)
        if activeRunning { return .running }

        if hasInProgressRunLog { return .running }

        let snapshotOffline = snapshotIsOffline(snap: snap, nowMs: nowMs)
        if snapshotOffline { return .offline }

        if target.state == "running" { return .running }

        if target.state == "failed" || target.lastRun?.status == "failed" { return .failed }
        if target.state == "stale" { return .offline }

        return .idle
    }

    static func progressFraction(_ p: StatusProgress?) -> Double? {
        guard let p else { return nil }
        if isPreparePhase(p.phase) {
            return nil
        }

        let normalizedPhase = p.phase.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let unstableTotalPhases: Set<String> = ["scan", "upload", "index", "index_sync"]

        if let done = p.chunksDone, let total = p.chunksTotal, total > 0 {
            if done == total && unstableTotalPhases.contains(normalizedPhase) {
                return nil
            }
            return min(1.0, Double(done) / Double(total))
        }

        if let done = p.filesDone, let total = p.filesTotal, total > 0 {
            if done == total && unstableTotalPhases.contains(normalizedPhase) {
                return nil
            }
            return min(1.0, Double(done) / Double(total))
        }

        return nil
    }

    static func backupProgressVisual(_ p: StatusProgress?) -> BackupProgressVisual {
        // UX contract: only prepare renders as indeterminate.
        guard let p else { return .determinate(scan: 0, success: 0) }
        if isPreparePhase(p.phase) {
            return .indeterminate
        }

        if let sourceBytesTotal = p.sourceBytesTotal, sourceBytesTotal > 0 {
            let total = Double(sourceBytesTotal)
            let uploaded = max(Int64(0), p.bytesUploaded ?? 0)
            let deduped = max(Int64(0), p.bytesDeduped ?? 0)
            let read = max(Int64(0), p.bytesRead ?? 0)
            let successBytes = uploaded > (Int64.max - deduped) ? Int64.max : (uploaded + deduped)
            let success = min(1.0, max(0.0, Double(successBytes) / total))
            let scan = min(1.0, max(0.0, Double(read) / total))
            // Keep the two metrics independent so the bar reflects actual scan/upload states.
            return .determinate(scan: scan, success: success)
        }

        if let fallback = progressFraction(p) {
            return .determinate(scan: fallback, success: fallback)
        }

        return .determinate(scan: 0, success: 0)
    }

    static func lastRunSummary(target: StatusTarget, now: Date) -> String? {
        guard let r = target.lastRun else { return nil }

        let status = (r.status?.isEmpty == false) ? (r.status ?? "unknown") : "unknown"
        let statusTitle = status.prefix(1).uppercased() + status.dropFirst()

        var parts: [String] = []
        if status == "failed" {
            if let code = r.errorCode, !code.isEmpty {
                parts.append("Last run: Failed (\(code))")
            } else {
                parts.append("Last run: Failed")
            }
        } else {
            parts.append("Last run: \(statusTitle)")
        }

        if let finishedAt = r.finishedAt, let d = parseIsoDate(finishedAt) {
            let ageSeconds = Int(now.timeIntervalSince(d))
            if ageSeconds >= 0 {
                let rel = formatRelativeSeconds(ageSeconds)
                parts.append(rel == "just now" ? rel : (rel + " ago"))
            }
        }

        if status != "failed" {
            if let uploaded = r.bytesUploaded, uploaded > 0 {
                parts.append("Uploaded \(formatBytes(uploaded))")
            }
            if let saved = r.bytesDeduped, saved > 0 {
                parts.append("Saved \(formatBytes(saved))")
            }
        }

        if let dur = r.durationSeconds, dur > 0 {
            parts.append(formatDuration(dur))
        }

        return parts.joined(separator: " · ")
    }

    static func lastRunCompact(target: StatusTarget, now: Date) -> String? {
        guard let r = target.lastRun else { return nil }

        let status = (r.status?.isEmpty == false) ? (r.status ?? "unknown") : "unknown"
        if status == "failed" {
            if let code = r.errorCode, !code.isEmpty {
                return "Last run: Failed (\(code))"
            }
            return "Last run: Failed"
        }

        var parts: [String] = ["Last run: \(status.prefix(1).uppercased() + status.dropFirst())"]

        if let finishedAt = r.finishedAt, let d = parseIsoDate(finishedAt) {
            let ageSeconds = Int(now.timeIntervalSince(d))
            if ageSeconds >= 0 {
                let rel = formatRelativeSeconds(ageSeconds)
                parts.append(rel == "just now" ? rel : (rel + " ago"))
            }
        }

        if let uploaded = r.bytesUploaded, uploaded > 0 {
            parts.append("+\(formatBytes(uploaded))")
        }

        if let dur = r.durationSeconds, dur > 0 {
            parts.append(formatDuration(dur))
        }

        return parts.joined(separator: " · ")
    }

    static func parseIsoDate(_ s: String) -> Date? {
        if let d = isoFormatter.date(from: s) { return d }
        let f2 = ISO8601DateFormatter()
        f2.formatOptions = [.withInternetDateTime]
        return f2.date(from: s)
    }

    static func formatRelativeSeconds(_ s: Int) -> String {
        if s < 5 { return "just now" }
        if s < 60 { return "\(s)s" }
        if s < 3600 { return "\(s / 60)m" }
        if s < 86400 { return "\(s / 3600)h" }
        return "\(s / 86400)d"
    }
}

struct BackupUnifiedProgressBar: View {
    let visual: BackupProgressVisual
    var tint: Color = .blue
    var height: CGFloat = 6

    var body: some View {
        switch visual {
        case .indeterminate:
            ProgressView()
                .progressViewStyle(.linear)
                .controlSize(.mini)
                .tint(tint.opacity(0.92))
        case let .determinate(scan, success):
            let scanFrac = min(1.0, max(0.0, scan))
            let successFrac = min(1.0, max(0.0, success))
            let track = RoundedRectangle(cornerRadius: height / 2, style: .continuous)
            let successHeight = max(2, height * 0.66)
            let successInset = (height - successHeight) / 2
            let scanColor = Color(red: 0.33, green: 0.76, blue: 1.0)
            let successColor = Color(red: 0.11, green: 0.47, blue: 0.98)

            ZStack(alignment: .leading) {
                track.fill(Color.primary.opacity(0.10))

                if scanFrac > 0 {
                    track.fill(scanColor.opacity(0.78))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .scaleEffect(x: CGFloat(scanFrac), y: 1, anchor: .leading)
                }

                if successFrac > 0 {
                    RoundedRectangle(cornerRadius: successHeight / 2, style: .continuous)
                        .fill(successColor.opacity(0.97))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .scaleEffect(x: CGFloat(successFrac), y: 1, anchor: .leading)
                        .padding(.vertical, successInset)
                }
            }
            .frame(height: height)
            .animation(.easeOut(duration: 0.18), value: scanFrac)
            .animation(.easeOut(duration: 0.18), value: successFrac)
        }
    }
}
