import Foundation

/// Connectivity is useful diagnostics; only app-bound build evidence proves update readiness.
struct SnapshotMountHelperReadiness {
    let reachable: Bool
    let isReady: Bool
    let helperVersion: String?
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
        helperVersion: String? = nil,
        activeMounts: UInt32? = nil,
        liveMounts: UInt32? = nil,
        pendingCleanup: UInt32? = nil,
        recoverableStaleMounts: UInt32? = nil,
        unknownBlockingMounts: UInt32? = nil
    ) {
        self.reachable = reachable
        self.helperVersion = helperVersion
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
        if isLegacyAggregateStatus {
            let count = activeMounts.map(String.init) ?? "one or more"
            return "The installed helper cannot classify \(count) legacy snapshot mount record(s). Update will verify Snapshot Access is idle before replacing it."
        }
        return "The installed helper has not been verified for this app. Update it to use the latest fixes."
    }

    /// The 0.5.0 and 0.9.8 helpers expose only the aggregate mount count. Their
    /// replacement is safe to offer because the CLI rechecks Snapshot Access
    /// leases before booting out the old helper and fails closed on active work.
    var isLegacyAggregateStatus: Bool {
        guard reachable,
              let helperVersion,
              helperVersion == "0.5.0" || helperVersion == "0.9.8",
              activeMounts != nil,
              pendingCleanup == nil,
              liveMounts == nil,
              recoverableStaleMounts == nil,
              unknownBlockingMounts == nil
        else { return false }
        return true
    }

    var classificationKnown: Bool {
        liveMounts != nil && pendingCleanup != nil && recoverableStaleMounts != nil && unknownBlockingMounts != nil
    }

    var hasBlockingRecords: Bool {
        [activeMounts, liveMounts, pendingCleanup, recoverableStaleMounts, unknownBlockingMounts]
            .compactMap { $0 }
            .contains { $0 > 0 }
    }

    var canUpdate: Bool {
        // Install remains actionable when the helper is unreachable. The
        // privileged install transaction performs its own journal/identity
        // checks and fails closed if records are active or ambiguous.
        guard reachable else { return true }
        if classificationKnown {
            guard activeMounts != nil else { return false }
            return !hasBlockingRecords
        }
        if isLegacyAggregateStatus {
            return true
        }
        // An unrecognized helper that omits classification remains conservative.
        // A missing pending-cleanup field is compatible with a zero aggregate;
        // a reported cleanup row remains an explicit blocker even if the legacy
        // aggregate is inconsistent with it.
        return activeMounts == 0 && (pendingCleanup.map { $0 == 0 } ?? true)
    }

    var canRecoverStaleMounts: Bool {
        guard isReady,
              classificationKnown,
              let activeMounts,
              let liveMounts,
              let pendingCleanup,
              let recoverableStaleMounts,
              let unknownBlockingMounts
        else { return false }
        return recoverableStaleMounts > 0
            && liveMounts == 0
            && pendingCleanup == 0
            && unknownBlockingMounts == 0
            && activeMounts == recoverableStaleMounts
    }

    /// An unrecognized aggregate-only status is blocking even when the optional
    /// cleanup breakdown is unavailable. Recognized legacy versions are handled
    /// by the guarded replacement path above.
    static func updateBlockReason(activeMounts: UInt32?, pendingCleanup: UInt32?) -> String? {
        guard let activeMounts else {
            return "The installed helper did not report a complete cleanup status. Refresh diagnostics before changing it."
        }
        if let pendingCleanup, pendingCleanup > 0 {
            let mountDescription: String
            if activeMounts == pendingCleanup {
                mountDescription = "All reported snapshot mount records are pending cleanup; strict mode remains blocked until the recorded snapshot inventory is reconciled."
            } else if activeMounts == 0 {
                mountDescription = "The helper reports pending cleanup records while its aggregate is zero; strict mode remains blocked until the recorded snapshot inventory is reconciled."
            } else {
                mountDescription = "Additional snapshot mount record(s) are also blocking the update; wait for the backup or cleanup to finish."
            }
            return "\(pendingCleanup) snapshot cleanup record(s) are pending. \(mountDescription)"
        }
        if activeMounts == 0 {
            return nil
        }
        guard pendingCleanup != nil else {
            return "The installed helper reports \(activeMounts) blocking snapshot mount record(s), but cannot classify cleanup. Finish the backup or wait for cleanup before updating it."
        }
        return "\(activeMounts) snapshot mount record(s) are still blocking the update. Finish the backup or wait for cleanup before updating the helper."
    }

    static func canUpdate(activeMounts: UInt32?, pendingCleanup: UInt32?) -> Bool {
        updateBlockReason(activeMounts: activeMounts, pendingCleanup: pendingCleanup) == nil
    }
}
