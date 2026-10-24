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
        // Install remains actionable when the helper is unreachable. The
        // privileged install transaction performs its own journal/identity
        // checks and fails closed if records are active or ambiguous.
        guard reachable else { return true }
        if classificationKnown {
            return !hasBlockingRecords
        }
        // Legacy helpers do not report the additive classification fields. Their
        // aggregate counters are still safe to use as a conservative update gate.
        return activeMounts == 0 && pendingCleanup == 0
    }

    /// The root helper's legacy-compatible aggregate is blocking even when the
    /// optional cleanup breakdown is unavailable. Never offer an update while
    /// that aggregate still contains a journal row.
    static func updateBlockReason(activeMounts: UInt32?, pendingCleanup: UInt32?) -> String? {
        let activeMounts = activeMounts ?? 0
        let pendingCleanup = pendingCleanup ?? 0
        guard activeMounts > 0 || pendingCleanup > 0 else { return nil }
        if pendingCleanup > 0 {
            let mountDescription = activeMounts == pendingCleanup
                ? "All reported snapshot mount records are pending cleanup; strict mode remains blocked until the recorded snapshot inventory is reconciled."
                : "Additional snapshot mount record(s) are also blocking the update; wait for the backup or cleanup to finish."
            return "\(pendingCleanup) snapshot cleanup record(s) are pending. \(mountDescription)"
        }
        return "\(activeMounts) snapshot mount record(s) are still blocking the update. Finish the backup or wait for cleanup before updating the helper."
    }

    static func canUpdate(activeMounts: UInt32?, pendingCleanup: UInt32?) -> Bool {
        updateBlockReason(activeMounts: activeMounts, pendingCleanup: pendingCleanup) == nil
    }
}
