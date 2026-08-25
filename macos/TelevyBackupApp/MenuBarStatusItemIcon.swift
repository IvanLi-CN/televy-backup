import AppKit

enum MenuBarStatusItemIcon {
    static func image(for activity: MenuBarActivityState, isDev: Bool) -> NSImage {
        let base = NSImage(
            systemSymbolName: "externaldrive",
            accessibilityDescription: "TelevyBackup \(activity.accessibilityDescription)"
        )
        let size = NSSize(width: 18, height: 18)
        let image = NSImage(size: size, flipped: false) { rect in
            base?.draw(
                in: rect,
                from: .zero,
                operation: .sourceOver,
                fraction: 1.0,
                respectFlipped: false,
                hints: nil
            )

            let badgeSymbol: String?
            switch activity {
            case .idle: badgeSymbol = nil
            case .failure: badgeSymbol = "exclamationmark.triangle"
            case .backup: badgeSymbol = "arrow.up"
            case .restore: badgeSymbol = "arrow.down"
            case .verify: badgeSymbol = "checkmark.shield"
            case .bidirectional: badgeSymbol = "arrow.up.arrow.down"
            }
            if let badgeSymbol,
               let badge = NSImage(
                systemSymbolName: badgeSymbol,
                accessibilityDescription: activity.accessibilityDescription
               )?.withSymbolConfiguration(NSImage.SymbolConfiguration(pointSize: 8, weight: .bold))
            {
                badge.draw(
                    in: NSRect(x: 1, y: rect.maxY - 9, width: 8, height: 8),
                    from: .zero,
                    operation: .sourceOver,
                    fraction: 1.0,
                    respectFlipped: false,
                    hints: nil
                )
            }

            guard isDev else { return true }

            let badgeWidth: CGFloat = 14
            let badgeHeight: CGFloat = 7
            let inset: CGFloat = 1
            let badgeRect = NSRect(
                x: rect.maxX - badgeWidth - inset,
                y: inset,
                width: badgeWidth,
                height: badgeHeight
            )

            let badgePath = NSBezierPath(roundedRect: badgeRect, xRadius: 2, yRadius: 2)
            NSColor.black.setFill()
            badgePath.fill()

            guard let context = NSGraphicsContext.current else { return true }
            NSGraphicsContext.saveGraphicsState()
            context.compositingOperation = .destinationOut
            let attrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.monospacedSystemFont(ofSize: 6, weight: .bold),
                .foregroundColor: NSColor.black,
            ]
            let text = NSAttributedString(string: "DEV", attributes: attrs)
            let textSize = text.size()
            let textPoint = NSPoint(
                x: badgeRect.midX - textSize.width / 2.0,
                y: badgeRect.midY - textSize.height / 2.0 - 0.5
            )
            text.draw(at: textPoint)
            NSGraphicsContext.restoreGraphicsState()

            return true
        }
        image.isTemplate = true
        return image
    }
}
