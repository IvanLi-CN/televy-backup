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

private func testActiveTaskDecodingIsForwardCompatible() {
    let json = """
    {
      "type": "status.snapshot",
      "schemaVersion": 1,
      "generatedAt": 1000,
      "source": { "kind": "daemon" },
      "global": {
        "up": {},
        "down": {},
        "upTotal": {},
        "downTotal": {}
      },
      "targets": [
        {
          "targetId": "missing-fields",
          "sourcePath": "/tmp/missing-fields",
          "endpointId": "endpoint",
          "enabled": true,
          "state": "idle",
          "up": {},
          "upTotal": {},
          "activeTask": {}
        },
        {
          "targetId": "unknown-kind",
          "sourcePath": "/tmp/unknown-kind",
          "endpointId": "endpoint",
          "enabled": true,
          "state": "idle",
          "up": {},
          "upTotal": {},
          "activeTask": { "kind": "archive", "directions": ["up"] }
        },
        {
          "targetId": "wrong-field-types",
          "sourcePath": "/tmp/wrong-field-types",
          "endpointId": "endpoint",
          "enabled": true,
          "state": "idle",
          "up": {},
          "upTotal": {},
          "activeTask": { "kind": ["backup"], "directions": "up" }
        },
        {
          "targetId": "scalar-activity",
          "sourcePath": "/tmp/scalar-activity",
          "endpointId": "endpoint",
          "enabled": true,
          "state": "idle",
          "up": {},
          "upTotal": {},
          "activeTask": "archive"
        }
      ]
    }
    """

    let snapshot = try! JSONDecoder().decode(StatusSnapshot.self, from: Data(json.utf8))
    expectMenuBar(snapshot.targets.count == 4, "incomplete activeTask must not discard the status snapshot")
    expectMenuBar(
        snapshot.targets.allSatisfy { $0.activeTask?.isSupported == false },
        "incomplete or unknown activity must not become a supported menu bar activity"
    )
    expectMenuBar(
        presentation(snapshot).activity == .idle,
        "incomplete or unknown activity must not change the menu bar state"
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
        output.title == "\u{2191} 1.0K/s \u{2193} 2.0K/s",
        "failure should preserve active-direction rates"
    )
    expectMenuBar(presentation(snapshot, rates: false).title.isEmpty, "rates are hidden by default")
    expectMenuBar(
        presentation(snapshot, phase: .stale, rates: true).title.isEmpty,
        "stale snapshots must not render rates"
    )
}

