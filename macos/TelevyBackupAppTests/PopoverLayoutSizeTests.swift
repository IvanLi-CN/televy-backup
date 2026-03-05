import AppKit
import Foundation
import SwiftUI

@discardableResult
private func expect(_ ok: @autoclosure () -> Bool, _ message: String) -> Bool {
    if !ok() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
    return true
}

private func expectClose(_ lhs: CGFloat, _ rhs: CGFloat, _ message: String, eps: CGFloat = 1.0) {
    expect(abs(lhs - rhs) <= eps, "\(message) (got=\(lhs) expected=\(rhs))")
}

private func pumpRunLoop(_ seconds: Double) {
    RunLoop.current.run(until: Date().addingTimeInterval(seconds))
}

private func waitUntil(
    _ message: String,
    timeoutSeconds: Double = 2.0,
    stepSeconds: Double = 0.02,
    _ condition: () -> Bool
) {
    let deadline = Date().addingTimeInterval(timeoutSeconds)
    while Date() < deadline {
        if condition() { return }
        pumpRunLoop(stepSeconds)
    }
    expect(false, message)
}

private func nowMs() -> Int64 {
    Int64(Date().timeIntervalSince1970 * 1000.0)
}

private func mkTarget(
    id: String,
    label: String,
    sourcePath: String,
    state: String,
    nowMs: Int64
) -> StatusTarget {
    let runningSince = state == "running" ? (nowMs - 12_000) : nil
    let progress: StatusProgress? = state == "running"
        ? StatusProgress(
            phase: "upload",
            sourceFilesTotal: 10_000,
            sourceBytesTotal: 5_120_000_000,
            filesTotal: 10_000,
            filesDone: 3_210,
            chunksTotal: nil,
            chunksDone: nil,
            bytesRead: 2_432_000_000,
            bytesUploaded: 1_700_000_000,
            bytesDownloaded: nil,
            bytesDeduped: 11_800_000
        )
        : nil

    return StatusTarget(
        targetId: id,
        label: label,
        sourcePath: sourcePath,
        endpointId: "ep_\(id)",
        enabled: true,
        state: state,
        runningSince: runningSince,
        up: StatusRate(bytesPerSecond: 1_200_000),
        upTotal: StatusCounter(bytes: 123_456_789),
        progress: progress,
        lastRun: nil
    )
}

private func mkSnapshot(targets: [StatusTarget], nowMs: Int64) -> StatusSnapshot {
    StatusSnapshot(
        type: "status.snapshot",
        schemaVersion: 1,
        generatedAt: nowMs,
        source: StatusSource(kind: "daemon", detail: "ui-test"),
        global: StatusGlobal(
            up: StatusRate(bytesPerSecond: 3_200_000),
            down: StatusRate(bytesPerSecond: 8_100_000),
            upTotal: StatusCounter(bytes: 1_234_567_890),
            downTotal: StatusCounter(bytes: 987_654_321),
            uiUptimeSeconds: 42.0
        ),
        targets: targets
    )
}

private final class PopoverSizingHarness {
    let model: AppModel
    let appDelegate: AppDelegate
    let popover: NSPopover
    let host: NSHostingController<AnyView>

    var observedHeight: CGFloat {
        popover.contentSize.height
    }

    init(model: AppModel) {
        self.model = model
        appDelegate = AppDelegate()
        let setup = appDelegate.testing_setUpPopoverForSizingOnly(model: model)
        popover = setup.popover
        host = setup.host
    }

    func layoutHostAndExpectedHeight() -> CGFloat {
        let s = host.sizeThatFits(in: NSSize(width: PopoverAutoSize.width, height: 10_000))
        host.view.frame = NSRect(x: 0, y: 0, width: PopoverAutoSize.width, height: s.height)
        host.view.layoutSubtreeIfNeeded()
        return PopoverAutoSize.clampHeight(s.height)
    }
}

@discardableResult
private func waitForPopoverHeightToMatchExpected(
    harness: PopoverSizingHarness,
    _ message: String,
    timeoutSeconds: Double = 2.0,
    eps: CGFloat = 1.0
) -> CGFloat {
    var lastExpected: CGFloat = 0
    waitUntil("\(message) (timeout)", timeoutSeconds: timeoutSeconds) {
        lastExpected = harness.layoutHostAndExpectedHeight()
        return abs(harness.observedHeight - lastExpected) <= eps
    }
    expectClose(harness.observedHeight, lastExpected, message, eps: eps)
    return lastExpected
}

private func test_popover_clampHeight_semantics() {
    expectClose(PopoverAutoSize.clampHeight(0), PopoverAutoSize.minHeight, "clampHeight should clamp to minHeight")
    expectClose(
        PopoverAutoSize.clampHeight(PopoverAutoSize.minHeight - 0.1),
        PopoverAutoSize.minHeight,
        "clampHeight should ceil before applying minHeight"
    )
    expectClose(
        PopoverAutoSize.clampHeight(PopoverAutoSize.minHeight + 0.1),
        PopoverAutoSize.minHeight + 1,
        "clampHeight should use ceil to avoid fractional jitter"
    )
    expectClose(
        PopoverAutoSize.clampHeight(PopoverAutoSize.maxHeight - 0.1),
        PopoverAutoSize.maxHeight,
        "clampHeight should ceil near maxHeight"
    )
    expectClose(
        PopoverAutoSize.clampHeight(PopoverAutoSize.maxHeight + 0.1),
        PopoverAutoSize.maxHeight,
        "clampHeight should clamp to maxHeight"
    )
}

