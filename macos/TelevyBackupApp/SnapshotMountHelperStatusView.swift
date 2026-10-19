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
        VStack(alignment: .leading, spacing: 6) {
            Label(readiness.title, systemImage: readiness.isReady ? "checkmark.shield.fill" : "exclamationmark.shield.fill")
                .foregroundStyle(readiness.isReady ? .green : .orange)
            if let path {
                Text(path)
                    .font(.system(size: 11, design: .monospaced))
                    .textSelection(.enabled)
            }
            if let version {
                Text("Version \(version)").font(.system(size: 11)).foregroundStyle(.secondary)
            }
            if pendingCleanup > 0 {
                let mountDescription = activeMounts == pendingCleanup
                    ? "No active mount is running, but strict mode remains blocked until the recorded snapshot inventory is reconciled."
                    : "An active snapshot mount is also present; wait for it to finish before updating the helper."
                Text("\(pendingCleanup) snapshot cleanup record(s) are pending. \(mountDescription)")
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
                    .disabled(busy)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
