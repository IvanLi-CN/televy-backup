import Foundation

/// Connectivity is useful diagnostics; only app-bound build evidence proves update readiness.
struct SnapshotMountHelperReadiness {
    let reachable: Bool
    let isReady: Bool
    let activeMounts: UInt32?
    let liveMounts: UInt32?
    let pendingCleanup: UInt32?
    let recoverableStaleMounts: UInt32?
    let unknownBlockingMounts: UInt32?

    init(
        reachable: Bool,
        daemonReady: Bool,
        buildCommit: String?,
        expectedBuildCommit: String?,
        activeMounts: UInt32? = nil,
        liveMounts: UInt32? = nil,
        pendingCleanup: UInt32? = nil,
        recoverableStaleMounts: UInt32? = nil,
        unknownBlockingMounts: UInt32? = nil
    ) {
        self.reachable = reachable
        self.activeMounts = activeMounts
        self.liveMounts = liveMounts
        self.pendingCleanup = pendingCleanup
        self.recoverableStaleMounts = recoverableStaleMounts
        self.unknownBlockingMounts = unknownBlockingMounts
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
        guard reachable && !isReady else { return nil }
        return "The installed helper has not been verified for this app. Update it to use the latest fixes."
    }

    var classificationKnown: Bool {
        liveMounts != nil && pendingCleanup != nil && recoverableStaleMounts != nil && unknownBlockingMounts != nil
    }

    var hasBlockingRecords: Bool {
        [liveMounts, pendingCleanup, recoverableStaleMounts, unknownBlockingMounts]
            .compactMap { $0 }
            .contains { $0 > 0 }
    }

    var canUpdate: Bool {
        guard reachable else { return false }
        if classificationKnown {
            return !hasBlockingRecords
        }
        // Legacy helpers do not report the additive classification fields. Their
        // aggregate counters are still safe to use as a conservative update gate.
        return activeMounts == 0 && pendingCleanup == 0
    }
}
