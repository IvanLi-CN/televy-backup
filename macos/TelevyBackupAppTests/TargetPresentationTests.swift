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
    pendingBatchId: String? = nil,
    activeTask: StatusActiveTask? = nil,
    enabled: Bool = true
) -> StatusTarget {
    StatusTarget(
        targetId: "target-a",
        label: "Target A",
        sourcePath: "/tmp/target-a",
        endpointId: "endpoint-a",
        enabled: enabled,
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
        activeTask: activeTask,
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
        TargetPresentation.backupButtonState(
            snap: snapshot([value]),
            backupRequest: request,
            backupStopRequest: nil
        ) == .starting,
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
        TargetPresentation.backupButtonState(snap: snapshot([running]), backupRequest: nil, backupStopRequest: nil) == .stop,
        "an active or pending backup should expose the stop action"
    )
}

private func testRunningWithoutPendingCanQueueNextBatch() {
    let running = target(
        state: "running",
        phase: "prepare",
        activeBatchId: "batch-active",
        activeTask: StatusActiveTask(kind: "backup", directions: ["up"])
    )
    expect(
        TargetPresentation.backupButtonState(snap: snapshot([running]), backupRequest: nil, backupStopRequest: nil) == .stop,
        "a running backup should expose the stop action"
    )
    expect(TargetPresentation.stageText(running.progress?.phase) == "Preparing", "prepare wording stays aligned with z324m")
}

private func testMenuBackupControlDistinguishesTaskKinds() {
    let restore = target(
        state: "running",
        activeTask: StatusActiveTask(kind: "restore", directions: ["down"])
    )
    let verify = target(
        state: "running",
        activeTask: StatusActiveTask(kind: "verify", directions: [])
    )
    expect(
        !TargetPresentation.hasBackupInProgress(snap: snapshot([restore, verify])),
        "restore and verify must not be treated as backup work"
    )
    expect(
        TargetPresentation.menuBackupControlState(
            snap: snapshot([restore, verify]),
            backupRequest: nil,
            backupStopRequest: nil,
            lifecycleBusy: false,
            nowMs: 1_000
        ) == .backupAvailable,
        "restore and verify should leave the global backup action available"
    )

    let ambiguousRunning = target(state: "running")
    expect(
        TargetPresentation.menuBackupControlState(
            snap: snapshot([ambiguousRunning]),
            backupRequest: nil,
            backupStopRequest: nil,
            lifecycleBusy: false,
            nowMs: 1_000
        ) == .disabled,
        "an old running snapshot without activeTask must fail closed"
    )

    let disabled = target(enabled: false)
    expect(
        TargetPresentation.menuBackupControlState(
            snap: snapshot([disabled]),
            backupRequest: nil,
            backupStopRequest: nil,
            lifecycleBusy: false,
            nowMs: 1_000
        ) == .disabled,
        "backup must be disabled when no target is enabled"
    )
    expect(
        TargetPresentation.menuBackupControlState(
            snap: nil,
            backupRequest: nil,
            backupStopRequest: nil,
            lifecycleBusy: false,
            nowMs: 1_000
        ) == .backupAvailable,
        "without a snapshot the menu may start the daemon before enqueueing backup"
    )

    let stale = snapshot([target()])
    expect(
        TargetPresentation.menuBackupControlState(
            snap: stale,
            backupRequest: nil,
            backupStopRequest: nil,
            lifecycleBusy: false,
            nowMs: stale.generatedAt + StatusFreshness.staleMs + 1
        ) == .disabled,
        "stale snapshots must not start a backup"
    )
}

private func testBackupButtonUsesOnlyStartOrStopSemantics() {
    let idle = target()
    expect(
        TargetPresentation.backupButtonState(snap: snapshot([idle]), backupRequest: nil, backupStopRequest: nil) == .idle,
        "idle state should expose Start backup"
    )
    expect(
        TargetPresentation.backupButtonState(
            snap: snapshot([idle]),
            backupRequest: nil,
            backupStopRequest: BackupStopPresentation(startedAt: Date())
        ) == .stopping,
        "a stop request should disable the action while stopping"
    )
}

private func testStatusColorsFollowStateSemantics() {
    expect(TargetUserStatus.starting.colorRole == .active, "Starting should use the active color role")
    expect(TargetUserStatus.running.colorRole == .active, "Running should use the active color role")
    expect(TargetUserStatus.queued.colorRole == .queued, "Queued should use the waiting color role")
    expect(TargetUserStatus.idle.colorRole == .neutral, "Idle should use the neutral color role")
    expect(TargetUserStatus.failed.colorRole == .error, "Failed should use the error color role")
    expect(TargetUserStatus.offline.colorRole == .warning, "Offline should use the warning color role")
}

private func testBatchAcknowledgementUsesLatestSnapshot() {
    let request = BackupRequestPresentation(
        targetIds: ["target-a"],
        batchId: "batch-active",
        phase: .awaitingDaemonSnapshot,
        startedAt: Date()
    )
    let acknowledged = target(activeBatchId: "batch-active")
    expect(
        request.isObserved(in: snapshot([acknowledged])),
        "a snapshot received before the enqueue response should still acknowledge the batch"
    )
}

@main
enum TargetPresentationTestsMain {
    static func main() {
        testStartingOverlayPrecedesDaemonSnapshot()
        testQueuedAndRunningNextQueuedProjection()
        testRunningWithoutPendingCanQueueNextBatch()
        testMenuBackupControlDistinguishesTaskKinds()
        testBatchAcknowledgementUsesLatestSnapshot()
        testBackupButtonUsesOnlyStartOrStopSemantics()
        testStatusColorsFollowStateSemantics()
        print("OK: TargetPresentationTests")
    }
}
