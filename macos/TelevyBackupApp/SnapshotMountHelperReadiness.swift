import Foundation

/// Connectivity is useful diagnostics; only app-bound build evidence proves update readiness.
struct SnapshotMountHelperReadiness {
    let reachable: Bool
    let isReady: Bool
    let buildCommit: String?
    let expectedBuildCommit: String?
    let helperVersion: String?
    let activeMounts: UInt32?
    let liveMounts: UInt32?
    let pendingCleanup: UInt32?
    let recoverableStaleMounts: UInt32?
    let unknownBlockingMounts: UInt32?
    let inventoryDriftMounts: UInt32?
    let pendingCleanupCandidates: UInt32?
    let snapshotAccessActiveLeases: UInt32
    let snapshotAccessPendingCleanup: UInt32

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
        unknownBlockingMounts: UInt32? = nil,
        inventoryDriftMounts: UInt32? = nil,
        pendingCleanupCandidates: UInt32? = nil,
        snapshotAccessActiveLeases: UInt32 = 0,
        snapshotAccessPendingCleanup: UInt32 = 0
    ) {
        self.reachable = reachable
        self.buildCommit = buildCommit
        self.expectedBuildCommit = expectedBuildCommit
        self.helperVersion = helperVersion
        self.activeMounts = activeMounts
        self.liveMounts = liveMounts
        self.pendingCleanup = pendingCleanup
        self.recoverableStaleMounts = recoverableStaleMounts
        self.unknownBlockingMounts = unknownBlockingMounts
        self.inventoryDriftMounts = inventoryDriftMounts
        self.pendingCleanupCandidates = pendingCleanupCandidates
        self.snapshotAccessActiveLeases = snapshotAccessActiveLeases
        self.snapshotAccessPendingCleanup = snapshotAccessPendingCleanup
        let validExpected = expectedBuildCommit.map {
            $0.utf8.count == 40 && $0.utf8.allSatisfy { (48...57).contains($0) || (65...70).contains($0) || (97...102).contains($0) }
        } ?? false
        isReady = reachable && daemonReady && validExpected && buildCommit == expectedBuildCommit
    }

    var title: String {
        if isReady && !classificationKnown {
            return "Snapshot mount helper verification required"
        }
        if isReady && snapshotAccessActiveLeases > 0 {
            return "Snapshot Access busy"
        }
        if isReady && snapshotAccessPendingCleanup > 0 {
            return "Snapshot Access cleanup required"
        }
        if isReady && (unknownBlockingMounts ?? 0) > 0 {
            return "Snapshot mount helper legacy repair required"
        }
        if isReady && (inventoryDriftMounts ?? 0) > 0 {
            return "Snapshot inventory reconciliation required"
        }
        if isReady && (recoverableStaleMounts ?? 0) > 0 {
            return "Snapshot mount helper recovery required"
        }
        if isReady && (pendingCleanup ?? 0) > 0 {
            return "Snapshot mount helper cleanup required"
        }
        if isReady { return "Snapshot mount helper ready" }
        return reachable ? "Snapshot mount helper update required" : "Snapshot mount helper required"
    }

    /// Build identity proves that the helper is the installed candidate; blocking journal rows
    /// still keep strict mode unavailable until their verified cleanup action completes.
    var isOperational: Bool {
        isReady
            && classificationKnown
            && activeMounts != nil
            && snapshotAccessActiveLeases == 0
            && snapshotAccessPendingCleanup == 0
            && !hasBlockingRecords
    }

    var actionTitle: String? {
        if isReady { return nil }
        return reachable ? "Update mount helper" : "Install mount helper"
    }

    var updateExplanation: String? {
        guard reachable && !isReady else { return nil }
        if isPendingCleanupUpgradeSafe {
            return "The installed helper build is stale, but Snapshot Access is idle and all blocking records are verified cleanup candidates. Update it to restore the cleanup action."
        }
        if isLegacyAggregateStatus {
            let count = activeMounts.map(String.init) ?? "one or more"
            return "The installed helper cannot classify \(count) legacy snapshot mount record(s). Update will verify Snapshot Access is idle before replacing it."
        }
        if isLegacyPendingCleanupStatus {
            return "The installed helper reports pending cleanup without lease candidates. Update it after Snapshot Access is idle to restore the safe cleanup action."
        }
        if isStaleCurrentPartialClassification {
            return "The installed helper is an older build that omits inventory-drift classification. Update it after Snapshot Access is idle so the latest helper can verify the preserved journal."
        }
        if isLegacyUnknownOnlyUpgradeSafe {
            return nil
        }
        return "The installed helper has not been verified for this app. Update it to use the latest fixes."
    }

    /// The 0.5.1, 0.5.0 and 0.9.8 helpers expose only the aggregate mount count. Their
    /// replacement is safe to offer because the CLI rechecks Snapshot Access
    /// leases before booting out the old helper and fails closed on active work.
    var isLegacyAggregateStatus: Bool {
        guard reachable,
              let helperVersion,
              helperVersion == "0.5.1" || helperVersion == "0.5.0" || helperVersion == "0.9.8",
              activeMounts != nil,
              pendingCleanup == nil,
              liveMounts == nil,
              recoverableStaleMounts == nil,
              unknownBlockingMounts == nil
        else { return false }
        return true
    }

    /// Older helpers can identify pending rows but cannot expose their lease IDs. A guarded
    /// replacement is the only safe way to upgrade them before cleanup can be retried explicitly.
    var isLegacyPendingCleanupStatus: Bool {
        guard reachable,
              helperVersion == "0.5.3" || helperVersion == "0.5.2" || helperVersion == "0.5.1",
              let buildCommit
        else { return false }
        let validBuild = buildCommit.utf8.count == 40
            && buildCommit.utf8.allSatisfy {
                (48...57).contains($0) || (65...70).contains($0) || (97...102).contains($0)
            }
        guard validBuild,
              let activeMounts,
              let pendingCleanup,
              let pendingCleanupCandidates,
              let liveMounts,
              let recoverableStaleMounts,
              let unknownBlockingMounts,
              pendingCleanupCandidates == 0,
              activeMounts > 0,
              activeMounts == pendingCleanup,
              liveMounts == 0,
              recoverableStaleMounts == 0,
              unknownBlockingMounts == 0
        else { return false }
        return true
    }

    /// Current and recent helpers reported the four original classification partitions but did not
    /// report inventory drift. A mismatched build may be replaced only when those partitions form
    /// a complete aggregate; the CLI repeats the Snapshot Access idle check before bootout.
    var isStaleCurrentPartialClassification: Bool {
        guard reachable,
              !isReady,
              helperVersion == "0.5.6" || helperVersion == "0.5.5" || helperVersion == "0.5.4" || helperVersion == "0.5.3" || helperVersion == "0.5.2" || helperVersion == "0.5.1",
              let buildCommit,
              buildCommit != expectedBuildCommit,
              buildCommit.utf8.count == 40,
              buildCommit.utf8.allSatisfy({
                  (48...57).contains($0) || (65...70).contains($0) || (97...102).contains($0)
              }),
              let activeMounts,
              let pendingCleanup,
              let liveMounts,
              let recoverableStaleMounts,
              let unknownBlockingMounts,
              inventoryDriftMounts == nil,
              liveMounts == 0
        else { return false }
        let (pendingLive, pendingLiveOverflow) = pendingCleanup.addingReportingOverflow(liveMounts)
        let (withRecoverable, recoverableOverflow) = pendingLive.addingReportingOverflow(recoverableStaleMounts)
        let (total, unknownOverflow) = withRecoverable.addingReportingOverflow(unknownBlockingMounts)
        return !pendingLiveOverflow
            && !recoverableOverflow
            && !unknownOverflow
            && total == activeMounts
    }

    /// A 0.5.x helper may report a complete partition while classifying every blocking row as
    /// unknown. The rows remain untouched; only the guarded helper replacement is enabled so the
    /// current helper can expose the explicit repair action.
    var isLegacyUnknownOnlyUpgradeSafe: Bool {
        guard reachable,
              !isReady,
              helperVersion == "0.5.6" || helperVersion == "0.5.5" || helperVersion == "0.5.4" || helperVersion == "0.5.3" || helperVersion == "0.5.2" || helperVersion == "0.5.1",
              let buildCommit,
              buildCommit != expectedBuildCommit,
              buildCommit.utf8.count == 40,
              buildCommit.utf8.allSatisfy({
                  (48...57).contains($0) || (65...70).contains($0) || (97...102).contains($0)
              }),
              classificationKnown,
              let activeMounts,
              let pendingCleanup,
              let pendingCleanupCandidates,
              let liveMounts,
              let recoverableStaleMounts,
              let unknownBlockingMounts,
              let inventoryDriftMounts
        else { return false }
        return activeMounts > 0
            && pendingCleanup == 0
            && pendingCleanupCandidates == 0
            && liveMounts == 0
            && recoverableStaleMounts == 0
            && unknownBlockingMounts == activeMounts
            && inventoryDriftMounts == 0
            && snapshotAccessActiveLeases == 0
            && snapshotAccessPendingCleanup == 0
    }

    var classificationKnown: Bool {
        liveMounts != nil && pendingCleanup != nil && recoverableStaleMounts != nil && unknownBlockingMounts != nil && inventoryDriftMounts != nil
    }

    var hasBlockingRecords: Bool {
        [activeMounts, liveMounts, pendingCleanup, recoverableStaleMounts, unknownBlockingMounts, inventoryDriftMounts]
            .compactMap { $0 }
            .contains { $0 > 0 }
    }

    var canUpdate: Bool {
        // Install remains actionable when the helper is unreachable. The
        // privileged install transaction performs its own journal/identity
        // checks and fails closed if records are active or ambiguous.
        guard reachable else { return true }
        // Helper replacement and removal mutate the same system boundary as Snapshot Access.
        // Keep the action disabled while a lease or cleanup retry is active; the CLI repeats
        // this check immediately before bootout so a stale UI cannot bypass the boundary.
        guard snapshotAccessActiveLeases == 0 && snapshotAccessPendingCleanup == 0 else { return false }
        if isLegacyPendingCleanupStatus || isPendingCleanupUpgradeSafe || isStaleCurrentPartialClassification || isLegacyUnknownOnlyUpgradeSafe { return true }
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

    /// A mismatched current helper can be replaced without releasing anything when its complete
    /// status proves that every blocker is already cleanup-pending and has an exact lease record.
    /// The replacement is still serialized through the CLI's Snapshot Access idle check.
    var isPendingCleanupUpgradeSafe: Bool {
        guard reachable,
              !isReady,
              helperVersion == "0.5.6" || helperVersion == "0.5.5" || helperVersion == "0.5.4",
              classificationKnown,
              let activeMounts,
              let liveMounts,
              let pendingCleanup,
              let recoverableStaleMounts,
              let unknownBlockingMounts,
              let pendingCleanupCandidates,
              let inventoryDriftMounts
        else { return false }
        return pendingCleanup > 0
            && activeMounts == pendingCleanup
            && liveMounts == 0
            && recoverableStaleMounts == 0
            && unknownBlockingMounts == 0
            && inventoryDriftMounts == 0
            && pendingCleanupCandidates == pendingCleanup
            && snapshotAccessActiveLeases == 0
            && snapshotAccessPendingCleanup == 0
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
            && (inventoryDriftMounts ?? 0) == 0
            && snapshotAccessActiveLeases == 0
            && snapshotAccessPendingCleanup == 0
            && activeMounts == recoverableStaleMounts
    }

    /// Legacy rows are actionable only when the helper has classified every other partition as
    /// empty. The daemon repeats this predicate under its maintenance lock; the UI never mutates
    /// journal state during Refresh.
    var canRepairLegacyRecords: Bool {
        guard isReady,
              classificationKnown,
              let activeMounts,
              let liveMounts,
              let pendingCleanup,
              let recoverableStaleMounts,
              let unknownBlockingMounts,
              let inventoryDriftMounts
        else { return false }
        return unknownBlockingMounts > 0
            && activeMounts == unknownBlockingMounts
            && liveMounts == 0
            && pendingCleanup == 0
            && recoverableStaleMounts == 0
            && inventoryDriftMounts == 0
            && snapshotAccessActiveLeases == 0
            && snapshotAccessPendingCleanup == 0
    }

    var canReconcileInventory: Bool {
        guard isReady,
              classificationKnown,
              let activeMounts,
              let liveMounts,
              let pendingCleanup,
              let recoverableStaleMounts,
              let unknownBlockingMounts,
              let inventoryDriftMounts
        else { return false }
        return inventoryDriftMounts > 0
            && activeMounts == inventoryDriftMounts
            && liveMounts == 0
            && pendingCleanup == 0
            && recoverableStaleMounts == 0
            && unknownBlockingMounts == 0
            && snapshotAccessActiveLeases == 0
            && snapshotAccessPendingCleanup == 0
    }

    /// Pending cleanup is actionable only when the helper has proved that no live, stale, or
    /// unknown row is mixed into the same inventory. The operation still revalidates every row
    /// through the root helper before changing the journal.
    var canRetryPendingCleanup: Bool {
        guard reachable,
              cleanupRetryCompatible,
              classificationKnown,
              let activeMounts,
              let liveMounts,
              let pendingCleanup,
              let pendingCleanupCandidates,
              let recoverableStaleMounts,
              let unknownBlockingMounts,
              let inventoryDriftMounts
        else { return false }
        return pendingCleanup > 0
            && liveMounts == 0
            && recoverableStaleMounts == 0
            && unknownBlockingMounts == 0
            && inventoryDriftMounts == 0
            && snapshotAccessActiveLeases == 0
            && snapshotAccessPendingCleanup == 0
            && activeMounts == pendingCleanup
            && pendingCleanupCandidates == pendingCleanup
    }

    /// The status view has a separate unknown-row explanation. Suppress the aggregate update
    /// warning for this state so one journal inventory is described exactly once.
    var visibleUpdateBlockReason: String? {
        // A current helper exposes the exact unknown-row count and the status view renders the
        // explicit repair action. Showing the legacy aggregate warning as well would repeat the
        // same blocker and imply that updating the helper can clear it.
        guard (unknownBlockingMounts ?? 0) == 0, !isLegacyUnknownOnlyUpgradeSafe else { return nil }
        return SnapshotMountHelperReadiness.updateBlockReason(
            activeMounts: activeMounts,
            pendingCleanup: pendingCleanup
        )
    }

    /// The cleanup request is protocol-compatible with the current and immediately previous
    /// helper. It intentionally does not require the current build identity: this is the escape
    /// hatch that clears a safe pending row before the helper update can be enabled.
    var cleanupRetryCompatible: Bool {
        guard let helperVersion, let buildCommit else { return false }
        let validBuild = buildCommit.utf8.count == 40
            && buildCommit.utf8.allSatisfy {
                (48...57).contains($0) || (65...70).contains($0) || (97...102).contains($0)
            }
        return (helperVersion == "0.5.6" && isReady)
            || (helperVersion == "0.5.4" && isReady)
            || (helperVersion == "0.5.3" && validBuild)
            || (helperVersion == "0.5.2" && validBuild)
            || (helperVersion == "0.5.1" && validBuild)
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
