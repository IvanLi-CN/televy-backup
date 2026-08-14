import Foundation

@discardableResult
private func expect(_ ok: @autoclosure () -> Bool, _ message: String) -> Bool {
    if !ok() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
    return true
}

private func target(
    state: String = "idle",
    phase: String? = nil,
    activeBatchId: String? = nil,
    pendingBatchId: String? = nil
) -> StatusTarget {
    StatusTarget(
        targetId: "target-a",
        label: "Target A",
        sourcePath: "/tmp/target-a",
        endpointId: "endpoint-a",
        enabled: true,
        state: state,
        runningSince: state == "running" ? 1_000 : nil,
        up: StatusRate(bytesPerSecond: nil),
        upTotal: StatusCounter(bytes: nil),
        progress: phase.map {
            StatusProgress(
                phase: $0,
                sourceFilesTotal: nil,
                sourceBytesTotal: nil,
                filesTotal: nil,
                filesDone: nil,
                chunksTotal: nil,
                chunksDone: nil,
                bytesRead: nil,
                bytesUploaded: nil,
                bytesDownloaded: nil,
                bytesDeduped: nil
            )
        },
        lastRun: nil,
        backupQueue: StatusBackupQueue(activeBatchId: activeBatchId, pendingBatchId: pendingBatchId)
    )
}

private func snapshot(_ targets: [StatusTarget]) -> StatusSnapshot {
    StatusSnapshot(
        type: "status.snapshot",
        schemaVersion: 1,
        generatedAt: 1_000,
        source: StatusSource(kind: "daemon", detail: nil),
        global: StatusGlobal(
            up: StatusRate(bytesPerSecond: nil),
            down: StatusRate(bytesPerSecond: nil),
            upTotal: StatusCounter(bytes: nil),
            downTotal: StatusCounter(bytes: nil),
            uiUptimeSeconds: nil
        ),
        targets: targets
    )
}

private func testStartingOverlayPrecedesDaemonSnapshot() {
    let value = target()
    let request = BackupRequestPresentation(
        targetIds: [value.targetId],
        batchId: nil,
        phase: .starting,
        startedAt: Date()
    )
    let status = TargetPresentation.userStatus(
        target: value,
        activeTask: nil,
        backupRequest: request,
        hasInProgressRunLog: false,
        snap: snapshot([value]),
        nowMs: 1_000
    )
    expect(status == .starting, "local request should render Starting before daemon acknowledgement")
    expect(
        TargetPresentation.backupButtonState(snap: snapshot([value]), backupRequest: request) == .starting,
        "Starting request should disable the button"
    )
}

private func testQueuedAndRunningNextQueuedProjection() {
    let queued = target(activeBatchId: "batch-active")
    expect(
        TargetPresentation.userStatus(
            target: queued,
            activeTask: nil,
            backupRequest: nil,
            hasInProgressRunLog: false,
            snap: snapshot([queued]),
            nowMs: 1_000
        ) == .queued,
        "active batch member without a run should render Queued"
    )

    let running = target(
        state: "running",
        phase: "connecting",
        activeBatchId: "batch-active",
        pendingBatchId: "batch-next"
    )
    expect(
        TargetPresentation.userStatus(
            target: running,
            activeTask: nil,
            backupRequest: nil,
            hasInProgressRunLog: false,
            snap: snapshot([running]),
            nowMs: 1_000
        ) == .running,
        "running state remains authoritative when a later batch exists"
    )
    expect(TargetPresentation.hasNextQueuedBackup(target: running), "pending membership should render Next queued")
    expect(TargetPresentation.stageText(running.progress?.phase) == "Connecting", "connecting stage text should be explicit")
    expect(TargetPresentation.isConnectingPhase(running.progress?.phase), "connecting stage should use inline activity")
    expect(
        TargetPresentation.backupButtonState(snap: snapshot([running]), backupRequest: nil) == .queued,
        "pending batch should disable the all-target button"
    )
}

private func testRunningWithoutPendingCanQueueNextBatch() {
    let running = target(state: "running", phase: "prepare", activeBatchId: "batch-active")
    expect(
        TargetPresentation.backupButtonState(snap: snapshot([running]), backupRequest: nil) == .enqueueNext,
        "a running batch without a pending batch should allow one next request"
    )
    expect(TargetPresentation.stageText(running.progress?.phase) == "Preparing", "prepare wording stays aligned with z324m")
}

@main
enum TargetPresentationTestsMain {
    static func main() {
        testStartingOverlayPrecedesDaemonSnapshot()
        testQueuedAndRunningNextQueuedProjection()
        testRunningWithoutPendingCanQueueNextBatch()
        print("OK: TargetPresentationTests")
    }
}
