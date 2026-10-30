import Foundation

/// Connectivity is useful diagnostics; only app-bound build evidence proves update readiness.
struct SnapshotMountHelperReadiness {
    let reachable: Bool
    let isReady: Bool
    let buildCommit: String?
    let helperVersion: String?
    let activeMounts: UInt32?
    let liveMounts: UInt32?
    let pendingCleanup: UInt32?
    let recoverableStaleMounts: UInt32?
    let unknownBlockingMounts: UInt32?
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
        pendingCleanupCandidates: UInt32? = nil,
        snapshotAccessActiveLeases: UInt32 = 0,
        snapshotAccessPendingCleanup: UInt32 = 0
    ) {
        self.reachable = reachable
        self.buildCommit = buildCommit
        self.helperVersion = helperVersion
        self.activeMounts = activeMounts
        self.liveMounts = liveMounts
        self.pendingCleanup = pendingCleanup
        self.recoverableStaleMounts = recoverableStaleMounts
        self.unknownBlockingMounts = unknownBlockingMounts
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
            return "Snapshot mount helper verification required"
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

    /// The previous helper can identify pending rows but cannot expose their lease IDs. A guarded
    /// replacement is the only safe way to upgrade it before cleanup can be retried explicitly.
    var isLegacyPendingCleanupStatus: Bool {
        guard reachable,
              helperVersion == "0.5.1",
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
        if isLegacyPendingCleanupStatus || isPendingCleanupUpgradeSafe { return true }
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
              helperVersion == "0.5.2",
              classificationKnown,
              let activeMounts,
              let liveMounts,
              let pendingCleanup,
              let recoverableStaleMounts,
              let unknownBlockingMounts,
              let pendingCleanupCandidates
        else { return false }
        return pendingCleanup > 0
            && activeMounts == pendingCleanup
            && liveMounts == 0
            && recoverableStaleMounts == 0
            && unknownBlockingMounts == 0
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
            && snapshotAccessActiveLeases == 0
            && snapshotAccessPendingCleanup == 0
            && activeMounts == recoverableStaleMounts
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
              let unknownBlockingMounts
        else { return false }
        return pendingCleanup > 0
            && liveMounts == 0
            && recoverableStaleMounts == 0
            && unknownBlockingMounts == 0
            && snapshotAccessActiveLeases == 0
            && snapshotAccessPendingCleanup == 0
            && activeMounts == pendingCleanup
            && pendingCleanupCandidates == pendingCleanup
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
        return (helperVersion == "0.5.2" && isReady)
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
