import Foundation

@discardableResult
private func expectNavigation(_ condition: @autoclosure () -> Bool, _ message: String) -> Bool {
    if !condition() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
    return true
}

private func navigationRun(targetId: String, runId: String) -> RunLogSummary {
    RunLogSummary(
        id: "\(targetId)-\(runId).ndjson",
        runId: runId,
        kind: "backup",
        targetId: targetId,
        endpointId: "endpoint",
        sourcePath: "/source",
        snapshotId: "snapshot",
        status: "succeeded",
        errorCode: nil,
        durationSeconds: 1,
        startedAt: nil,
        finishedAt: nil,
        logURL: URL(fileURLWithPath: "/tmp/\(runId).ndjson"),
        bytesUploaded: nil,
        bytesDeduped: nil,
        bytesWritten: nil,
        bytesChecked: nil,
        filesRestored: nil,
        chunksDownloaded: nil,
        chunksChecked: nil,
        ignoreRuleFiles: nil,
        ignoreInvalidRules: nil
    )
}

private func testTerminalNavigationMatchesBothTargetAndRunIdentity() {
    let intended = navigationRun(targetId: "photos", runId: "run-current")
    let sameTargetOlder = navigationRun(targetId: "photos", runId: "run-older")
    let sameRunOtherTarget = navigationRun(targetId: "documents", runId: "run-current")

    let match = MainWindowNavigationResolver.exactRun(
        targetId: "photos",
        runId: "run-current",
        runs: [sameTargetOlder, sameRunOtherTarget, intended]
    )
    expectNavigation(match?.id == intended.id, "terminal navigation must not select a different target or older run")
    expectNavigation(
        MainWindowNavigationResolver.exactRun(targetId: "photos", runId: "missing", runs: [sameTargetOlder]) == nil,
        "missing log identity must remain in the target overview instead of guessing"
    )
}

private func testTerminalNavigationIgnoresInProgressLog() {
    var inProgress = navigationRun(targetId: "photos", runId: "run-current")
    inProgress = RunLogSummary(
        id: inProgress.id,
        runId: inProgress.runId,
        kind: inProgress.kind,
        targetId: inProgress.targetId,
        endpointId: inProgress.endpointId,
        sourcePath: inProgress.sourcePath,
        snapshotId: inProgress.snapshotId,
        status: "running",
        errorCode: inProgress.errorCode,
        durationSeconds: inProgress.durationSeconds,
        startedAt: inProgress.startedAt,
        finishedAt: inProgress.finishedAt,
        logURL: inProgress.logURL,
        bytesUploaded: inProgress.bytesUploaded,
        bytesDeduped: inProgress.bytesDeduped,
        bytesWritten: inProgress.bytesWritten,
        bytesChecked: inProgress.bytesChecked,
        filesRestored: inProgress.filesRestored,
        chunksDownloaded: inProgress.chunksDownloaded,
        chunksChecked: inProgress.chunksChecked,
        ignoreRuleFiles: inProgress.ignoreRuleFiles,
        ignoreInvalidRules: inProgress.ignoreInvalidRules
    )

    expectNavigation(
        MainWindowNavigationResolver.exactRun(
            targetId: "photos",
            runId: "run-current",
            runs: [inProgress]
        ) == nil,
        "an in-progress log must not replace the active target detail"
    )
}

private func testNavigationStoreReplaysRepeatedClicks() {
    let store = MainWindowNavigationStore()
    store.submit(.target(targetId: "photos"))
    let first = store.request
    store.submit(.target(targetId: "photos"))
    let second = store.request

    expectNavigation(first?.destination == second?.destination, "repeated clicks should retain the same destination")
    expectNavigation(first?.revision != second?.revision, "repeated clicks must create a replayable navigation request")
}

private func testOptionalStatusIdentityDecodesWithLegacyFallback() {
    let legacy = """
    { "kind": "backup", "directions": ["up"] }
    """
    let current = """
    { "taskId": "run-current", "kind": "backup", "directions": ["up"] }
    """
    let oldRun = """
    { "finishedAt": "2026-01-01T00:00:00Z", "status": "succeeded" }
    """
    let currentRun = """
    { "runId": "run-current", "kind": "backup", "snapshotId": "snapshot-current", "status": "succeeded" }
    """

    let decoder = JSONDecoder()
    let legacyTask = try! decoder.decode(StatusActiveTask.self, from: Data(legacy.utf8))
    let currentTask = try! decoder.decode(StatusActiveTask.self, from: Data(current.utf8))
    let oldSummary = try! decoder.decode(StatusTargetRunSummary.self, from: Data(oldRun.utf8))
    let currentSummary = try! decoder.decode(StatusTargetRunSummary.self, from: Data(currentRun.utf8))

    expectNavigation(legacyTask.taskId == nil, "old daemon active task must decode without an identity")
    expectNavigation(currentTask.taskId == "run-current", "active task identity was not decoded")
    expectNavigation(oldSummary.runId == nil, "old daemon terminal summary must decode without identity")
    expectNavigation(
        currentSummary.runId == "run-current" && currentSummary.kind == "backup" && currentSummary.snapshotId == "snapshot-current",
        "terminal run identity fields were not decoded"
    )
}

private func testRunLogParserRetainsRunID() {
    let url = URL(fileURLWithPath: NSTemporaryDirectory())
        .appendingPathComponent("televy-navigation-run-id-\(UUID().uuidString).ndjson")
    let log = """
    {"timestamp":"2026-01-01T00:00:00Z","fields":{"event":"run.start","run_id":"run-current","kind":"backup","target_id":"photos"}}
    {"timestamp":"2026-01-01T00:01:00Z","fields":{"event":"run.finish","run_id":"run-current","kind":"backup","target_id":"photos","status":"succeeded","snapshot_id":"snapshot-current"}}
    """
    try! log.write(to: url, atomically: true, encoding: .utf8)
    defer { try? FileManager.default.removeItem(at: url) }

    let summary = AppModel().parseRunLogSummary(url: url)
    expectNavigation(summary?.runId == "run-current", "NDJSON run_id was not retained in RunLogSummary")
    expectNavigation(summary?.targetId == "photos", "NDJSON target identity was not retained")
}

@main
enum MainWindowNavigationTestsMain {
    static func main() {
        testTerminalNavigationMatchesBothTargetAndRunIdentity()
        testTerminalNavigationIgnoresInProgressLog()
        testNavigationStoreReplaysRepeatedClicks()
        testOptionalStatusIdentityDecodesWithLegacyFallback()
        testRunLogParserRetainsRunID()
        print("OK: MainWindowNavigationTests")
    }
}
