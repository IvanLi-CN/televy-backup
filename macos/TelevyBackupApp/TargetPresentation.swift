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
    case determinate(uploadWork: Double, uploadCurrent: Double, backedUp: Double, scanned: Double)
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
        case "scan_upload": return "Scanning + Uploading"
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
        // Do not treat run-history "running" rows as authoritative runtime state.
        // A crashed/aborted run can leave a stale running row and conflict with live daemon status.
        _ = hasInProgressRunLog

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
        let unstableTotalPhases: Set<String> = ["scan", "scan_upload", "upload", "index", "index_sync"]

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
        guard let p else { return .indeterminate }
        if isPreparePhase(p.phase) {
            return .indeterminate
        }

        if let fractions = backupFractions(p) {
            // Required semantics: UploadWork <= UploadCurrent <= BackedUp <= Scanned.
            return .determinate(
                uploadWork: fractions.uploadWork,
                uploadCurrent: fractions.uploadCurrent,
                backedUp: fractions.backedUp,
                scanned: fractions.scan
            )
        }

        if let fallback = progressFraction(p) {
            // Fallback indicates work has been scanned but exact backup accounting is unavailable.
            return .determinate(uploadWork: 0, uploadCurrent: 0, backedUp: 0, scanned: fallback)
        }

        return .indeterminate
    }

    static func backupFractions(_ p: StatusProgress?) -> (scan: Double, backedUp: Double, uploadWork: Double, uploadCurrent: Double)? {
        guard let fractions = BackupProgressProjection.compute(p) else { return nil }
        return (
            scan: fractions.scanned,
            backedUp: fractions.backedUp,
            uploadWork: fractions.needUploadConfirmed,
            uploadCurrent: fractions.uploadingCurrent
        )
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
    var height: CGFloat = 7

    var body: some View {
        switch visual {
        case .indeterminate:
            ProgressView()
                .progressViewStyle(.linear)
                .controlSize(.mini)
                .tint(tint.opacity(0.92))
        case let .determinate(uploadWork, uploadCurrent, backedUp, scanned):
            let uploadWorkFrac = min(1.0, max(0.0, uploadWork))
            let uploadCurrentFrac = max(uploadWorkFrac, min(1.0, max(0.0, uploadCurrent)))
            let backedUpFrac = max(uploadCurrentFrac, min(1.0, max(0.0, backedUp)))
            let scannedFrac = max(backedUpFrac, min(1.0, max(0.0, scanned)))
            let track = RoundedRectangle(cornerRadius: height / 2, style: .continuous)
            let scanColor = Color.gray.opacity(0.28)
            let backedUpColor = Color(red: 0.55, green: 0.78, blue: 0.98).opacity(0.92)
            let uploadCurrentColor = Color(red: 0.20, green: 0.58, blue: 0.95).opacity(0.95)
            let uploadWorkColor = tint.opacity(0.98)

            GeometryReader { geo in
                let width = geo.size.width
                ZStack(alignment: .leading) {
                    track.fill(Color.primary.opacity(0.10))

                    if scannedFrac > 0 {
                        RoundedRectangle(cornerRadius: height / 2, style: .continuous)
                            .fill(scanColor)
                            .frame(width: width * CGFloat(scannedFrac), height: height)
                    }

                    if backedUpFrac > 0 {
                        RoundedRectangle(cornerRadius: height / 2, style: .continuous)
                            .fill(backedUpColor)
                            .frame(width: width * CGFloat(backedUpFrac), height: height)
                    }

                    if uploadCurrentFrac > 0 {
                        RoundedRectangle(cornerRadius: height / 2, style: .continuous)
                            .fill(uploadCurrentColor)
                            .frame(width: width * CGFloat(uploadCurrentFrac), height: height)
                    }

                    if uploadWorkFrac > 0 {
                        RoundedRectangle(cornerRadius: height / 2, style: .continuous)
                            .fill(uploadWorkColor)
                            .frame(width: width * CGFloat(uploadWorkFrac), height: height)
                    }
                }
                .clipShape(track)
            }
            .frame(height: height)
            .animation(.easeOut(duration: 0.18), value: scannedFrac)
            .animation(.easeOut(duration: 0.18), value: backedUpFrac)
            .animation(.easeOut(duration: 0.18), value: uploadCurrentFrac)
            .animation(.easeOut(duration: 0.18), value: uploadWorkFrac)
        }
    }
}