private func testRateSlots() {
    let cases: [(Int64?, String)] = [
        (nil, "----"),
        (-1, "----"),
        (0, "  0B"),
        (12, " 12B"),
        (999, "999B"),
        (1_000, "1.0K"),
        (1_024, "1.0K"),
        (10 * 1_024, " 10K"),
        (999 * 1_024, "999K"),
        (1_023 * 1_024, "1.0M"),
        (10 * 1_024 * 1_024, " 10M"),
        (Int64.max, "8.0E"),
    ]
    for (bytesPerSecond, expected) in cases {
        let actual = MenuBarRateSlot.format(bytesPerSecond)
        expectMenuBar(actual == expected, "rate slot \(String(describing: bytesPerSecond)) should be \(expected), got \(actual)")
        expectMenuBar(actual.count == MenuBarRateSlot.width, "rate slot must always occupy four characters")
    }

    let active = menuSnapshot(
        targets: [menuTarget(id: "backup", state: "running", activity: StatusActiveTask(kind: "backup", directions: ["up"]))],
        up: nil
    )
    expectMenuBar(
        presentation(active, rates: true).title == "\u{2191} ----/s",
        "an active direction with an unavailable rate must retain its reserved slot"
    )
    let zero = menuSnapshot(
        targets: [menuTarget(id: "backup", state: "running", activity: StatusActiveTask(kind: "backup", directions: ["up"]))],
        up: 0
    )
    expectMenuBar(
        presentation(zero, rates: true).title == "\u{2191}   0B/s",
        "zero rate must retain its active direction and fixed slot"
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

    let duplicateFailureLatch = MenuBarFailureLatch()
    duplicateFailureLatch.observeLocalTask(
        MenuBarLocalTask(id: "restore-task", kind: "restore", state: "running", targetId: "target"),
        now: now
    )
    duplicateFailureLatch.observeStatus(snapshot: running, connectionPhase: .fresh, now: now)
    duplicateFailureLatch.observeLocalTask(
        MenuBarLocalTask(id: "restore-task", kind: "restore", state: "failed", targetId: "target"),
        now: now
    )
    duplicateFailureLatch.observeStatus(
        snapshot: historicalFailure,
        connectionPhase: .fresh,
        now: now.addingTimeInterval(0.8)
    )
    expectMenuBar(
        !duplicateFailureLatch.isActive(now: now.addingTimeInterval(10)),
        "a matching daemon failure must not extend the original ten-second local failure latch"
    )

    let distinctFailureLatch = MenuBarFailureLatch()
    distinctFailureLatch.observeLocalTask(
        MenuBarLocalTask(id: "restore-one", kind: "restore", state: "running", targetId: "target"),
        now: now
    )
    distinctFailureLatch.observeStatus(snapshot: running, connectionPhase: .fresh, now: now)
    distinctFailureLatch.observeLocalTask(
        MenuBarLocalTask(id: "restore-one", kind: "restore", state: "failed", targetId: "target"),
        now: now.addingTimeInterval(1)
    )
    distinctFailureLatch.observeStatus(
        snapshot: historicalFailure,
        connectionPhase: .fresh,
        now: now.addingTimeInterval(1.2)
    )
    distinctFailureLatch.observeLocalTask(
        MenuBarLocalTask(id: "restore-two", kind: "restore", state: "running", targetId: "target"),
        now: now.addingTimeInterval(2)
    )
    distinctFailureLatch.observeLocalTask(
        MenuBarLocalTask(id: "restore-two", kind: "restore", state: "failed", targetId: "target"),
        now: now.addingTimeInterval(3)
    )
    expectMenuBar(
        distinctFailureLatch.isActive(now: now.addingTimeInterval(11.5)),
        "a distinct task failure must receive its own full ten-second latch window"
    )
    expectMenuBar(
        !distinctFailureLatch.isActive(now: now.addingTimeInterval(13)),
        "the second task failure should expire after its own ten-second window"
    )

    let daemonFailureLatch = MenuBarFailureLatch()
    daemonFailureLatch.observeStatus(snapshot: running, connectionPhase: .fresh, now: now)
    daemonFailureLatch.observeStatus(
        snapshot: historicalFailure,
        connectionPhase: .fresh,
        now: now.addingTimeInterval(1)
    )
    expectMenuBar(
        daemonFailureLatch.isActive(now: now.addingTimeInterval(1)),
        "a daemon live failure should latch before its session resets"
    )
    daemonFailureLatch.resetStatusSession()
    expectMenuBar(
        !daemonFailureLatch.isActive(now: now.addingTimeInterval(1)),
        "a new daemon session must not retain the prior daemon failure latch"
    )
    expectMenuBar(
        presentation(historicalFailure, failure: daemonFailureLatch.isActive(now: now.addingTimeInterval(1))).activity == .idle,
        "clearing a daemon session must reproject the menu bar without failure"
    )

    let localFailureLatch = MenuBarFailureLatch()
    localFailureLatch.observeLocalTask(
        MenuBarLocalTask(id: "local-restore", kind: "restore", state: "failed"),
        now: now
    )
    localFailureLatch.resetStatusSession()
    expectMenuBar(
        localFailureLatch.isActive(now: now.addingTimeInterval(1)),
        "a status session reset must retain the current local failure latch"
    )

    let reconnectLatch = MenuBarFailureLatch()
    reconnectLatch.observeStatus(snapshot: running, connectionPhase: .fresh, now: now)
    reconnectLatch.resetStatusSession()
    reconnectLatch.observeStatus(
        snapshot: historicalFailure,
        connectionPhase: .fresh,
        now: now.addingTimeInterval(2)
    )
    expectMenuBar(
        !reconnectLatch.isActive(now: now.addingTimeInterval(2)),
        "a disconnected session must not turn a first reconnect failure into a live failure"
    )
    expectMenuBar(
        !MenuBarFailureLatch.requiresStatusSessionReset(
            previousIngressAt: now,
            now: now.addingTimeInterval(4.9),
            maximumGap: 5
        ),
        "an ingress gap shorter than the stale threshold must preserve the session"
    )
    expectMenuBar(
        MenuBarFailureLatch.requiresStatusSessionReset(
            previousIngressAt: now,
            now: now.addingTimeInterval(5.1),
            maximumGap: 5
        ),
        "an ingress gap beyond the stale threshold must start a new session"
    )

    let legacyStateLatch = MenuBarFailureLatch()
    legacyStateLatch.observeStatus(
        snapshot: menuSnapshot(targets: [menuTarget(id: "legacy", state: "running")]),
        connectionPhase: .fresh,
        now: now
    )
    legacyStateLatch.observeStatus(
        snapshot: menuSnapshot(targets: [menuTarget(id: "legacy", state: "failed")]),
        connectionPhase: .fresh,
        now: now.addingTimeInterval(1)
    )
    expectMenuBar(
        !legacyStateLatch.isActive(now: now.addingTimeInterval(1)),
        "legacy running state without activeTask must not latch failure"
    )

    let unknownActivityLatch = MenuBarFailureLatch()
    unknownActivityLatch.observeStatus(
        snapshot: menuSnapshot(targets: [
            menuTarget(
                id: "unknown",
                state: "running",
                activity: StatusActiveTask(kind: "archive", directions: ["up"])
            )
        ]),
        connectionPhase: .fresh,
        now: now
    )
    unknownActivityLatch.observeStatus(
        snapshot: menuSnapshot(targets: [menuTarget(id: "unknown", state: "failed")]),
        connectionPhase: .fresh,
        now: now.addingTimeInterval(1)
    )
    expectMenuBar(
        !unknownActivityLatch.isActive(now: now.addingTimeInterval(1)),
        "unsupported activeTask must not latch failure"
    )
}

private func testLocalTaskAndPreference() {
    expectMenuBar(
        presentation(nil, phase: .disconnected, localTask: MenuBarLocalTask(id: "restore", kind: "restore", state: "running")).activity == .restore,
        "local live task should remain visible when status transport is disconnected"
    )
    expectMenuBar(
        MenuBarLocalTask.eventTaskKind(commandArguments: ["--events", "restore", "latest"]) == "restore",
        "event restore commands should be eligible for synthetic failure reporting"
    )
    expectMenuBar(
        MenuBarLocalTask.eventTaskKind(commandArguments: ["--json", "backup", "enqueue"]) == nil,
        "non-event commands must not synthesize a task failure"
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
        testActiveTaskDecodingIsForwardCompatible()
        testFailurePriorityAndRates()
        testRateSlots()
        testFailureLatchLifecycle()
        testLocalTaskAndPreference()
        print("OK: MenuBarPresentationTests")
    }
}
