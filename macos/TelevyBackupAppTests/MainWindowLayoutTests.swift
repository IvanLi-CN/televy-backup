import AppKit
import Foundation
import SwiftUI
import QuartzCore

/// Exercise the production NSWindow, not an unconstrained component ImageRenderer.
/// Every status, path and run is synthetic; no daemon, backup or log file is opened.
@main
enum MainWindowLayoutTests {
    private static var failures: [String] = []

    @MainActor
    static func main() {
        _ = NSApplication.shared
        NSApp.setActivationPolicy(.regular)
        Task { @MainActor in
            do {
                try await exerciseWindow()
                exit(failures.isEmpty ? 0 : 1)
            } catch {
                fputs("FAIL: synthetic window harness: \(error)\n", stderr)
                exit(1)
            }
        }
        // SwiftUI's display transactions need the AppKit application event loop.
        // A nested Foundation RunLoop alone measures empty, unrendered hosts.
        NSApp.run()
    }

    @MainActor
    private static func exerciseWindow() async throws {
        guard CommandLine.arguments.count == 2,
              ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_DEMO"] == "1"
        else { exit(2) }
        let output = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        NSApp.activate(ignoringOtherApps: true)
        let control = NSWindow(contentViewController: NSHostingController(rootView:
            VStack { Text("Synthetic rendering control"); Color.red.frame(width: 80, height: 40) }.padding(20)
        ))
        control.isReleasedWhenClosed = false
        control.setContentSize(NSSize(width: 300, height: 150))
        control.makeKeyAndOrderFront(nil)
        await settle()
        try capture(control, to: output.appendingPathComponent("rendering-control.png"))
        control.contentView?.wantsLayer = true
        await settle()
        try capture(control, to: output.appendingPathComponent("rendering-control-layer.png"))
        control.close()
        let model = AppModel()
        var window: NSWindow?
        model.persistentWindowPresentationHandler = { candidate, _ in
            window = candidate
            candidate.makeKeyAndOrderFront(nil)
        }
        model.openMainWindow()
        await settle(model)
        guard let window else { fatalError("Production main window was not created") }
        defer { window.close() }

        // Begin with the running production demo, then deliver a terminal update
        // through the same stores as the real app while retaining the window.
        check(window, scenario: "running", failureExpected: false)
        model.runHistory = (0..<30).map(run)
        await settle(model)
        check(window, scenario: "running-long-history", failureExpected: false)
        model.runHistory = [run(0)]
        await settle(model)
        check(window, scenario: "running-short-history", failureExpected: false)
        var snapshot = model.statusSnapshot!
        snapshot.targets[0].state = "failed"
        snapshot.targets[0].runningSince = nil
        snapshot.targets[0].progress = nil
        snapshot.targets[0].backupQueue = nil
        snapshot.targets[0].lastRun = StatusTargetRunSummary(
            runId: "fixture-0", kind: "backup", snapshotId: nil,
            finishedAt: "2026-01-01T00:00:31Z", durationSeconds: 31,
            status: "failed", errorCode: "snapshot.acquire_failed",
            filesIndexed: nil, bytesUploaded: nil, bytesDeduped: nil
        )
        model.statusSnapshot = snapshot
        model.statusStore.ingest(snapshot, receivedAt: Date())
        await settle(model)
        check(window, scenario: "failed-short-history")
        model.runHistory = (0..<30).map(run)
        await settle(model)
        check(window, scenario: "failed-before-resize")
        try capture(window, to: output.appendingPathComponent("main-window-failed-before-resize.png"))

        for (theme, appearance) in [("light", NSAppearance.Name.aqua), ("dark", .darkAqua)] {
            window.appearance = NSAppearance(named: appearance)
            for (sizeName, size) in [
                ("default", NSSize(width: 980, height: 640)),
                ("minimum", NSSize(width: 860, height: 520)),
                ("enlarged", NSSize(width: 1400, height: 900)),
                ("restored", NSSize(width: 980, height: 640)),
            ] {
                window.setContentSize(size)
                await settle(model)
                check(window, scenario: "\(theme)-history-\(sizeName)")
                if sizeName == "default" || sizeName == "minimum" {
                    try capture(window, to: output.appendingPathComponent("main-window-\(theme)-history-\(sizeName).png"))
                }
            }
        }

        if let picker = views(window.contentView!).compactMap({ $0 as? NSSegmentedControl }).first {
            picker.selectedSegment = 1
            picker.sendAction(picker.action, to: picker.target)
            await settle(model)
            check(window, scenario: "diagnostics")
            record(views(window.contentView!).contains { $0.identifier?.rawValue == "main-window-diagnostics" }, "Diagnostics must actually be selected")
            try capture(window, to: output.appendingPathComponent("main-window-dark-diagnostics.png"))
            picker.selectedSegment = 0
            picker.sendAction(picker.action, to: picker.target)
            await settle(model)
            check(window, scenario: "history-return")
            record(!views(window.contentView!).contains { $0.identifier?.rawValue == "main-window-diagnostics" }, "History must actually be selected again")
        } else {
            record(false, "Production segmented control was not found")
        }
        if !failures.isEmpty {
            failures.forEach { fputs("FAIL: \($0)\n", stderr) }
            exit(1)
        }
        print("PASS: production main window retains visible sidebar/header/tabs across failure, resize and tab changes")
    }

