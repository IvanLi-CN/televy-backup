import SwiftUI

struct SnapshotMountOperationProgressView: View {
    let label: String
    let isObserving: Bool
    let resume: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            if isObserving {
                ProgressView().controlSize(.small)
            } else {
                Image(systemName: "pause.circle").foregroundStyle(.orange)
            }
            VStack(alignment: .leading, spacing: 5) {
                Text(isObserving ? "\(label) in progress…" : "Operation status check paused")
                    .font(.system(size: 12, weight: .semibold))
                Text(isObserving
                     ? "Waiting for the background operation to finish. Snapshot changes are disabled to prevent duplicate requests."
                     : "The background operation may still be running. Resume checking its status; do not start it again.")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if !isObserving {
                    Button("Resume status check", action: resume).buttonStyle(.bordered)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(Color.accentColor.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("snapshot-operation-progress")
    }
}

struct SnapshotStatusFreshnessView: View {
    let lastUpdatedAt: Date?
    let isRefreshing: Bool
    var error: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let error {
                Label("Status refresh failed. Previous values may be out of date. " + error,
                      systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack(spacing: 6) {
                if isRefreshing {
                    ProgressView().controlSize(.mini)
                    Text("Refreshing snapshot status…")
                } else if let lastUpdatedAt {
                    Text("Last checked")
                    Text(lastUpdatedAt, style: .time).monospacedDigit()
                } else {
                    Text("Snapshot status has not been checked yet")
                }
            }
        }
        .font(.system(size: 11))
        .foregroundStyle(.secondary)
        .accessibilityIdentifier("snapshot-status-freshness")
    }
}
