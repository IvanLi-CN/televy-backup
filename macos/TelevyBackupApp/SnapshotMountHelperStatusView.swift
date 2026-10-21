import SwiftUI

/// Shared by Settings and the synthetic CI preview; never installs during status refresh.
struct SnapshotMountHelperStatusView: View {
    let readiness: SnapshotMountHelperReadiness
    let path: String?
    let version: String?
    let error: String?
    let activeMounts: UInt32
    let pendingCleanup: UInt32
    let busy: Bool
    let install: () -> Void

    var body: some View {
        let updateBlockReason = SnapshotMountHelperReadiness.updateBlockReason(
            activeMounts: activeMounts,
            pendingCleanup: pendingCleanup
        )
        VStack(alignment: .leading, spacing: 6) {
            Label(
                updateBlockReason != nil && !readiness.isReady
                    ? "Snapshot mount helper update waiting"
                    : readiness.title,
                systemImage: readiness.isReady ? "checkmark.shield.fill" : "exclamationmark.shield.fill"
            )
                .foregroundStyle(readiness.isReady ? .green : .orange)
            if let path {
                Text(path)
                    .font(.system(size: 11, design: .monospaced))
                    .textSelection(.enabled)
            }
            if let version {
                Text("Version \(version)").font(.system(size: 11)).foregroundStyle(.secondary)
            }
            if let updateBlockReason {
                Text(updateBlockReason)
                    .font(.system(size: 11))
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            } else if let explanation = readiness.updateExplanation {
                Text(explanation).font(.system(size: 11)).foregroundStyle(.secondary)
            } else if let error, !readiness.isReady {
                Text(error).font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(2)
            }
            if let actionTitle = readiness.actionTitle {
                Button(actionTitle, action: install)
                    .buttonStyle(.borderedProminent)
                    .disabled(busy || !SnapshotMountHelperReadiness.canUpdate(
                        activeMounts: activeMounts,
                        pendingCleanup: pendingCleanup
                    ))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