    @MainActor
    private static func settle(_ model: AppModel? = nil) async {
        if let model, var snapshot = model.statusSnapshot {
            snapshot.generatedAt = Int64(Date().timeIntervalSince1970 * 1000)
            model.statusStore.ingest(snapshot, receivedAt: Date())
        }
        try? await Task.sleep(nanoseconds: 500_000_000)
        NSApp.windows.forEach { $0.contentView?.layoutSubtreeIfNeeded(); $0.displayIfNeeded() }
        CATransaction.flush()
    }

    private static func views(_ root: NSView) -> [NSView] {
        [root] + root.subviews.flatMap(views)
    }

    @MainActor
    private static func check(_ window: NSWindow, scenario: String, failureExpected: Bool = true) {
        let viewport = window.contentLayoutRect.insetBy(dx: -1, dy: -1)
        let nodes = views(window.contentView!)
        var identifiers = ["main-window-sidebar-heading", "main-window-target-heading", "main-window-detail-tabs"]
        if failureExpected { identifiers.append("main-window-failure-guidance") }
        for identifier in identifiers {
            guard let node = nodes.first(where: { $0.identifier?.rawValue == identifier }) else {
                record(false, "\(scenario): missing \(identifier)")
                continue
            }
            let frame = node.convert(node.bounds, to: nil)
            print("LAYOUT \(scenario) \(identifier): frame=\(frame) viewport=\(viewport)")
            record(!frame.isEmpty && viewport.contains(frame), "\(scenario): \(identifier) is clipped outside the window: \(frame), viewport=\(viewport)")
        }
    }

    private static func record(_ condition: Bool, _ message: String) {
        if !condition { failures.append(message) }
    }

    @MainActor
    private static func capture(_ window: NSWindow, to url: URL) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
        process.arguments = ["-x", "-o", "-l", String(window.windowNumber), url.path]
        try process.run()
        process.waitUntilExit()
        guard process.terminationStatus == 0, FileManager.default.fileExists(atPath: url.path) else {
            fatalError("Cannot capture the synthetic window; refusing an unscoped capture")
        }
    }

    private static func run(_ index: Int) -> RunLogSummary {
        let date = Date(timeIntervalSince1970: 1_767_225_600 - Double(index * 3600))
        return RunLogSummary(
            id: "fixture-\(index)", runId: "fixture-\(index)", kind: "backup",
            targetId: "t_demo_a", endpointId: "fixture-endpoint", sourcePath: "/Volumes/Demo/Photos",
            snapshotId: nil, status: "failed", errorCode: "snapshot.acquire_failed", durationSeconds: 31,
            startedAt: date, finishedAt: date.addingTimeInterval(31),
            logURL: URL(fileURLWithPath: "/tmp/fixture-\(index).ndjson"),
            bytesUploaded: nil, bytesDeduped: nil, bytesWritten: nil, bytesChecked: nil,
            filesRestored: nil, chunksDownloaded: nil, chunksChecked: nil,
            ignoreRuleFiles: nil, ignoreInvalidRules: nil
        )
    }
}
