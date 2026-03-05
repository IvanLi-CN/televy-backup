import AppKit
import Combine
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

private func pumpRunLoop(_ seconds: Double = 0.06) {
    RunLoop.current.run(until: Date().addingTimeInterval(seconds))
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

private func expectedPopoverHeight(host: NSHostingController<AnyView>) -> CGFloat {
    let s = host.sizeThatFits(in: NSSize(width: PopoverAutoSize.width, height: 10_000))
    return PopoverAutoSize.clampHeight(s.height)
}

private final class PopoverSizingHarness {
    let model: AppModel
    let host: NSHostingController<AnyView>
    private var cancellable: AnyCancellable?
    private(set) var observedHeight: CGFloat = 0

    init(model: AppModel) {
        self.model = model
        host = NSHostingController(rootView: AnyView(PopoverRootView().environmentObject(model)))
        _ = host.view // force view load
        observedHeight = PopoverAutoSize.clampedHeightThatFits(host)
        cancellable = model.$popoverResizeToken.sink { [weak self] _ in
            guard let self else { return }
            self.observedHeight = PopoverAutoSize.clampedHeightThatFits(self.host)
        }
    }

    func settleLayout() {
        let s = host.sizeThatFits(in: NSSize(width: PopoverAutoSize.width, height: 10_000))
        host.view.frame = NSRect(x: 0, y: 0, width: PopoverAutoSize.width, height: s.height)
        host.view.layoutSubtreeIfNeeded()
        pumpRunLoop()
    }
}

private func test_popover_height_tracks_sizeThatFits_across_scenarios() {
    let model = AppModel()
    let harness = PopoverSizingHarness(model: model)

    let t0 = nowMs()

    // A) 0 targets (empty state)
    model.statusSnapshot = mkSnapshot(targets: [], nowMs: t0)
    model.requestPopoverResize()
    harness.settleLayout()
    expectClose(
        harness.observedHeight,
        expectedPopoverHeight(host: harness.host),
        "0 targets popover height should match sizeThatFits (clamped)"
    )

    // B) 1 target (idle)
    let targets1 = [
        mkTarget(id: "t1", label: "Sync", sourcePath: "/Users/ivan/Sync", state: "idle", nowMs: t0),
    ]
    model.statusSnapshot = mkSnapshot(targets: targets1, nowMs: t0)
    model.requestPopoverResize()
    harness.settleLayout()
    expectClose(
        harness.observedHeight,
        expectedPopoverHeight(host: harness.host),
        "1 target idle popover height should match sizeThatFits (clamped)"
    )

    // C) 3 targets (idle, long-ish paths)
    let longPath = "/Users/ivan/Library/Application Support/TelevyBackup/test-source-mtproto"
    let targets3 = [
        mkTarget(id: "t1", label: "Sync", sourcePath: "/Users/ivan/Sync", state: "idle", nowMs: t0),
        mkTarget(id: "t2", label: "Projects", sourcePath: "/Users/ivan/Projects", state: "idle", nowMs: t0),
        mkTarget(id: "t3", label: "mtproto-test", sourcePath: longPath, state: "idle", nowMs: t0),
    ]
    model.statusSnapshot = mkSnapshot(targets: targets3, nowMs: t0)
    model.requestPopoverResize()
    harness.settleLayout()
    expectClose(
        harness.observedHeight,
        expectedPopoverHeight(host: harness.host),
        "3 targets idle popover height should match sizeThatFits (clamped)"
    )

    // D) 2 targets (running + idle)
    let targets2 = [
        mkTarget(id: "t1", label: "Sync", sourcePath: "/Users/ivan/Sync", state: "running", nowMs: t0),
        mkTarget(id: "t2", label: "Projects", sourcePath: "/Users/ivan/Projects", state: "idle", nowMs: t0),
    ]
    model.statusSnapshot = mkSnapshot(targets: targets2, nowMs: t0)
    model.requestPopoverResize()
    harness.settleLayout()
    expectClose(
        harness.observedHeight,
        expectedPopoverHeight(host: harness.host),
        "2 targets (running+idle) popover height should match sizeThatFits (clamped)"
    )

    // E) Many targets (should scroll + max clamp semantics must hold)
    var many: [StatusTarget] = []
    for i in 0..<28 {
        many.append(mkTarget(id: "t\(i)", label: "t\(i)", sourcePath: "/Users/ivan/Demo/\(i)", state: "idle", nowMs: t0))
    }
    model.statusSnapshot = mkSnapshot(targets: many, nowMs: t0)
    model.requestPopoverResize()
    harness.settleLayout()
    expect(harness.observedHeight >= PopoverAutoSize.minHeight, "many targets should respect minHeight")
    expect(harness.observedHeight <= PopoverAutoSize.maxHeight, "many targets should respect maxHeight")
    expectClose(
        harness.observedHeight,
        expectedPopoverHeight(host: harness.host),
        "many targets popover height should match sizeThatFits (clamped)"
    )
}

@main
enum PopoverLayoutSizeTestsMain {
    static func main() {
        test_popover_height_tracks_sizeThatFits_across_scenarios()
        print("OK: PopoverLayoutSizeTests")
    }
}

