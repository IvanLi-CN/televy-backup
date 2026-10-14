import Foundation
import SwiftUI

struct BackupFailurePresentation {
    let title: String
    let recovery: String
    let opensSettings: Bool

    init(code: String?, reason: String?) {
        opensSettings = code?.hasPrefix("snapshot.") == true || code == "config.invalid"
        switch reason {
        case "protocol_mismatch":
            title = "Snapshot components are incompatible"
            recovery = "Update TelevyBackup, restart it, then use Install mount helper in Settings → Snapshots."
        case "permission_denied":
            title = "Snapshot access was denied"
            recovery = "Review Full Disk Access for both components in Settings → Snapshots, then retry."
        case "service_unavailable":
            title = "Snapshot Access is unavailable"
            recovery = "Restart TelevyBackup and check Snapshot Access in Settings → Snapshots."
        case "mount_helper_unavailable":
            title = "The snapshot mount helper is unavailable"
            recovery = "Use Install mount helper in Settings → Snapshots, then retry."
        case "insufficient_space":
            title = "Not enough space to create a snapshot"
            recovery = "Free space on the source volume, then retry the backup."
        case "cleanup_pending":
            title = "Snapshot cleanup is incomplete"
            recovery = "Check pending cleanup in Settings → Snapshots. Open Diagnostics if it persists."
        case "request_timed_out":
            title = "Snapshot Access did not respond in time"
            recovery = "Check Snapshot Access in Settings → Snapshots, then retry."
        default:
            switch code {
            case "snapshot.acquire_failed":
                title = "Could not create the backup snapshot"
                recovery = "Check Snapshot Access and the mount helper in Settings → Snapshots. Open Diagnostics for details."
            case "snapshot.probe_failed":
                title = "Could not check the source volume"
                recovery = "Check that the source volume is available and verify it in Settings → Snapshots."
            case "snapshot.source_unavailable":
                title = "The source volume is unavailable"
                recovery = "Reconnect the source volume, then retry the backup."
            case "snapshot.access_failed":
                title = "Could not read the backup snapshot"
                recovery = "The strict backup stopped to preserve consistency. Retry; if it fails again, open Diagnostics."
            case "task.interrupted", "task.reporter_lost":
                title = "The run was interrupted"
                recovery = "The task ended without a completion record. Retry and open Diagnostics if it repeats."
            case "telegram.unauthorized", "telegram.mtproto.session_invalid":
                title = "Telegram authentication failed"
                recovery = "Review the Telegram credentials in Settings, then retry."
            case "config.invalid":
                title = "Backup configuration needs attention"
                recovery = "Review the target and storage settings before retrying."
            default:
                title = "The run failed"
                recovery = "Open Diagnostics or the run log for details, then retry."
            }
        }
    }

    /// Only infer fixed categories from legacy local diagnostics; never show the raw message.
    static func legacyReason(message: String?) -> String? {
        let message = message?.lowercased() ?? ""
        if message.contains("unsupported mount helper response version")
            || message.contains("unsupported access app response version")
            || message.contains("incompatible snapshot access component version")
            || message.contains("incompatible mount helper") { return "protocol_mismatch" }
        if message.contains("permission denied") || message.contains("operation not permitted")
            || message.contains("full disk access") { return "permission_denied" }
        if message.contains("mount helper unavailable") { return "mount_helper_unavailable" }
        if message.contains("free space") || message.contains("no space left") { return "insufficient_space" }
        if message.contains("cleanup") { return "cleanup_pending" }
        return nil
    }
}

struct BackupFailureNotice: View {
    let issue: BackupFailurePresentation
    let code: String?
    let openSettings: () -> Void
    let openDiagnostics: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label(issue.title, systemImage: "exclamationmark.triangle.fill")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(.red)
            Text(issue.recovery)
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 12) {
                if issue.opensSettings {
                    Button("Open Settings", action: openSettings)
                }
                Button("Open Diagnostics", action: openDiagnostics)
                if let code, !code.isEmpty {
                    Text(code)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                }
            }
            .font(.system(size: 11))
            .buttonStyle(.plain)
            .foregroundStyle(.blue)
        }
        .accessibilityElement(children: .contain)
    }
}
