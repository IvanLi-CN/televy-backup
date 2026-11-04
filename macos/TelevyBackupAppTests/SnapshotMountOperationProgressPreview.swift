import AppKit
import SwiftUI

/// Synthetic evidence rendered from the production progress component; no IPC or user data.
@main
enum SnapshotMountOperationProgressPreview {
    @MainActor static func main() throws {
        let output = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        for (name, scheme) in [("light", ColorScheme.light), ("dark", ColorScheme.dark)] {
            let content = VStack(alignment: .leading, spacing: 18) {
                Text("Snapshot inventory reconciliation").font(.headline)
                Text("While the background operation is running").font(.subheadline)
                SnapshotStatusFreshnessView(lastUpdatedAt: Date(timeIntervalSince1970: 1_700_000_000), isRefreshing: false)
                SnapshotMountOperationProgressView(label: "Snapshot inventory reconciliation", isObserving: true, resume: {})
                HStack {
                    Button("Reconcile snapshot inventory", action: {}).disabled(true)
                    Button("Check operation status", action: {})
                }.buttonStyle(.bordered)
                Divider()
                Text("If the status connection is interrupted").font(.subheadline)
                SnapshotMountOperationProgressView(label: "Snapshot inventory reconciliation", isObserving: false, resume: {})
                Divider()
                Text("After the refreshed status reaches zero").font(.subheadline)
                SnapshotStatusFreshnessView(lastUpdatedAt: Date(timeIntervalSince1970: 1_700_000_020), isRefreshing: false)
                SnapshotMountHelperStatusView(
                    readiness: SnapshotMountHelperReadiness(
                        reachable: true, daemonReady: true,
                        buildCommit: String(repeating: "a", count: 40),
                        expectedBuildCommit: String(repeating: "a", count: 40), helperVersion: "0.5.7",
                        activeMounts: 0, liveMounts: 0, pendingCleanup: 0,
                        recoverableStaleMounts: 0, unknownBlockingMounts: 0,
                        inventoryDriftMounts: 0, pendingCleanupCandidates: 0
                    ),
                    path: nil, version: "0.5.7", error: nil, activeMounts: 0,
                    pendingCleanup: 0, pendingCleanupCandidates: 0, liveMounts: 0,
                    recoverableStaleMounts: 0, unknownBlockingMounts: 0,
                    inventoryDriftMounts: 0, busy: false,
                    install: {}, recover: {}, retryCleanup: {}, reconcileInventory: { _ in }
                )
            }
            .padding(24)
            .frame(width: 650)
            .background(scheme == .dark ? Color(red: 0.16, green: 0.17, blue: 0.19) : Color.white)
            .environment(\.colorScheme, scheme)
            .environment(\.locale, Locale(identifier: "en_US"))
            .environment(\.timeZone, TimeZone(secondsFromGMT: 0)!)
            let host = NSHostingView(rootView: content)
            host.frame = NSRect(origin: .zero, size: host.fittingSize)
            host.layoutSubtreeIfNeeded()
            guard let bitmap = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { exit(1) }
            host.cacheDisplay(in: host.bounds, to: bitmap)
            guard let png = bitmap.representation(using: .png, properties: [:]) else { exit(1) }
            try png.write(to: output.appendingPathComponent("snapshot-operation-\(name).png"))
        }
    }
}
