import AppKit
import Foundation
import SwiftUI

/// Render the real main-window views with synthetic data; no screen capture or product service.
@main
enum BackupFailureHistoryPreview {
    static func main() throws {
        guard CommandLine.arguments.count == 2 else { exit(2) }
        let output = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        let sandbox = FileManager.default.temporaryDirectory
            .appendingPathComponent("televy-failure-preview-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: sandbox, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: sandbox) }
        setenv("TELEVYBACKUP_UI_DEMO", "1", 1)
        setenv("TELEVYBACKUP_UI_DEMO_SCENE", "main-window-target-snapshot-failure", 1)
        setenv("TELEVYBACKUP_DISABLE_KEYCHAIN", "1", 1)
        setenv("TELEVYBACKUP_CONFIG_DIR", sandbox.path, 1)
        setenv("TELEVYBACKUP_DATA_DIR", sandbox.path, 1)
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let model = AppModel()
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        for (name, appearance, scheme) in [
            ("light", NSAppearance.Name.aqua, ColorScheme.light),
            ("dark", NSAppearance.Name.darkAqua, ColorScheme.dark),
        ] {
            let root = MainWindowRootView()
                .environment(\.appRuntime, model)
                .environmentObject(model.statusStore)
                .environmentObject(model.runHistoryStore)
                .environmentObject(model.taskPresentationStore)
                .environmentObject(model.diagnosticsStore)
                .environmentObject(model.mainWindowNavigationStore)
                .environment(\.colorScheme, scheme)
            let host = NSHostingView(rootView: root)
            let window = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 1024, height: 640),
                styleMask: [.titled, .closable],
                backing: .buffered,
                defer: false
            )
            window.isReleasedWhenClosed = false
            window.appearance = NSAppearance(named: appearance)
            window.contentView = host
            window.orderFront(nil)
            RunLoop.current.run(until: Date().addingTimeInterval(0.4))
            host.layoutSubtreeIfNeeded()
            guard let bitmap = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { exit(1) }
            host.cacheDisplay(in: host.bounds, to: bitmap)
            guard let png = bitmap.representation(using: .png, properties: [:]) else { exit(1) }
            try png.write(to: output.appendingPathComponent("backup-failure-\(name).png"))
            window.close()
        }
    }
}
