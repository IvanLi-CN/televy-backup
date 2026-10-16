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
}
