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
        let legacy = state(reachable: true, daemonReady: false, commit: nil)
        expectReadiness(!legacy.isReady, "a reachable legacy helper must require an update")
        expectReadiness(legacy.actionTitle == "Update mount helper", "the old helper must retain its update action")
        expectReadiness(legacy.updateExplanation != nil, "the update requirement must explain the available action")

        let stale = state(reachable: true, daemonReady: true, commit: String(repeating: "b", count: 40))
        expectReadiness(!stale.isReady, "a stale daemon's ready flag must not override the current app identity")
        expectReadiness(stale.actionTitle == "Update mount helper", "a same-version stale build needs an update action")

        let current = state(reachable: true, daemonReady: true, commit: candidateCommit)
        expectReadiness(current.isReady && current.actionTitle == nil, "a verified current helper must be ready")
        let clear = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, liveMounts: 0, pendingCleanup: 0,
            recoverableStaleMounts: 0, unknownBlockingMounts: 0
        )
        expectReadiness(clear.canUpdate, "a verified helper with a complete clear status can be updated")
        let recoverable = SnapshotMountHelperReadiness(
            reachable: true, daemonReady: true, buildCommit: candidateCommit,
            expectedBuildCommit: candidateCommit, liveMounts: 0, pendingCleanup: 1,
            recoverableStaleMounts: 1, unknownBlockingMounts: 0
        )
        expectReadiness(!recoverable.canUpdate, "recoverable stale rows must keep helper mutation blocked")
        expectReadiness(!current.canUpdate, "missing additive status fields must not become a false zero")
        expectReadiness(!state(reachable: false, daemonReady: true, commit: candidateCommit).isReady, "unreachable helpers cannot be ready")
        expectReadiness(state(reachable: false, daemonReady: false, commit: nil).actionTitle == "Install mount helper", "missing helpers need an installation action")
        expectReadiness(!state(reachable: true, daemonReady: false, commit: candidateCommit).isReady, "a failed daemon verification cannot be overridden")
        for expected in [nil, "", "unknown", String(repeating: "z", count: 40)] as [String?] {
            expectReadiness(!SnapshotMountHelperReadiness(reachable: true, daemonReady: true, buildCommit: expected, expectedBuildCommit: expected).isReady, "missing or malformed app identity cannot prove readiness")
        }

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

    private static func state(reachable: Bool, daemonReady: Bool, commit: String?) -> SnapshotMountHelperReadiness {
        SnapshotMountHelperReadiness(reachable: reachable, daemonReady: daemonReady, buildCommit: commit, expectedBuildCommit: candidateCommit)
    }
}
