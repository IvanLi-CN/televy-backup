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

        let stale = state(reachable: true, daemonReady: true, commit: String(repeating: "b", count: 40), activeMounts: 1, pendingCleanup: 0)
        expectReadiness(!stale.isReady, "a stale daemon's ready flag must not override the current app identity")
        expectReadiness(stale.actionTitle == "Update mount helper", "a same-version stale build needs an update action")
        expectReadiness(!stale.canUpdate, "legacy aggregate blockers must keep the update action disabled")

        let current = state(reachable: true, daemonReady: true, commit: candidateCommit)
        expectReadiness(current.isReady && current.actionTitle == nil, "a verified current helper must be ready")
        let clear = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, activeMounts: 0, liveMounts: 0, pendingCleanup: 0,
            recoverableStaleMounts: 0, unknownBlockingMounts: 0
        )
        expectReadiness(clear.canUpdate, "a verified helper with a complete clear status can be updated")
        let recoverable = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, activeMounts: 1, liveMounts: 0, pendingCleanup: 0,
            recoverableStaleMounts: 1, unknownBlockingMounts: 0
        )
        expectReadiness(!recoverable.canUpdate, "recoverable stale rows must keep helper mutation blocked")
        expectReadiness(recoverable.canRecoverStaleMounts, "a complete recoverable state must expose the recovery action")
        let pendingWithCandidate = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, activeMounts: 2, liveMounts: 0, pendingCleanup: 1,
            recoverableStaleMounts: 1, unknownBlockingMounts: 0
        )
        expectReadiness(!pendingWithCandidate.canRecoverStaleMounts, "pending cleanup must disable stale recovery")
        let inconsistentAggregate = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, activeMounts: 1, liveMounts: 0, pendingCleanup: 0,
            recoverableStaleMounts: 0, unknownBlockingMounts: 0
        )
        expectReadiness(!inconsistentAggregate.canUpdate, "legacy aggregate blockers must remain mutation blockers")
        expectReadiness(!inconsistentAggregate.canRecoverStaleMounts, "an inconsistent aggregate must disable recovery")
        let missingAggregate = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, activeMounts: nil, liveMounts: 0, pendingCleanup: 0,
            recoverableStaleMounts: 0, unknownBlockingMounts: 0
        )
        expectReadiness(!missingAggregate.canUpdate, "missing aggregate status must disable helper updates")
        expectReadiness(!current.canUpdate, "missing additive status fields must not become a false zero")
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
        activeMounts: UInt32? = nil,
        pendingCleanup: UInt32? = nil
    ) -> SnapshotMountHelperReadiness {
        SnapshotMountHelperReadiness(
            reachable: reachable, daemonReady: daemonReady, buildCommit: commit,
            expectedBuildCommit: candidateCommit, activeMounts: activeMounts,
            pendingCleanup: pendingCleanup
        )
    }
}
