import Darwin
import Foundation

private func expectHistory(_ condition: @autoclosure () -> Bool, _ message: String) {
    if !condition() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
}

private func withLog(_ lines: String, _ test: (URL) -> Void) {
    let url = URL(fileURLWithPath: NSTemporaryDirectory())
        .appendingPathComponent("backup-history-\(UUID().uuidString).ndjson")
    try! lines.write(to: url, atomically: true, encoding: .utf8)
    defer { try? FileManager.default.removeItem(at: url) }
    test(url)
}

private let start = """
{"timestamp":"2026-01-01T00:00:00Z","fields":{"event":"run.start","run_id":"run-a","kind":"backup","target_id":"target-a"}}
"""

private func testLegacySnapshotFailures() {
    for event in ["snapshot.acquire_failed", "snapshot.probe_failed", "snapshot.waiting_for_source_volume"] {
        let log = start + "\n" + """
        {"timestamp":"2026-01-01T00:00:23Z","fields":{"event":"\(event)","target_id":"target-a","error":"mount helper protocol error: unsupported mount helper response version"}}
        """
        withLog(log) { url in
            let run = AppModel().parseRunLogSummary(url: url)!
            expectHistory(run.status == "failed", "legacy snapshot failure must not stay running")
            expectHistory(run.durationSeconds == 23, "failed run duration must stop at its terminal event")
            expectHistory(run.errorReason == "protocol_mismatch", "protocol failure should retain a safe diagnosis")
            expectHistory(run.runId == "run-a", "legacy repair must retain the exact run identity")
            expectHistory(run.snapshotId == nil, "setup failure must not invent a snapshot")
        }
    }
}

private func testExplicitTerminalWinsOverLegacyFailure() {
    withLog(start + "\n" + """
    {"timestamp":"2026-01-01T00:00:23Z","fields":{"event":"snapshot.acquire_failed","target_id":"target-a"}}
    {"timestamp":"2026-01-01T00:00:24Z","fields":{"event":"run.finish","run_id":"run-a","target_id":"target-a","status":"cancelled","duration_seconds":24}}
    """) { url in
        let run = AppModel().parseRunLogSummary(url: url)!
        expectHistory(run.status == "cancelled", "explicit finish must override legacy inference")
        expectHistory(run.errorCode == nil, "legacy failure must not contaminate an explicit finish")
    }
}

private func testActiveAndInterruptedLogs() {
    withLog(start + "\n" + """
    {"timestamp":"2026-01-01T00:00:23Z","fields":{"event":"snapshot.acquire_failed","target_id":"target-b"}}
    """) { url in
        let writer = try! FileHandle(forWritingTo: url)
        expectHistory(flock(writer.fileDescriptor, LOCK_SH) == 0, "fixture must hold the run writer lock")
        let active = AppModel().parseRunLogSummary(url: url)!
        expectHistory(active.status == "running", "a locked active log must remain running")
        expectHistory(active.errorCode == nil, "another target's failure must not finish this run")
        try! writer.close()
        let interrupted = AppModel().parseRunLogSummary(url: url)!
        expectHistory(interrupted.status == "interrupted", "a closed log without a finish must not stay running")
        expectHistory(interrupted.errorCode == "task.interrupted", "interrupted runs need an actionable explanation")
        expectHistory(interrupted.finishedAt == nil, "interruption must not invent an exact finish timestamp")
    }
}

private func testCurrentTerminalDiagnosis() {
    withLog(start + "\n" + """
    {"timestamp":"2026-01-01T00:00:23Z","fields":{"event":"run.finish","run_id":"run-a","target_id":"target-a","kind":"backup","status":"failed","error_code":"snapshot.acquire_failed","error_reason":"permission_denied","duration_seconds":23}}
    """) { url in
        let run = AppModel().parseRunLogSummary(url: url)!
        expectHistory(run.status == "failed" && run.errorReason == "permission_denied", "current terminal diagnosis was lost")
        let issue = BackupFailurePresentation(code: run.errorCode, reason: run.errorReason)
        expectHistory(issue.title == "Snapshot access was denied", "known errors should use plain language")
        expectHistory(issue.recovery.contains("Full Disk Access"), "permission failures should explain the next action")
        expectHistory(issue.opensSettings, "snapshot setup failures should link to Settings")
    }
    let mismatch = BackupFailurePresentation(code: "snapshot.acquire_failed", reason: "protocol_mismatch")
    expectHistory(mismatch.recovery.contains("Update TelevyBackup"), "protocol mismatches require a paired application update")
    expectHistory(mismatch.recovery.contains("Install mount helper"), "protocol mismatches should explain the mount helper step")
    let generic = BackupFailurePresentation(code: "future.unknown", reason: nil)
    expectHistory(generic.title == "The run failed", "unknown errors must not leak raw diagnostic text into the summary")
    expectHistory(generic.recovery.contains("Diagnostics"), "unknown errors should offer diagnostics")
    let legacy = try! JSONDecoder().decode(StatusTargetRunSummary.self, from: Data("{\"status\":\"failed\",\"errorCode\":\"snapshot.acquire_failed\"}".utf8))
    expectHistory(legacy.errorReason == nil, "older status snapshots must remain compatible")
}

private func testServiceFailuresKeepTheirSpecificExplanation() {
    for (code, reason, title, recovery) in [
        ("telegram.unavailable", nil, "Telegram is unavailable", "network, proxy"),
        ("telegram.timeout", nil, "Telegram did not respond in time", "Telegram connection"),
        ("telegram.unauthorized", nil, "Telegram authentication failed", "credentials"),
        ("snapshot.acquire_failed", "mount_permission_denied", "Snapshot mounting was denied", "installed mount helper"),
    ] as [(String, String?, String, String)] {
        let reasonField = reason.map { ",\"error_reason\":\"\($0)\"" } ?? ""
        withLog(start + "\n" + """
        {"timestamp":"2026-01-01T00:00:15Z","fields":{"event":"run.finish","run_id":"run-a","target_id":"target-a","status":"failed","error_code":"\(code)"\(reasonField)}}
        """) { url in
            let run = AppModel().parseRunLogSummary(url: url)!
            let issue = BackupFailurePresentation(code: run.errorCode, reason: run.errorReason)
            expectHistory(issue.title == title, "known service failure must not become The run failed: \(code)")
            expectHistory(issue.recovery.contains(recovery), "service failure needs a matching recovery action: \(code)")
            expectHistory(issue.opensSettings, "service failures should retain a Settings action")
            expectHistory(run.errorCode == code, "presentation must preserve the original error code")
        }
    }
    let legacy = BackupFailurePresentation.legacyReason(message:
        "stage=snapshot_mount command=mount_apfs exit=77 channel=stderr summary=operation not permitted")
    expectHistory(legacy == "mount_permission_denied", "legacy mount diagnostics must retain the mount phase")
    let access = BackupFailurePresentation.legacyReason(message: "permission denied (os error 13)")
    expectHistory(access == "permission_denied", "generic permission errors must not invent a mount failure")
}

@main
enum BackupFailureHistoryTestsMain {
    static func main() {
        testLegacySnapshotFailures()
        testExplicitTerminalWinsOverLegacyFailure()
        testActiveAndInterruptedLogs()
        testCurrentTerminalDiagnosis()
        testServiceFailuresKeepTheirSpecificExplanation()
        print("OK: BackupFailureHistoryTests")
    }
}
