import Foundation

/// Connectivity is useful diagnostics; only app-bound build evidence proves update readiness.
struct SnapshotMountHelperReadiness {
    let reachable: Bool
    let isReady: Bool

    init(reachable: Bool, daemonReady: Bool, buildCommit: String?, expectedBuildCommit: String?) {
        self.reachable = reachable
        let validExpected = expectedBuildCommit.map {
            $0.utf8.count == 40 && $0.utf8.allSatisfy { (48...57).contains($0) || (65...70).contains($0) || (97...102).contains($0) }
        } ?? false
        isReady = reachable && daemonReady && validExpected && buildCommit == expectedBuildCommit
    }

    var title: String {
        if isReady { return "Snapshot mount helper ready" }
        return reachable ? "Snapshot mount helper update required" : "Snapshot mount helper required"
    }

    var actionTitle: String? {
        if isReady { return nil }
        return reachable ? "Update mount helper" : "Install mount helper"
    }

    var updateExplanation: String? {
        reachable && !isReady
            ? "The installed helper has not been verified for this app. Update it to use the latest fixes."
            : nil
    }

    /// The root helper's legacy-compatible aggregate is blocking even when the
    /// optional cleanup breakdown is unavailable. Never offer an update while
    /// that aggregate still contains a journal row.
    static func updateBlockReason(activeMounts: UInt32, pendingCleanup: UInt32) -> String? {
        guard activeMounts > 0 || pendingCleanup > 0 else { return nil }
        if pendingCleanup > 0 {
            let mountDescription = activeMounts == pendingCleanup
                ? "All reported snapshot mount records are pending cleanup; strict mode remains blocked until the recorded snapshot inventory is reconciled."
                : "Additional snapshot mount record(s) are also blocking the update; wait for the backup or cleanup to finish."
            return "\(pendingCleanup) snapshot cleanup record(s) are pending. \(mountDescription)"
        }
        return "\(activeMounts) snapshot mount record(s) are still blocking the update. Finish the backup or wait for cleanup before updating the helper."
    }

    static func canUpdate(activeMounts: UInt32, pendingCleanup: UInt32) -> Bool {
        updateBlockReason(activeMounts: activeMounts, pendingCleanup: pendingCleanup) == nil
    }
}
