import Foundation

private func expectInstallation(_ condition: @autoclosure () -> Bool, _ message: String) {
    if !condition() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
}

private struct FixtureStatus {
    let reachable: Bool
}

private func testInstallationFailureSurvivesSuccessfulStatusRefresh() {
    var finishInstall: ((Bool, String?) -> Void)?
    var finishRefresh: ((FixtureStatus?, String?) -> Void)?
    var result: (Bool, String?, FixtureStatus?)?
    SnapshotMountHelperInstallation.perform(
        install: { finishInstall = $0 },
        refresh: { finishRefresh = $0 },
        isReady: { $0.reachable },
        completion: { result = ($0, $1, $2) }
    )
    expectInstallation(result == nil && finishRefresh == nil, "refresh must wait for administrator installation")
    finishInstall?(false, "bootstrap failed: fixture error")
    expectInstallation(result == nil, "completion must wait for the status refresh")
    finishRefresh?(FixtureStatus(reachable: false), nil)
    expectInstallation(result?.0 == false, "failed installation cannot become success")
    expectInstallation(result?.1 == "bootstrap failed: fixture error", "status refresh must not erase the actual installer failure")
}

private func testExitZeroIsInsufficientWhenTheProtocolStillFails() {
    var result: (Bool, String?, FixtureStatus?)?
    SnapshotMountHelperInstallation.perform(
        install: { $0(true, nil) },
        refresh: { $0(FixtureStatus(reachable: false), nil) },
        isReady: { $0.reachable },
        completion: { result = ($0, $1, $2) }
    )
    expectInstallation(result?.0 == false, "an authorized command exiting zero is not protocol readiness")
    expectInstallation(result?.1?.contains("communication") == true, "incompatible services need a persistent installation diagnosis")
}

private func testConfirmedReadinessAndRetry() {
    var successes = 0
    SnapshotMountHelperInstallation.perform(
        install: { $0(true, nil) },
        refresh: { $0(FixtureStatus(reachable: true), nil) },
        isReady: { $0.reachable },
        completion: { success, error, _ in
            expectInstallation(success && error == nil, "a compatible installed service must succeed")
            successes += 1
        }
    )
    expectInstallation(successes == 1, "completion must be delivered exactly once")
}

private func testInstallationFailureWinsOverAnExistingHealthyService() {
    SnapshotMountHelperInstallation.perform(
        install: { $0(false, "authorization cancelled") },
        refresh: { $0(FixtureStatus(reachable: true), "fixture refresh warning") },
        isReady: { $0.reachable },
        completion: { success, error, _ in
            expectInstallation(!success, "an existing healthy service cannot turn a cancelled installation into success")
            expectInstallation(error == "authorization cancelled", "installer failure must take precedence over refresh output")
        }
    )
}

private func testUnavailableStatusPreservesTheVerificationError() {
    SnapshotMountHelperInstallation.perform(
        install: { $0(true, nil) },
        refresh: { $0(nil as FixtureStatus?, "fixture control IPC unavailable") },
        isReady: { $0.reachable },
        completion: { success, error, _ in
            expectInstallation(!success, "an unavailable verification must fail closed")
            expectInstallation(error?.contains("fixture control IPC unavailable") == true, "the verification failure must be visible")
        }
    )
}

@main
enum SnapshotMountHelperInstallationTestsMain {
    static func main() {
        testInstallationFailureSurvivesSuccessfulStatusRefresh()
        testExitZeroIsInsufficientWhenTheProtocolStillFails()
        testConfirmedReadinessAndRetry()
        testInstallationFailureWinsOverAnExistingHealthyService()
        testUnavailableStatusPreservesTheVerificationError()
        print("OK: SnapshotMountHelperInstallationTests")
    }
}
