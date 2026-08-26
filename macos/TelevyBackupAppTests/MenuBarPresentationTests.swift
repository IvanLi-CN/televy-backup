import Foundation

@discardableResult
private func expectMenuBar(_ condition: @autoclosure () -> Bool, _ message: String) -> Bool {
    if !condition() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
    return true
}

private func menuTarget(
    id: String,
    state: String = "idle",
    activity: StatusActiveTask? = nil,
    queued: Bool = false
) -> StatusTarget {
    StatusTarget(
        targetId: id,
        label: id,
        sourcePath: "/tmp/\(id)",
        endpointId: "endpoint",
        enabled: true,
        state: state,
        runningSince: activity == nil ? nil : 1,
        up: StatusRate(bytesPerSecond: nil),
        upTotal: StatusCounter(bytes: nil),
        progress: nil,
        lastRun: nil,
        activeTask: activity,
        backupQueue: queued ? StatusBackupQueue(activeBatchId: "batch", pendingBatchId: nil) : nil
    )
}

private func menuSnapshot(
    targets: [StatusTarget],
    up: Int64? = nil,
    down: Int64? = nil
) -> StatusSnapshot {
    StatusSnapshot(
        type: "status.snapshot",
        schemaVersion: 1,
        generatedAt: 1_000,
        source: StatusSource(kind: "daemon", detail: nil),
        global: StatusGlobal(
            up: StatusRate(bytesPerSecond: up),
            down: StatusRate(bytesPerSecond: down),
            upTotal: StatusCounter(bytes: nil),
            downTotal: StatusCounter(bytes: nil),
            uiUptimeSeconds: nil
        ),
        targets: targets
    )
}

private func presentation(
    _ snapshot: StatusSnapshot?,
    phase: StatusConnectionPhase = .fresh,
    localTask: MenuBarLocalTask? = nil,
    failure: Bool = false,
    rates: Bool = false
) -> MenuBarPresentation {
    MenuBarPresentation.make(
        snapshot: snapshot,
        connectionPhase: phase,
        localTask: localTask,
        hasLiveFailure: failure,
        showsTransferRates: rates
    )
}

private func testActivityMatrix() {
    expectMenuBar(presentation(menuSnapshot(targets: [])).activity == .idle, "empty snapshot should be idle")

    let backup = StatusActiveTask(kind: "backup", directions: ["up"])
    let restore = StatusActiveTask(kind: "restore", directions: ["down"])
    let verify = StatusActiveTask(kind: "verify", directions: [])
    let sync = StatusActiveTask(kind: "sync", directions: ["up", "down"])

    expectMenuBar(
        presentation(menuSnapshot(targets: [menuTarget(id: "backup", state: "running", activity: backup)])).activity == .backup,
        "backup must remain active at zero rate"
    )
    expectMenuBar(
        presentation(menuSnapshot(targets: [menuTarget(id: "restore", state: "running", activity: restore)])).activity == .restore,
        "restore should project to restore"
    )
    expectMenuBar(
        presentation(menuSnapshot(targets: [menuTarget(id: "verify", state: "running", activity: verify)])).activity == .verify,
        "verify should project to verify"
    )
    expectMenuBar(
        presentation(menuSnapshot(targets: [menuTarget(id: "sync", state: "running", activity: sync)])).activity == .bidirectional,
        "native sync should project to bidirectional"
    )
    expectMenuBar(
        presentation(menuSnapshot(targets: [
            menuTarget(id: "backup", state: "running", activity: backup),
            menuTarget(id: "restore", state: "running", activity: restore),
        ])).activity == .bidirectional,
        "cross-target upload and download should project to bidirectional"
    )
    expectMenuBar(
        presentation(menuSnapshot(targets: [menuTarget(id: "queued", queued: true)])).activity == .backup,
        "queued backup should remain visible"
    )
}

