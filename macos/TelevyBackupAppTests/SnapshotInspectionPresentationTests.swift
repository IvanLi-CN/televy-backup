import Foundation

@discardableResult
private func expect(_ ok: @autoclosure () -> Bool, _ message: String) -> Bool {
    if !ok() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
    return true
}

private func run(kind: String, status: String?, snapshotId: String?) -> RunLogSummary {
    RunLogSummary(
        id: "run",
        kind: kind,
        targetId: "target",
        endpointId: "endpoint",
        sourcePath: "/source",
        snapshotId: snapshotId,
        status: status,
        errorCode: nil,
        durationSeconds: nil,
        startedAt: nil,
        finishedAt: nil,
        logURL: URL(fileURLWithPath: "/tmp/run.ndjson"),
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

private func testSnapshotInspectionEligibility() {
    expect(
        SnapshotInspectionEligibility.forRun(run(kind: "backup", status: "succeeded", snapshotId: "snp_1")) == .inspectable,
        "successful backup with snapshot id should be inspectable"
    )
    expect(
        SnapshotInspectionEligibility.forRun(run(kind: "backup", status: "failed", snapshotId: "snp_1"))
            == .unavailable("File and block data are available only for successful backup runs."),
        "failed backup must not request snapshot files"
    )
    expect(
        SnapshotInspectionEligibility.forRun(run(kind: "backup", status: "succeeded", snapshotId: nil))
            == .unavailable("This run does not have a retained snapshot identifier."),
        "successful run without snapshot id must remain summary-only"
    )
}

@main
enum SnapshotInspectionPresentationTestsMain {
    static func main() {
        testSnapshotInspectionEligibility()
        print("OK: SnapshotInspectionPresentationTests")
    }
}
