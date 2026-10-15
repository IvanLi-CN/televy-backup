import AppKit
import Foundation
import SwiftUI

/// Exercise the production NSWindow, not an unconstrained component ImageRenderer.
/// Every status, path and run is synthetic; no daemon, backup or log file is opened.
@main
enum MainWindowLayoutTests {
    private static var failures: [String] = []

    @MainActor
    static func main() throws {
        guard CommandLine.arguments.count == 2,
              ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_DEMO"] == "1"
        else { exit(2) }
        let output = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        _ = NSApplication.shared
        NSApp.setActivationPolicy(.regular)
        let model = AppModel()
        var window: NSWindow?
        model.persistentWindowPresentationHandler = { candidate, _ in
            window = candidate
            candidate.makeKeyAndOrderFront(nil)
        }
        model.openMainWindow()
        settle()
        guard let window else { fatalError("Production main window was not created") }
        defer { window.close() }

        // Begin with the running production demo, then deliver a terminal update
        // through the same stores as the real app while retaining the window.
        check(window, scenario: "running")
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
        model.runHistory = (0..<30).map(run)
        model.statusSnapshot = snapshot
        model.statusStore.ingest(snapshot, receivedAt: Date())
        settle()

        for (theme, appearance) in [("light", NSAppearance.Name.aqua), ("dark", .darkAqua)] {
            window.appearance = NSAppearance(named: appearance)
            for (sizeName, size) in [
                ("default", NSSize(width: 980, height: 640)),
                ("minimum", NSSize(width: 860, height: 520)),
                ("enlarged", NSSize(width: 1400, height: 900)),
                ("restored", NSSize(width: 980, height: 640)),
            ] {
                window.setContentSize(size)
                settle()
                check(window, scenario: "\(theme)-history-\(sizeName)")
                if sizeName == "default" || sizeName == "minimum" {
                    try capture(window, to: output.appendingPathComponent("main-window-\(theme)-history-\(sizeName).png"))
                }
            }
        }

        // Operate the real segmented control through its in-process accessibility
        // interface. This requires no Accessibility permission or external UI agent.
        if let diagnostics = elements(window).first(where: { text($0) == "Diagnostics" }) {
            record(diagnostics.accessibilityPerformPress(), "Diagnostics must be selectable")
            settle()
            check(window, scenario: "diagnostics")
            try capture(window, to: output.appendingPathComponent("main-window-dark-diagnostics.png"))
        } else {
            record(false, "Diagnostics control was not found")
        }
        if let history = elements(window).first(where: { text($0) == "History" }) {
            record(history.accessibilityPerformPress(), "History must be selectable again")
            settle()
            check(window, scenario: "history-return")
        } else {
            record(false, "History control was not found")
        }
        if !failures.isEmpty {
            failures.forEach { fputs("FAIL: \($0)\n", stderr) }
            exit(1)
        }
        print("PASS: production main window retains visible sidebar/header/tabs across failure, resize and tab changes")
    }

    @MainActor
    private static func settle() {
        RunLoop.current.run(until: Date().addingTimeInterval(0.5))
        NSApp.windows.forEach { $0.contentView?.layoutSubtreeIfNeeded(); $0.displayIfNeeded() }
    }

    private static func elements(_ root: Any, depth: Int = 0) -> [any NSAccessibilityProtocol] {
        guard depth < 40, let element = root as? any NSAccessibilityProtocol else { return [] }
        return [element] + (element.accessibilityChildren() ?? []).flatMap { elements($0, depth: depth + 1) }
    }

    private static func text(_ element: any NSAccessibilityProtocol) -> String {
        element.accessibilityLabel() ?? (element.accessibilityValue() as? String) ?? ""
    }

    @MainActor
    private static func check(_ window: NSWindow, scenario: String) {
        let viewport = window.convertToScreen(window.contentLayoutRect).insetBy(dx: -1, dy: -1)
        let nodes = elements(window)
        for identifier in ["main-window-sidebar-heading", "main-window-target-heading", "main-window-detail-tabs"] {
            guard let node = nodes.first(where: { $0.accessibilityIdentifier() == identifier }) else {
                record(false, "\(scenario): missing \(identifier)")
                continue
            }
            let frame = node.accessibilityFrame()
            print("LAYOUT \(scenario) \(identifier): frame=\(frame) viewport=\(viewport)")
            record(!frame.isEmpty && viewport.contains(frame), "\(scenario): \(identifier) is clipped outside the window: \(frame), viewport=\(viewport)")
        }
        if scenario != "running" {
            let notice = nodes.first { text($0) == "Could not create the backup snapshot" }
            record(notice != nil, "\(scenario): failure guidance must be present")
            if let notice {
                record(viewport.contains(notice.accessibilityFrame()), "\(scenario): failure guidance is clipped")
            }
        }
    }

    private static func record(_ condition: Bool, _ message: String) {
        if !condition { failures.append(message) }
    }

    @MainActor
    private static func capture(_ window: NSWindow, to url: URL) throws {
        guard let view = window.contentView,
              let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)
        else { fatalError("Cannot capture synthetic window") }
        view.cacheDisplay(in: view.bounds, to: bitmap)
        guard let png = bitmap.representation(using: .png, properties: [:]) else { fatalError("Cannot encode synthetic window") }
        try png.write(to: url)
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
