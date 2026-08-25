import AppKit
import Foundation

@main
enum MenuBarStatusItemIconPreview {
    static func main() {
        guard CommandLine.arguments.count == 2 else {
            fputs("Usage: MenuBarStatusItemIconPreview <output.png>\n", stderr)
            exit(2)
        }

        let output = URL(fileURLWithPath: CommandLine.arguments[1])
        let size = NSSize(width: 900, height: 180)
        let image = NSImage(size: size)
        image.lockFocus()
        NSColor(calibratedWhite: 0.95, alpha: 1).setFill()
        NSBezierPath(rect: NSRect(origin: .zero, size: size)).fill()

        let barWidth: CGFloat = 860
        let barHeight: CGFloat = 46

        let states: [(MenuBarActivityState, String)] = [
            (.idle, ""),
            (.backup, "\u{2191} 12.4 MB/s"),
            (.restore, "\u{2193} 8.1 MB/s"),
            (.verify, ""),
            (.bidirectional, "\u{2191} 12.4 MB/s  \u{2193} 8.1 MB/s"),
            (.failure, "\u{2191} 12.4 MB/s  \u{2193} 8.1 MB/s"),
        ]
        let slotWidth = barWidth / 3

        for (index, state) in states.enumerated() {
            let row = index / 3
            let column = index % 3
            let bar = NSRect(
                x: 20,
                y: row == 0 ? 106 : 52,
                width: barWidth,
                height: barHeight
            )
            if column == 0 {
                NSColor.white.setFill()
                NSBezierPath(roundedRect: bar, xRadius: 6, yRadius: 6).fill()
            }
            let slot = NSRect(
                x: bar.minX + CGFloat(column) * slotWidth,
                y: bar.minY,
                width: slotWidth,
                height: bar.height
            )
            if column > 0 {
                NSColor(calibratedWhite: 0.86, alpha: 1).setFill()
                NSBezierPath(rect: NSRect(x: slot.minX, y: slot.minY + 10, width: 1, height: slot.height - 20)).fill()
            }

            let icon = MenuBarStatusItemIcon.image(for: state.0, isDev: true)
            icon.draw(
                in: NSRect(x: slot.minX + 14, y: slot.midY - 14, width: 28, height: 28),
                from: .zero,
                operation: .sourceOver,
                fraction: 1.0,
                respectFlipped: false,
                hints: nil
            )
            let label = NSAttributedString(
                string: state.0.accessibilityDescription,
                attributes: [
                    .font: NSFont.systemFont(ofSize: 11, weight: .medium),
                    .foregroundColor: NSColor(calibratedWhite: 0.12, alpha: 1),
                ]
            )
            label.draw(at: NSPoint(x: slot.minX + 50, y: slot.midY + 3))
            let title = NSAttributedString(
                string: state.1,
                attributes: [
                    .font: NSFont.monospacedSystemFont(ofSize: 9, weight: .regular),
                    .foregroundColor: NSColor(calibratedWhite: 0.34, alpha: 1),
                ]
            )
            title.draw(at: NSPoint(x: slot.minX + 50, y: slot.midY - 12))
        }

        let caption = NSAttributedString(
            string: "Static status badges with non-overlapping DEV marker",
            attributes: [
                .font: NSFont.systemFont(ofSize: 11, weight: .medium),
                .foregroundColor: NSColor(calibratedWhite: 0.34, alpha: 1),
            ]
        )
        caption.draw(at: NSPoint(x: 20, y: 14))
        image.unlockFocus()

        guard let tiff = image.tiffRepresentation,
              let bitmap = NSBitmapImageRep(data: tiff),
              let png = bitmap.representation(using: .png, properties: [:])
        else {
            fputs("Failed to create PNG data\n", stderr)
            exit(1)
        }

        do {
            try FileManager.default.createDirectory(at: output.deletingLastPathComponent(), withIntermediateDirectories: true)
            try png.write(to: output, options: .atomic)
        } catch {
            fputs("Failed to write PNG: \(error)\n", stderr)
            exit(1)
        }
    }
}
