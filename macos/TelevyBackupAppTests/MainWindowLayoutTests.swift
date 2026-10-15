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
        control.setContentSize(NSSize(width: 300, height: 150))
        control.makeKeyAndOrderFront(nil)
        await settle()
        try capture(control, to: output.appendingPathComponent("rendering-control.png"))
        print("[DEBUG-viewport] control-nodes=\(elements(control).count)")
        control.close()
        let model = AppModel()
        var window: NSWindow?
        model.persistentWindowPresentationHandler = { candidate, _ in
            window = candidate
            candidate.makeKeyAndOrderFront(nil)
        }
        model.openMainWindow()
        await settle()
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
        await settle()

        for (theme, appearance) in [("light", NSAppearance.Name.aqua), ("dark", .darkAqua)] {
            window.appearance = NSAppearance(named: appearance)
            for (sizeName, size) in [
                ("default", NSSize(width: 980, height: 640)),
                ("minimum", NSSize(width: 860, height: 520)),
                ("enlarged", NSSize(width: 1400, height: 900)),
                ("restored", NSSize(width: 980, height: 640)),
            ] {
                window.setContentSize(size)
                await settle()
                check(window, scenario: "\(theme)-history-\(sizeName)")
                if sizeName == "default" || sizeName == "minimum" {
                    try capture(window, to: output.appendingPathComponent("main-window-\(theme)-history-\(sizeName).png"))
                }
            }
        }

        // Operate the real segmented control through its in-process accessibility
        // interface. This requires no Accessibility permission or external UI agent.
        if let diagnostics = elements(window).first(where: { text($0) == "Diagnostics" }) {
            diagnostics.accessibilityPerformAction(.press)
            await settle()
            check(window, scenario: "diagnostics")
            try capture(window, to: output.appendingPathComponent("main-window-dark-diagnostics.png"))
        } else {
            record(false, "Diagnostics control was not found")
        }
        if let history = elements(window).first(where: { text($0) == "History" }) {
            history.accessibilityPerformAction(.press)
            await settle()
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
    private static func settle() async {
        try? await Task.sleep(nanoseconds: 500_000_000)
        NSApp.windows.forEach { $0.contentView?.layoutSubtreeIfNeeded(); $0.displayIfNeeded() }
        CATransaction.flush()
    }

    // SwiftUI virtual accessibility nodes do not all declare the complete
    // NSAccessibilityProtocol. Use AppKit's public attribute bridge for traversal.
    private static func elements(_ root: Any, depth: Int = 0) -> [NSObject] {
        guard depth < 40, let element = root as? NSObject else { return [] }
        let children = attribute(element, "accessibilityChildren", .children) as? [Any] ?? []
        return [element] + children.flatMap { elements($0, depth: depth + 1) }
    }

    private static func attribute(_ element: NSObject, _ key: String, _ legacy: NSAccessibility.Attribute) -> Any? {
        if element.responds(to: NSSelectorFromString(key)), let value = element.value(forKey: key) { return value }
        return element.accessibilityAttributeValue(legacy)
    }

    private static func text(_ element: NSObject) -> String {
        for (key, name) in [("accessibilityLabel", NSAccessibility.Attribute.description), ("accessibilityTitle", .title), ("accessibilityValue", .value)] {
            if let value = attribute(element, key, name) as? String, !value.isEmpty { return value }
        }
        return ""
    }

    private static func frame(_ element: NSObject) -> NSRect {
        if element.responds(to: NSSelectorFromString("accessibilityFrame")),
           let value = element.value(forKey: "accessibilityFrame") as? NSValue { return value.rectValue }
        let origin = (element.accessibilityAttributeValue(.position) as? NSValue)?.pointValue ?? .zero
        let size = (element.accessibilityAttributeValue(.size) as? NSValue)?.sizeValue ?? .zero
        return NSRect(origin: origin, size: size)
    }

    @MainActor
    private static func check(_ window: NSWindow, scenario: String) {
        let viewport = window.convertToScreen(window.contentLayoutRect).insetBy(dx: -1, dy: -1)
        let nodes = elements(window)
        print("[DEBUG-viewport] nodes=\(nodes.count) host=\(String(describing: window.contentView?.frame)) layout=\(window.contentLayoutRect)")
        if scenario == "running" {
            nodes.prefix(25).forEach { print("[DEBUG-viewport] \(type(of: $0)) \(text($0))") }
            if let view = window.contentView { dumpViews(view) }
        }
        for identifier in ["main-window-sidebar-heading", "main-window-target-heading", "main-window-detail-tabs"] {
            guard let node = nodes.first(where: { (attribute($0, "accessibilityIdentifier", .identifier) as? String) == identifier }) else {
                record(false, "\(scenario): missing \(identifier)")
                continue
            }
            let frame = frame(node)
            print("LAYOUT \(scenario) \(identifier): frame=\(frame) viewport=\(viewport)")
            record(!frame.isEmpty && viewport.contains(frame), "\(scenario): \(identifier) is clipped outside the window: \(frame), viewport=\(viewport)")
        }
        if scenario != "running" {
            let notice = nodes.first { text($0) == "Could not create the backup snapshot" }
            record(notice != nil, "\(scenario): failure guidance must be present")
            if let notice {
                record(viewport.contains(frame(notice)), "\(scenario): failure guidance is clipped")
            }
        }
    }

    private static func dumpViews(_ view: NSView, depth: Int = 0) {
        guard depth < 8 else { return }
        print("[DEBUG-viewport] view[\(depth)] \(type(of: view)) frame=\(view.frame) bounds=\(view.bounds) hidden=\(view.isHidden)")
        view.subviews.forEach { dumpViews($0, depth: depth + 1) }
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