private func testFailurePriorityAndRates() {
    let snapshot = menuSnapshot(
        targets: [
            menuTarget(id: "backup", state: "running", activity: StatusActiveTask(kind: "backup", directions: ["up"])),
            menuTarget(id: "restore", state: "running", activity: StatusActiveTask(kind: "restore", directions: ["down"])),
        ],
        up: 1_024,
        down: 2_048
    )
    let output = presentation(snapshot, failure: true, rates: true)
    expectMenuBar(output.activity == .failure, "live failure must take priority")
    expectMenuBar(
        output.title == "\u{2191} 1.0 KB/s \u{2193} 2.0 KB/s",
        "failure should preserve active-direction rates"
    )
    expectMenuBar(presentation(snapshot, rates: false).title.isEmpty, "rates are hidden by default")
    expectMenuBar(
        presentation(snapshot, phase: .stale, rates: true).title.isEmpty,
        "stale snapshots must not render rates"
    )
}

private func testFailureLatchLifecycle() {
    let now = Date(timeIntervalSince1970: 1_000)
    let latch = MenuBarFailureLatch()
    let historicalFailure = menuSnapshot(targets: [menuTarget(id: "target", state: "failed")])
    latch.observeStatus(snapshot: historicalFailure, connectionPhase: .fresh, now: now)
    expectMenuBar(!latch.isActive(now: now), "initial historical failure must not latch")

    let running = menuSnapshot(targets: [
        menuTarget(id: "target", state: "running", activity: StatusActiveTask(kind: "backup", directions: ["up"]))
    ])
    latch.observeStatus(snapshot: running, connectionPhase: .fresh, now: now)
    latch.observeStatus(snapshot: historicalFailure, connectionPhase: .fresh, now: now.addingTimeInterval(1))
    expectMenuBar(latch.isActive(now: now.addingTimeInterval(10.9)), "live transition should latch for ten seconds")
    expectMenuBar(!latch.isActive(now: now.addingTimeInterval(11)), "latch should expire after ten seconds")

    latch.observeLocalTask(MenuBarLocalTask(id: "busy", kind: "restore", state: "failed"), now: now)
    expectMenuBar(latch.isActive(now: now), "current local task failure should latch without a snapshot")

    let reconnectLatch = MenuBarFailureLatch()
    reconnectLatch.observeStatus(snapshot: running, connectionPhase: .fresh, now: now)
    reconnectLatch.observeStatus(snapshot: nil, connectionPhase: .stale, now: now.addingTimeInterval(1))
    reconnectLatch.observeStatus(
        snapshot: historicalFailure,
        connectionPhase: .fresh,
        now: now.addingTimeInterval(2)
    )
    expectMenuBar(
        !reconnectLatch.isActive(now: now.addingTimeInterval(2)),
        "a disconnected session must not turn a first reconnect failure into a live failure"
    )
}

private func testLocalTaskAndPreference() {
    expectMenuBar(
        presentation(nil, phase: .disconnected, localTask: MenuBarLocalTask(id: "restore", kind: "restore", state: "running")).activity == .restore,
        "local live task should remain visible when status transport is disconnected"
    )

    let suiteName = "TelevyBackup.MenuBarPresentationTests.\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: suiteName)!
    defaults.removePersistentDomain(forName: suiteName)
    expectMenuBar(!MenuBarPreferences.showsTransferRates(defaults: defaults), "rate preference defaults to hidden")
    MenuBarPreferences.setShowsTransferRates(true, defaults: defaults)
    expectMenuBar(MenuBarPreferences.showsTransferRates(defaults: defaults), "rate preference should persist")
    defaults.removePersistentDomain(forName: suiteName)
}

@main
enum MenuBarPresentationTestsMain {
    static func main() {
        testActivityMatrix()
        testFailurePriorityAndRates()
        testFailureLatchLifecycle()
        testLocalTaskAndPreference()
        print("OK: MenuBarPresentationTests")
    }
}