private func test_popover_height_tracks_sizeThatFits_across_scenarios() {
    let model = AppModel()
    let harness = PopoverSizingHarness(model: model)

    let t0 = nowMs()

    // A) 0 targets (empty state)
    model.statusSnapshot = mkSnapshot(targets: [], nowMs: t0)
    model.requestPopoverResize()
    waitForPopoverHeightToMatchExpected(harness: harness, "0 targets popover height should match sizeThatFits (clamped)")

    // B) 1 target (idle)
    let targets1 = [
        mkTarget(id: "t1", label: "Sync", sourcePath: "/Users/ivan/Sync", state: "idle", nowMs: t0),
    ]
    model.statusSnapshot = mkSnapshot(targets: targets1, nowMs: t0)
    model.requestPopoverResize()
    waitForPopoverHeightToMatchExpected(harness: harness, "1 target idle popover height should match sizeThatFits (clamped)")

    // C) 3 targets (idle, long-ish paths)
    let longPath = "/Users/ivan/Library/Application Support/TelevyBackup/test-source-mtproto"
    let targets3 = [
        mkTarget(id: "t1", label: "Sync", sourcePath: "/Users/ivan/Sync", state: "idle", nowMs: t0),
        mkTarget(id: "t2", label: "Projects", sourcePath: "/Users/ivan/Projects", state: "idle", nowMs: t0),
        mkTarget(id: "t3", label: "mtproto-test", sourcePath: longPath, state: "idle", nowMs: t0),
    ]
    model.statusSnapshot = mkSnapshot(targets: targets3, nowMs: t0)
    model.requestPopoverResize()
    waitForPopoverHeightToMatchExpected(harness: harness, "3 targets idle popover height should match sizeThatFits (clamped)")

    // D) 2 targets (running + idle)
    let targets2 = [
        mkTarget(id: "t1", label: "Sync", sourcePath: "/Users/ivan/Sync", state: "running", nowMs: t0),
        mkTarget(id: "t2", label: "Projects", sourcePath: "/Users/ivan/Projects", state: "idle", nowMs: t0),
    ]
    model.statusSnapshot = mkSnapshot(targets: targets2, nowMs: t0)
    model.requestPopoverResize()
    waitForPopoverHeightToMatchExpected(harness: harness, "2 targets (running+idle) popover height should match sizeThatFits (clamped)")

    // E) Many targets (should scroll + max clamp semantics must hold)
    var many: [StatusTarget] = []
    for i in 0..<28 {
        many.append(mkTarget(id: "t\(i)", label: "t\(i)", sourcePath: "/Users/ivan/Demo/\(i)", state: "idle", nowMs: t0))
    }
    model.statusSnapshot = mkSnapshot(targets: many, nowMs: t0)
    model.requestPopoverResize()
    waitForPopoverHeightToMatchExpected(harness: harness, "many targets popover height should match sizeThatFits (clamped)")
    expect(harness.observedHeight >= PopoverAutoSize.minHeight, "many targets should respect minHeight")
    expect(harness.observedHeight <= PopoverAutoSize.maxHeight, "many targets should respect maxHeight")
}

private func test_popover_auto_resize_triggers_on_target_change() {
    let model = AppModel()
    let harness = PopoverSizingHarness(model: model)

    let t0 = nowMs()

    // Prime a baseline so `.onAppear` has a chance to run; subsequent target changes must rely on `.onChange`.
    model.statusSnapshot = mkSnapshot(targets: [], nowMs: t0)
    model.requestPopoverResize()
    waitForPopoverHeightToMatchExpected(harness: harness, "baseline empty state should size popover correctly")

    let token0 = model.popoverResizeToken

    let targets1 = [
        mkTarget(id: "t1", label: "Sync", sourcePath: "/Users/ivan/Sync", state: "idle", nowMs: t0),
    ]
    model.statusSnapshot = mkSnapshot(targets: targets1, nowMs: t0)

    waitUntil("changing targetIds should auto request popover resize (timeout)") {
        model.popoverResizeToken > token0
    }

    waitForPopoverHeightToMatchExpected(
        harness: harness,
        "auto requestPopoverResize should update popover height to match sizeThatFits (clamped)"
    )
}

private func test_popover_resize_threshold_avoids_jitter() {
    let model = AppModel()
    let harness = PopoverSizingHarness(model: model)

    let t0 = nowMs()
    let targets1 = [
        mkTarget(id: "t1", label: "Sync", sourcePath: "/Users/ivan/Sync", state: "idle", nowMs: t0),
    ]
    model.statusSnapshot = mkSnapshot(targets: targets1, nowMs: t0)
    model.requestPopoverResize()
    let expected = waitForPopoverHeightToMatchExpected(
        harness: harness,
        "baseline popover height should match sizeThatFits (clamped)"
    )

    // Nudge popover height within the updateThreshold; apply should early-return and keep the existing size.
    let jittered = expected + 0.5
    harness.popover.contentSize = NSSize(width: PopoverAutoSize.width, height: jittered)
    harness.appDelegate.testing_applyPopoverSizeThatFitsNow()
    expectClose(
        harness.observedHeight,
        jittered,
        "popover height should not be updated when within updateThreshold"
    )
}

@main
enum PopoverLayoutSizeTestsMain {
    static func main() {
        test_popover_clampHeight_semantics()
        test_popover_height_tracks_sizeThatFits_across_scenarios()
        test_popover_auto_resize_triggers_on_target_change()
        test_popover_resize_threshold_avoids_jitter()
        print("OK: PopoverLayoutSizeTests")
    }
}
