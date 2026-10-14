import AppKit
import Foundation

@main
enum MenuBarStatusItemIconMatrixPreview {
    static func main() throws {
        guard CommandLine.arguments.count == 3, CommandLine.arguments[2] == "all" else {
            fputs("Usage: MenuBarStatusItemIconMatrixPreview <output.png> all\n", stderr)
            exit(2)
        }

        let output = URL(fileURLWithPath: CommandLine.arguments[1])
        let size = NSSize(width: 704, height: 432)
        let bitmap = makeBitmap(size: size, pixelsWide: 1408, pixelsHigh: 864)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
        NSColor(calibratedWhite: 0.95, alpha: 1).setFill()
        NSBezierPath(rect: NSRect(origin: .zero, size: size)).fill()

        let states: [(MenuBarActivityState, String)] = [
            (.idle, "Idle"), (.backup, "Backup"), (.restore, "Restore"),
            (.verify, "Verify"), (.bidirectional, "Sync"), (.failure, "Warning"),
        ]
        for (index, state) in states.enumerated() {
            label(state.1, at: NSPoint(x: 166 + CGFloat(index) * 88, y: 401), color: .black, size: 11)
        }

        for row in 0..<4 {
            let isDev = row >= 2
            let isDark = row % 2 == 1
            let rowRect = NSRect(x: 16, y: 306 - CGFloat(row) * 90, width: 672, height: 82)
            let background: NSColor = isDark
                ? NSColor(calibratedRed: 0.36, green: 0.49, blue: 0.64, alpha: 1)
                : .white
            let foreground: NSColor = isDark ? .white : .black
            background.setFill()
            NSBezierPath(rect: rowRect).fill()
            label(isDev ? "Dev" : "Release", at: NSPoint(x: 28, y: rowRect.midY + 3), color: foreground, size: 12)
            label(isDark ? "Dark" : "Light", at: NSPoint(x: 28, y: rowRect.midY - 14), color: foreground, size: 10)

            let appearance = NSAppearance(named: isDark ? .darkAqua : .aqua)
            for (column, state) in states.enumerated() {
                let icon = MenuBarStatusItemIcon.image(for: state.0, isDev: isDev, appearance: appearance)
                let x: CGFloat = 160 + CGFloat(column) * 88
                for (offset, dimension): (CGFloat, CGFloat) in [(2, 18), (28, 54)] {
                    // Rasterize each size independently so the enlargement preserves vector edges.
                    let pixels = Int(dimension * 2)
                    let iconBitmap = makeBitmap(size: icon.size, pixelsWide: pixels, pixelsHigh: pixels)
                    NSGraphicsContext.saveGraphicsState()
                    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: iconBitmap)
                    icon.draw(in: NSRect(origin: .zero, size: icon.size), from: .zero, operation: .sourceOver, fraction: 1)
                    if icon.isTemplate && isDark {
                        NSGraphicsContext.current!.compositingOperation = .sourceIn
                        foreground.setFill()
                        NSBezierPath(rect: NSRect(origin: .zero, size: icon.size)).fill()
                    }
                    NSGraphicsContext.restoreGraphicsState()

                    let rendered = NSImage(size: icon.size)
                    rendered.addRepresentation(iconBitmap)
                    rendered.draw(
                        in: NSRect(x: x + offset, y: rowRect.midY - dimension / 2, width: dimension, height: dimension),
                        from: .zero,
                        operation: .sourceOver,
                        fraction: 1
                    )
                }
            }
        }

        label(
            "Each pair: native 18pt canvas + 3x enlargement. Preview rendered at 2x pixel density.",
            at: NSPoint(x: 16, y: 13),
            color: NSColor(calibratedWhite: 0.3, alpha: 1),
            size: 10
        )
        NSGraphicsContext.restoreGraphicsState()
        try bitmap.representation(using: .png, properties: [:])!.write(to: output, options: .atomic)
    }

    private static func makeBitmap(size: NSSize, pixelsWide: Int, pixelsHigh: Int) -> NSBitmapImageRep {
        let bitmap = NSBitmapImageRep(
            bitmapDataPlanes: nil,
            pixelsWide: pixelsWide,
            pixelsHigh: pixelsHigh,
            bitsPerSample: 8,
            samplesPerPixel: 4,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: .deviceRGB,
            bytesPerRow: 0,
            bitsPerPixel: 0
        )!
        bitmap.size = size
        return bitmap
    }

    private static func label(_ text: String, at point: NSPoint, color: NSColor, size: CGFloat) {
        NSAttributedString(
            string: text,
            attributes: [.font: NSFont.systemFont(ofSize: size, weight: .medium), .foregroundColor: color]
        ).draw(at: point)
    }
}
