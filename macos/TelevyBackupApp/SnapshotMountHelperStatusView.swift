import SwiftUI

/// Shared by Settings and the synthetic CI preview; never installs during status refresh.
struct SnapshotMountHelperStatusView: View {
    let readiness: SnapshotMountHelperReadiness
    let path: String?
    let version: String?
    let error: String?
    let activeMounts: UInt32?
    let pendingCleanup: UInt32?
    let liveMounts: UInt32?
    let recoverableStaleMounts: UInt32?
    let unknownBlockingMounts: UInt32?
    let busy: Bool
    let install: () -> Void
    let recover: () -> Void

    var body: some View {
        let updateBlockReason = readiness.isLegacyAggregateStatus
            ? nil
            : SnapshotMountHelperReadiness.updateBlockReason(
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
            if let liveMounts, liveMounts > 0 {
                Text("\(liveMounts) active snapshot mount(s) are still running. Finish the backup before updating the helper.")
                    .font(.system(size: 11))
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let pendingCleanup, pendingCleanup > 0 {
                Text("\(pendingCleanup) snapshot cleanup record(s) are pending. Strict mode remains blocked until cleanup is complete.")
                    .font(.system(size: 11))
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let recoverableStaleMounts, recoverableStaleMounts > 0 {
                Text("\(recoverableStaleMounts) stale snapshot mount record(s) can be safely recovered.")
                    .font(.system(size: 11))
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
                Button("Recover stale mount records", action: recover)
                    .buttonStyle(.bordered)
                    .disabled(busy || !readiness.canRecoverStaleMounts)
            }
            if let unknownBlockingMounts, unknownBlockingMounts > 0 {
                Text("\(unknownBlockingMounts) snapshot record(s) cannot be verified safely. Open Diagnostics and finish or repair the owning run.")
                    .font(.system(size: 11))
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if !readiness.classificationKnown && readiness.isReady {
                Text("The installed helper did not report a complete mount status. Refresh diagnostics before changing it.")
                    .font(.system(size: 11))
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            } else if let updateBlockReason {
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
                    .disabled(busy || !readiness.canUpdate)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
