import Foundation

private let candidateCommit = String(repeating: "a", count: 40)

private func expectReadiness(_ condition: @autoclosure () -> Bool, _ message: String) {
    if !condition() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
}

@main
enum SnapshotMountHelperReadinessTests {
    static func main() {
        let legacy = state(reachable: true, daemonReady: false, commit: nil, activeMounts: 0, pendingCleanup: 0)
        expectReadiness(!legacy.isReady, "a reachable legacy helper must require an update")
        expectReadiness(legacy.actionTitle == "Update mount helper", "the old helper must retain its update action")
        expectReadiness(legacy.updateExplanation != nil, "the update requirement must explain the available action")
        expectReadiness(legacy.canUpdate, "a legacy helper with no aggregate blockers must remain updateable")

        let legacyWireStatus = state(reachable: true, daemonReady: false, commit: nil, activeMounts: 0)
        expectReadiness(legacyWireStatus.canUpdate, "a legacy helper may omit the additive cleanup field when its aggregate is zero")
        expectReadiness(
            SnapshotMountHelperReadiness.updateBlockReason(activeMounts: 0, pendingCleanup: nil) == nil,
            "a zero legacy aggregate must not display a cleanup blocker"
        )
        let inconsistentLegacyStatus = state(reachable: true, daemonReady: false, commit: nil, activeMounts: 0, pendingCleanup: 1)
        expectReadiness(!inconsistentLegacyStatus.canUpdate, "a reported cleanup row must block an inconsistent legacy aggregate")
        expectReadiness(
            SnapshotMountHelperReadiness.updateBlockReason(activeMounts: 0, pendingCleanup: 1)?.contains("snapshot cleanup record(s) are pending") == true,
            "a reported cleanup row must remain visible even when the legacy aggregate is zero"
        )
        expectReadiness(
            SnapshotMountHelperReadiness.updateBlockReason(activeMounts: 0, pendingCleanup: 1)?.contains("aggregate is zero") == true,
            "an inconsistent zero aggregate must explain why the cleanup row still blocks the update"
        )

        let legacyBlocked = SnapshotMountHelperReadiness.updateBlockReason(activeMounts: 1, pendingCleanup: nil)
        expectReadiness(legacyBlocked?.contains("1 blocking snapshot mount record(s)") == true, "legacy blockers must include the aggregate count")
        expectReadiness(legacyBlocked?.contains("cannot classify cleanup") == true, "legacy blockers must explain the missing cleanup classification")

        let stale = state(reachable: true, daemonReady: true, commit: String(repeating: "b", count: 40), helperVersion: "0.5.0", activeMounts: 1, pendingCleanup: nil)
        expectReadiness(!stale.isReady, "a stale daemon's ready flag must not override the current app identity")
        expectReadiness(stale.actionTitle == "Update mount helper", "a same-version stale build needs an update action")
        expectReadiness(stale.isLegacyAggregateStatus, "a supported legacy helper must be recognized by its aggregate-only status")
        expectReadiness(stale.canUpdate, "the CLI must be allowed to verify Snapshot Access before replacing a legacy helper")
        expectReadiness(stale.updateExplanation?.contains("verify Snapshot Access is idle") == true, "legacy replacement must explain its guarded preflight")

        let staleUnknownOnly = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: false, buildCommit: String(repeating: "b", count: 40),
            expectedBuildCommit: candidateCommit, helperVersion: "0.5.2",
            activeMounts: 65, liveMounts: 0, pendingCleanup: 0,
            recoverableStaleMounts: 0, unknownBlockingMounts: 65,
            inventoryDriftMounts: 0, pendingCleanupCandidates: 0
        )
        expectReadiness(staleUnknownOnly.isLegacyUnknownOnlyUpgradeSafe, "a stale helper with a complete unknown-only partition must be replaceable")
        expectReadiness(staleUnknownOnly.canUpdate, "unknown-only legacy rows must retain a guarded update action")
        expectReadiness(staleUnknownOnly.updateExplanation == nil, "the unknown-row explanation must not be repeated as an aggregate update warning")
        expectReadiness(staleUnknownOnly.visibleUpdateBlockReason == nil, "the UI must suppress the duplicate aggregate warning")

