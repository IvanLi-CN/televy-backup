import AppKit

enum UISnapshot {
    private static func sanitizeFileComponent(_ s: String) -> String {
        let trimmed = s.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { return "untitled" }
        let replaced = trimmed
            .replacingOccurrences(of: "/", with: "-")
            .replacingOccurrences(of: ":", with: "-")
            .replacingOccurrences(of: "\n", with: "-")
        return replaced
    }

    private static func bitmapRepForWindowContent(_ window: NSWindow) -> NSBitmapImageRep? {
        guard let view = window.contentView else { return nil }
        let bounds = view.bounds
        guard bounds.width > 4, bounds.height > 4 else { return nil }

        // Most of our views rely on materials/transparent backgrounds. For a readable snapshot,
        // composite on a neutral background instead of leaving alpha as-is.
        let backgroundColor: NSColor = .windowBackgroundColor

        // Prefer PDF-rendering for panels like NSSavePanel/NSTexturedFullScreenWindow.
        // Some system windows do not render correctly with `cacheDisplay`.
        //
        // Note: NSOpenPanel sometimes renders blank via PDF in sandboxed/deterministic snapshot
        // mode; fall back to `cacheDisplay` for it.
        if window is NSPanel, !(window is NSOpenPanel) {
            let pdf = view.dataWithPDF(inside: bounds)
            if let img = NSImage(data: pdf) {
                let rep = NSBitmapImageRep(
                    bitmapDataPlanes: nil,
                    pixelsWide: Int(bounds.width * window.backingScaleFactor),
                    pixelsHigh: Int(bounds.height * window.backingScaleFactor),
                    bitsPerSample: 8,
                    samplesPerPixel: 4,
                    hasAlpha: true,
                    isPlanar: false,
                    colorSpaceName: .deviceRGB,
                    bytesPerRow: 0,
                    bitsPerPixel: 0
                )
                if let rep {
                    rep.size = bounds.size
                    NSGraphicsContext.saveGraphicsState()
                    if let ctx = NSGraphicsContext(bitmapImageRep: rep) {
                        NSGraphicsContext.current = ctx
                        backgroundColor.set()
                        NSBezierPath(rect: NSRect(origin: .zero, size: bounds.size)).fill()
                        img.draw(
                            in: NSRect(origin: .zero, size: bounds.size),
                            from: .zero,
                            operation: .copy,
                            fraction: 1.0
                        )
                    }
                    NSGraphicsContext.restoreGraphicsState()
                    return rep
                }
            }
        }

        let rep = view.bitmapImageRepForCachingDisplay(in: bounds) ??
            NSBitmapImageRep(
                bitmapDataPlanes: nil,
                pixelsWide: Int(bounds.width * window.backingScaleFactor),
                pixelsHigh: Int(bounds.height * window.backingScaleFactor),
                bitsPerSample: 8,
                samplesPerPixel: 4,
                hasAlpha: true,
                isPlanar: false,
                colorSpaceName: .deviceRGB,
                bytesPerRow: 0,
                bitsPerPixel: 0
            )
        guard let rep else { return nil }
        rep.size = bounds.size
        // Ensure a deterministic background even when the view tree renders with alpha.
        NSGraphicsContext.saveGraphicsState()
        if let ctx = NSGraphicsContext(bitmapImageRep: rep) {
            NSGraphicsContext.current = ctx
            backgroundColor.set()
            NSBezierPath(rect: NSRect(origin: .zero, size: bounds.size)).fill()
        }
        NSGraphicsContext.restoreGraphicsState()
        view.cacheDisplay(in: bounds, to: rep)
        return rep
    }

    static func captureVisibleWindows(to dir: URL, prefix: String) {
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)

        let windows = NSApp.windows.filter { w in
            w.isVisible && !w.isMiniaturized && w.alphaValue > 0.01 && w.contentView != nil
        }

        for (idx, w) in windows.enumerated() {
            let title = w.title
            let name: String
            if title == "Settings" {
                name = "settings"
            } else if title.localizedCaseInsensitiveContains("Export backup config") {
                name = "export"
            } else if title.localizedCaseInsensitiveContains("Import backup config") {
                name = "import"
            } else if title.isEmpty {
                name = "window-\(idx)"
            } else {
                name = sanitizeFileComponent(title)
            }

            let url = dir.appendingPathComponent("\(prefix)-\(name).png")
            guard let rep = bitmapRepForWindowContent(w) else { continue }
            guard let data = rep.representation(using: .png, properties: [:]) else { continue }
            try? data.write(to: url, options: [.atomic])
        }
    }
}