        let unknownLegacyShape = state(reachable: true, daemonReady: true, commit: nil, helperVersion: "0.6.0", activeMounts: 1, pendingCleanup: nil)
        expectReadiness(!unknownLegacyShape.isLegacyAggregateStatus, "an unrecognized incomplete status must not be treated as a replaceable legacy helper")
        expectReadiness(!unknownLegacyShape.canUpdate, "an unrecognized incomplete status must keep helper mutation disabled")

        let current = state(reachable: true, daemonReady: true, commit: candidateCommit)
        expectReadiness(current.isReady && current.actionTitle == nil, "a verified current helper must be ready")
        let clear = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, activeMounts: 0, liveMounts: 0, pendingCleanup: 0,
            recoverableStaleMounts: 0, unknownBlockingMounts: 0, inventoryDriftMounts: 0,
            pendingCleanupCandidates: 0
        )
        expectReadiness(clear.canUpdate, "a verified helper with a complete clear status can be updated")
        expectReadiness(clear.isOperational, "a verified current helper with no blockers must be operational")
        let recoverable = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, activeMounts: 1, liveMounts: 0, pendingCleanup: 0,
            recoverableStaleMounts: 1, unknownBlockingMounts: 0,
            inventoryDriftMounts: 0
        )
        expectReadiness(!recoverable.canUpdate, "recoverable stale rows must keep helper mutation blocked")
        expectReadiness(recoverable.canRecoverStaleMounts, "a complete recoverable state must expose the recovery action")
        expectReadiness(!recoverable.canRetryPendingCleanup, "recoverable stale rows must not expose cleanup retry")
        expectReadiness(recoverable.title == "Snapshot mount helper recovery required", "recoverable rows must not present the helper as fully ready")
        let pendingWithCandidate = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, activeMounts: 2, liveMounts: 0, pendingCleanup: 1,
            recoverableStaleMounts: 1, unknownBlockingMounts: 0,
            inventoryDriftMounts: 0
        )
        expectReadiness(!pendingWithCandidate.canRecoverStaleMounts, "pending cleanup must disable stale recovery")
        let pendingOnly = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, helperVersion: "0.5.4",
            activeMounts: 1, liveMounts: 0, pendingCleanup: 1,
            recoverableStaleMounts: 0, unknownBlockingMounts: 0, inventoryDriftMounts: 0,
            pendingCleanupCandidates: 1
        )
        expectReadiness(pendingOnly.canRetryPendingCleanup, "a verified pending-only state must expose cleanup retry")
        expectReadiness(!pendingOnly.isOperational, "pending cleanup must keep strict mode unavailable")
        expectReadiness(pendingOnly.title == "Snapshot mount helper cleanup required", "pending cleanup must not present the helper as fully ready")

        let staleCurrentPending = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: false, buildCommit: String(repeating: "b", count: 40),
            expectedBuildCommit: candidateCommit, helperVersion: "0.5.4",
            activeMounts: 1, liveMounts: 0, pendingCleanup: 1,
            recoverableStaleMounts: 0, unknownBlockingMounts: 0, inventoryDriftMounts: 0,
            pendingCleanupCandidates: 1
        )
        expectReadiness(staleCurrentPending.isPendingCleanupUpgradeSafe, "a stale current helper with an exact pending-only partition must be replaceable")
        expectReadiness(staleCurrentPending.canUpdate, "a stale current helper must retain a guarded update action")
        expectReadiness(!staleCurrentPending.canRetryPendingCleanup, "a stale current helper must not expose cleanup retry before replacement")
        expectReadiness(staleCurrentPending.updateExplanation?.contains("all blocking records are verified cleanup candidates") == true, "the guarded update must explain why it is safe")

        let staleCurrentPartialClear = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: false, buildCommit: String(repeating: "b", count: 40),
            expectedBuildCommit: candidateCommit, helperVersion: "0.5.4",
            activeMounts: 0, liveMounts: 0, pendingCleanup: 0,
            recoverableStaleMounts: 0, unknownBlockingMounts: 0
        )
        expectReadiness(staleCurrentPartialClear.isStaleCurrentPartialClassification, "a stale current helper with complete zero partitions must be recognized")
        expectReadiness(staleCurrentPartialClear.canUpdate, "a stale current helper with complete zero partitions must retain its guarded update action")

        let staleCurrentMixed = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: false, buildCommit: String(repeating: "b", count: 40),
            expectedBuildCommit: candidateCommit, helperVersion: "0.5.4",
            activeMounts: 2, liveMounts: 1, pendingCleanup: 1,
            recoverableStaleMounts: 0, unknownBlockingMounts: 0, pendingCleanupCandidates: 1
        )
        expectReadiness(!staleCurrentMixed.isPendingCleanupUpgradeSafe, "a live mount must keep a stale current helper blocked")
        expectReadiness(!staleCurrentMixed.canUpdate, "a live mount must disable helper replacement")

        let accessPending = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, helperVersion: "0.5.4",
            activeMounts: 1, liveMounts: 0, pendingCleanup: 1,
            recoverableStaleMounts: 0, unknownBlockingMounts: 0, inventoryDriftMounts: 0,
            pendingCleanupCandidates: 1,
            snapshotAccessPendingCleanup: 1
        )
        expectReadiness(!accessPending.canRetryPendingCleanup, "Snapshot Access cleanup must gate helper cleanup retry")
        expectReadiness(accessPending.title == "Snapshot Access cleanup required", "Snapshot Access cleanup must be visible as the blocking operation")
        let staleAccessBusy = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: false, buildCommit: String(repeating: "b", count: 40),
            expectedBuildCommit: candidateCommit, helperVersion: "0.5.0", activeMounts: 0,
            pendingCleanup: 0, snapshotAccessActiveLeases: 1
        )
        expectReadiness(!staleAccessBusy.canUpdate, "Snapshot Access active leases must disable helper replacement")
        let staleAccessCleanup = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: false, buildCommit: String(repeating: "b", count: 40),
            expectedBuildCommit: candidateCommit, helperVersion: "0.5.0", activeMounts: 0,
            pendingCleanup: 0, snapshotAccessPendingCleanup: 1
        )
        expectReadiness(!staleAccessCleanup.canUpdate, "Snapshot Access cleanup retries must disable helper replacement")
        let pendingPrevious = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: false, buildCommit: String(repeating: "b", count: 40),
            expectedBuildCommit: candidateCommit, helperVersion: "0.5.1",
            activeMounts: 1, liveMounts: 0, pendingCleanup: 1,
            recoverableStaleMounts: 0, unknownBlockingMounts: 0, inventoryDriftMounts: 0,
            pendingCleanupCandidates: 0
        )
        expectReadiness(pendingPrevious.isLegacyPendingCleanupStatus, "the previous helper must expose its pending-only upgrade state")
        expectReadiness(pendingPrevious.canUpdate, "the previous helper must retain a guarded update action before cleanup retry")
        for version in ["0.5.3", "0.5.2", "0.5.1"] {
            let pendingOlder = SnapshotMountHelperReadiness(
                reachable: true, daemonReady: false, buildCommit: String(repeating: "b", count: 40),
                expectedBuildCommit: candidateCommit, helperVersion: version,
                activeMounts: 1, liveMounts: 0, pendingCleanup: 1,
                recoverableStaleMounts: 0, unknownBlockingMounts: 0, inventoryDriftMounts: 0,
                pendingCleanupCandidates: 0
            )
            expectReadiness(pendingOlder.isLegacyPendingCleanupStatus, "helper \(version) must retain a guarded pending-cleanup upgrade")
            expectReadiness(pendingOlder.canUpdate, "helper \(version) must remain updateable before cleanup retry")
        }
        let historicalCleanupRetry = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: false, buildCommit: String(repeating: "b", count: 40),
            expectedBuildCommit: candidateCommit, helperVersion: "0.5.1",
            activeMounts: 1, liveMounts: 0, pendingCleanup: 1,
            recoverableStaleMounts: 0, unknownBlockingMounts: 0, inventoryDriftMounts: 0,
            pendingCleanupCandidates: 1
        )
        expectReadiness(historicalCleanupRetry.canRetryPendingCleanup, "helper 0.5.1 must expose a guarded cleanup retry when its candidates are complete")
        for version in ["0.5.3", "0.5.2", "0.5.1"] {
            let partialOlder = SnapshotMountHelperReadiness(
                reachable: true, daemonReady: false, buildCommit: String(repeating: "b", count: 40),
                expectedBuildCommit: candidateCommit, helperVersion: version,
                activeMounts: 1, liveMounts: 0, pendingCleanup: 0,
                recoverableStaleMounts: 0, unknownBlockingMounts: 1, inventoryDriftMounts: nil
            )
            expectReadiness(partialOlder.isStaleCurrentPartialClassification, "helper \(version) must retain a guarded partial-classification upgrade")
            expectReadiness(partialOlder.canUpdate, "helper \(version) must remain updateable when inventory classification is absent")
        }
        let inconsistentAggregate = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, activeMounts: 1, liveMounts: 0, pendingCleanup: 0,
            recoverableStaleMounts: 0, unknownBlockingMounts: 0,
            inventoryDriftMounts: 0
        )
        expectReadiness(!inconsistentAggregate.canUpdate, "legacy aggregate blockers must remain mutation blockers")
        expectReadiness(!inconsistentAggregate.canRecoverStaleMounts, "an inconsistent aggregate must disable recovery")
        let inventoryDrift = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, helperVersion: "0.5.4",
            activeMounts: 1, liveMounts: 0, pendingCleanup: 0,
            recoverableStaleMounts: 0, unknownBlockingMounts: 0,
            inventoryDriftMounts: 1
        )
        expectReadiness(inventoryDrift.title == "Snapshot inventory reconciliation required", "inventory drift must have a dedicated blocking title")
        expectReadiness(inventoryDrift.canReconcileInventory, "a verified inventory-only drift must expose reconciliation")
        expectReadiness(!inventoryDrift.isOperational, "inventory drift must keep strict mode unavailable")
        let legacyRepair = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, helperVersion: "0.5.4",
            activeMounts: 2, liveMounts: 0, pendingCleanup: 0,
            recoverableStaleMounts: 0, unknownBlockingMounts: 2,
            inventoryDriftMounts: 0
        )
        expectReadiness(legacyRepair.canRepairLegacyRecords, "only an unknown-only verified partition may expose legacy repair")
        expectReadiness(legacyRepair.title == "Snapshot mount helper legacy repair required", "legacy blockers must name the repair action")
        expectReadiness(legacyRepair.visibleUpdateBlockReason == nil, "current helper legacy repair must not repeat the aggregate blocker")
        let mixedLegacyRepair = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, helperVersion: "0.5.4",
            activeMounts: 2, liveMounts: 1, pendingCleanup: 0,
            recoverableStaleMounts: 0, unknownBlockingMounts: 1,
            inventoryDriftMounts: 0
        )
        expectReadiness(!mixedLegacyRepair.canRepairLegacyRecords, "mixed live and unknown rows must keep legacy repair disabled")
        let missingAggregate = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, activeMounts: nil, liveMounts: 0, pendingCleanup: 0,
            recoverableStaleMounts: 0, unknownBlockingMounts: 0, inventoryDriftMounts: 0
        )
        expectReadiness(!missingAggregate.canUpdate, "missing aggregate status must disable helper updates")
        expectReadiness(!current.canUpdate, "missing additive status fields must not become a false zero")
        expectReadiness(!current.isOperational, "missing additive status fields must not present strict mode as operational")
        expectReadiness(!pendingOnly.canUpdate, "pending cleanup must remain an update blocker")
        expectReadiness(!state(reachable: false, daemonReady: true, commit: candidateCommit).isReady, "unreachable helpers cannot be ready")
        expectReadiness(state(reachable: false, daemonReady: false, commit: nil).actionTitle == "Install mount helper", "missing helpers need an installation action")
        expectReadiness(state(reachable: false, daemonReady: false, commit: nil).canUpdate, "missing helpers must retain an installation action")
        expectReadiness(!state(reachable: true, daemonReady: false, commit: candidateCommit).isReady, "a failed daemon verification cannot be overridden")
        for expected in [nil, "", "unknown", String(repeating: "z", count: 40)] as [String?] {
            expectReadiness(!SnapshotMountHelperReadiness(reachable: true, daemonReady: true, buildCommit: expected, expectedBuildCommit: expected).isReady, "missing or malformed app identity cannot prove readiness")
        }

        let activeMount = SnapshotMountHelperReadiness.updateBlockReason(activeMounts: 1, pendingCleanup: 0)
        expectReadiness(activeMount?.contains("1 snapshot mount record(s)") == true, "a blocking helper record must be visible in the update state")
        expectReadiness(activeMount?.contains("wait for cleanup") == true, "an active helper mount must explain how to recover")
        expectReadiness(!SnapshotMountHelperReadiness.canUpdate(activeMounts: 1, pendingCleanup: 0), "an active helper mount must disable update")

        let pendingCleanup = SnapshotMountHelperReadiness.updateBlockReason(activeMounts: 1, pendingCleanup: 1)
        expectReadiness(pendingCleanup?.contains("snapshot cleanup record(s) are pending") == true, "pending cleanup must remain distinct from an active mount")
        expectReadiness(!SnapshotMountHelperReadiness.canUpdate(activeMounts: 1, pendingCleanup: 1), "pending cleanup must disable update")
        let mixedBlockingState = SnapshotMountHelperReadiness.updateBlockReason(activeMounts: 2, pendingCleanup: 1)
        expectReadiness(mixedBlockingState?.contains("Additional snapshot mount record(s)") == true, "mixed aggregate state must remain neutral about live mounts")
        expectReadiness(mixedBlockingState?.contains("active snapshot mount") != true, "aggregate state must not claim a live mount")
        expectReadiness(SnapshotMountHelperReadiness.canUpdate(activeMounts: 0, pendingCleanup: 0), "an idle helper may be updated")

        var result: (Bool, String?)?
        SnapshotMountHelperInstallation.perform(
            install: { $0(true, nil) },
            refresh: { $0(legacy, nil) },
            isReady: { $0.isReady },
            readinessError: { $0.updateExplanation },
            completion: { success, error, _ in result = (success, error) }
        )
        expectReadiness(result?.0 == false && result?.1?.contains("Update") == true, "exit zero plus an old reachable build must fail installation confirmation")
        SnapshotMountHelperInstallation.perform(
            install: { $0(true, nil) },
            refresh: { $0(current, nil) },
            isReady: { $0.isReady },
            completion: { success, error, _ in result = (success, error) }
        )
        expectReadiness(result?.0 == true && result?.1 == nil, "the candidate replacement must complete confirmation")
        print("OK: SnapshotMountHelperReadinessTests")
    }

    private static func state(
        reachable: Bool,
        daemonReady: Bool,
        commit: String?,
        helperVersion: String? = "0.5.0",
        activeMounts: UInt32? = nil,
        pendingCleanup: UInt32? = nil,
        inventoryDriftMounts: UInt32? = nil
    ) -> SnapshotMountHelperReadiness {
        SnapshotMountHelperReadiness(
            reachable: reachable, daemonReady: daemonReady, buildCommit: commit,
            expectedBuildCommit: candidateCommit, helperVersion: helperVersion, activeMounts: activeMounts,
            pendingCleanup: pendingCleanup, inventoryDriftMounts: inventoryDriftMounts
        )
    }
}
