import AppKit

enum MenuBarStatusItemIcon {
    static let idleSize = NSSize(width: 18, height: 18)
    static let activeSize = NSSize(width: 18, height: 18)
    static let activeBaseRect = NSRect(origin: .zero, size: idleSize)
    // The badge is flush with the image's lower-right edge. Clearance only separates it from
    // the product icon on the left and above; there is intentionally none to the right or below.
    static let statusClearanceRect = NSRect(x: 6, y: 0, width: 12, height: 12)
    static let statusBadgeRect = NSRect(x: 7, y: 0, width: 11, height: 11)

    static func image(
        for activity: MenuBarActivityState,
        isDev: Bool,
        appearance: NSAppearance? = nil
    ) -> NSImage {
        if activity == .idle {
            return baseImage(size: idleSize, baseRect: NSRect(origin: .zero, size: idleSize), isDev: isDev)
        }

        let image = NSImage(size: activeSize, flipped: false) { _ in
            let foregroundColor = activity == .failure ? darkFailureBaseColor(for: appearance) : nil
            drawOriginalBase(
                in: activeBaseRect,
                isDev: isDev,
                activity: activity,
                foregroundColor: foregroundColor
            )
            if activity == .failure {
                clearFailureBadge(in: statusClearanceRect)
                drawFailureBadge(in: statusBadgeRect)
            } else {
                clearActivityBadge(in: statusClearanceRect)
                drawActivityBadge(for: activity, in: statusBadgeRect)
            }
            return true
        }
        image.isTemplate = activity != .failure
        return image
    }

    private static func baseImage(size: NSSize, baseRect: NSRect, isDev: Bool) -> NSImage {
        let image = NSImage(size: size, flipped: false) { _ in
            drawOriginalBase(in: baseRect, isDev: isDev, activity: .idle)
            return true
        }
        image.isTemplate = true
        return image
    }

    // Normal and activity variants use the monochrome template brand mark. On a dark menu bar,
    // failure uses its foreground color on the same base alpha geometry.
    private static func drawOriginalBase(
        in rect: NSRect,
        isDev: Bool,
        activity: MenuBarActivityState,
        foregroundColor: NSColor? = nil
    ) {
        let symbol = BrandMarkAsset.image(for: .template)
        if let foregroundColor {
            drawTemplateSymbol(symbol, in: rect, foregroundColor: foregroundColor)
        } else {
            symbol?.draw(
                in: rect,
                from: .zero,
                operation: .sourceOver,
                fraction: 1.0,
                respectFlipped: false,
                hints: nil
            )
        }

        guard isDev else { return }
        drawOriginalDevBadge(in: rect, foregroundColor: foregroundColor ?? .black)
    }

    private static func drawTemplateSymbol(
        _ symbol: NSImage?,
        in rect: NSRect,
        foregroundColor: NSColor
    ) {
        symbol?.draw(
            in: rect,
            from: .zero,
            operation: .sourceOver,
            fraction: 1.0,
            respectFlipped: false,
            hints: nil
        )
        guard let context = NSGraphicsContext.current else { return }
        NSGraphicsContext.saveGraphicsState()
        context.compositingOperation = .sourceIn
        foregroundColor.setFill()
        NSBezierPath(rect: rect).fill()
        NSGraphicsContext.restoreGraphicsState()
    }

    private static func darkFailureBaseColor(for appearance: NSAppearance?) -> NSColor? {
        guard let appearance else { return nil }
        return appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? .white : nil
    }

    private static func drawOriginalDevBadge(in rect: NSRect, foregroundColor: NSColor) {
        let badgeRect = NSRect(
            x: rect.maxX - 15,
            y: rect.minY + 1,
            width: 14,
            height: 7
        )
        foregroundColor.setFill()
        NSBezierPath(roundedRect: badgeRect, xRadius: 2, yRadius: 2).fill()

        guard let context = NSGraphicsContext.current else { return }
        NSGraphicsContext.saveGraphicsState()
        context.compositingOperation = .destinationOut
        let text = NSAttributedString(
            string: "DEV",
            attributes: [
                .font: NSFont.monospacedSystemFont(ofSize: 6, weight: .bold),
                .foregroundColor: foregroundColor,
            ]
        )
        let textSize = text.size()
        text.draw(at: NSPoint(
            x: badgeRect.midX - textSize.width / 2,
            y: badgeRect.midY - textSize.height / 2 - 0.5
        ))
        NSGraphicsContext.restoreGraphicsState()
    }

    private static func drawActivityBadge(for activity: MenuBarActivityState, in rect: NSRect) {
        NSColor.black.setFill()
        NSBezierPath(ovalIn: rect).fill()

        switch activity {
        case .backup: clearUpArrow(in: rect)
        case .restore: clearDownArrow(in: rect)
        case .verify: clearCheckmark(in: rect)
        case .bidirectional: clearBidirectionalArrows(in: rect)
        case .idle, .failure:
            return
        }
    }

    private static func clearActivityBadge(in rect: NSRect) {
        clearShape {
            NSBezierPath(ovalIn: rect).fill()
        }
    }

    private static func clearFailureBadge(in rect: NSRect) {
        clearShape {
            failureTriangle(in: rect).fill()
        }
    }

    private static func drawFailureBadge(in rect: NSRect) {
        let triangle = failureTriangle(in: rect)
        // The failure marker has one foreground color. The exclamation is a transparent knockout,
        // so it remains distinct from the unchanged monochrome product icon without adding yellow.
        NSColor.systemRed.setFill()
        triangle.fill()

        clearFailureExclamation(in: rect)
    }

    private static func clearFailureExclamation(in rect: NSRect) {
        let scale = rect.width / 10
        clearShape {
            NSBezierPath(
                roundedRect: NSRect(
                    x: rect.midX - 1.15 * scale,
                    y: rect.minY + 3.45 * scale,
                    width: 2.3 * scale,
                    height: 4.4 * scale
                ),
                xRadius: 1.15 * scale,
                yRadius: 1.15 * scale
            ).fill()
            NSBezierPath(ovalIn: NSRect(
                x: rect.midX - 1.15 * scale,
                y: rect.minY + 1.35 * scale,
                width: 2.3 * scale,
                height: 1.9 * scale
            )).fill()
        }
    }

    private static func failureTriangle(in rect: NSRect) -> NSBezierPath {
        let scale = rect.width / 10
        let triangle = NSBezierPath()
        triangle.move(to: NSPoint(x: rect.midX, y: rect.maxY - 0.3 * scale))
        triangle.line(to: NSPoint(x: rect.minX + 0.5 * scale, y: rect.minY + 0.7 * scale))
        triangle.line(to: NSPoint(x: rect.maxX - 0.5 * scale, y: rect.minY + 0.7 * scale))
        triangle.close()
        return triangle
    }

    private static func clearShape(_ draw: () -> Void) {
        guard let context = NSGraphicsContext.current else { return }
        NSGraphicsContext.saveGraphicsState()
        context.compositingOperation = .destinationOut
        draw()
        NSGraphicsContext.restoreGraphicsState()
    }

    private static func clearUpArrow(in rect: NSRect) {
        clearShape {
            upArrowPath(in: rect).fill()
        }
    }

    private static func drawUpArrow(in rect: NSRect) {
        NSColor.black.setFill()
        upArrowPath(in: rect).fill()
    }

    private static func upArrowPath(in rect: NSRect) -> NSBezierPath {
        let path = NSBezierPath()
        path.move(to: NSPoint(x: rect.midX, y: rect.maxY - 1.4))
        path.line(to: NSPoint(x: rect.minX + 2.1, y: rect.midY + 0.1))
        path.line(to: NSPoint(x: rect.minX + 3.9, y: rect.midY + 0.1))
        path.line(to: NSPoint(x: rect.minX + 3.9, y: rect.minY + 1.6))
        path.line(to: NSPoint(x: rect.maxX - 3.9, y: rect.minY + 1.6))
        path.line(to: NSPoint(x: rect.maxX - 3.9, y: rect.midY + 0.1))
        path.line(to: NSPoint(x: rect.maxX - 2.1, y: rect.midY + 0.1))
        path.close()
        return path
    }

    private static func clearDownArrow(in rect: NSRect) {
        clearShape {
            downArrowPath(in: rect).fill()
        }
    }

    private static func drawDownArrow(in rect: NSRect) {
        NSColor.black.setFill()
        downArrowPath(in: rect).fill()
    }

    private static func downArrowPath(in rect: NSRect) -> NSBezierPath {
        let path = NSBezierPath()
        path.move(to: NSPoint(x: rect.midX, y: rect.minY + 1.4))
        path.line(to: NSPoint(x: rect.minX + 2.1, y: rect.midY - 0.1))
        path.line(to: NSPoint(x: rect.minX + 3.9, y: rect.midY - 0.1))
        path.line(to: NSPoint(x: rect.minX + 3.9, y: rect.maxY - 1.6))
        path.line(to: NSPoint(x: rect.maxX - 3.9, y: rect.maxY - 1.6))
        path.line(to: NSPoint(x: rect.maxX - 3.9, y: rect.midY - 0.1))
        path.line(to: NSPoint(x: rect.maxX - 2.1, y: rect.midY - 0.1))
        path.close()
        return path
    }

    private static func clearCheckmark(in rect: NSRect) {
        clearShape {
            checkmarkPath(in: rect).stroke()
        }
    }

    private static func drawCheckmark(in rect: NSRect) {
        NSColor.black.setStroke()
        checkmarkPath(in: rect).stroke()
    }

    private static func checkmarkPath(in rect: NSRect) -> NSBezierPath {
        let path = NSBezierPath()
        path.move(to: NSPoint(x: rect.minX + 2.2, y: rect.midY - 0.2))
        path.line(to: NSPoint(x: rect.minX + 4.2, y: rect.minY + 2.4))
        path.line(to: NSPoint(x: rect.maxX - 1.9, y: rect.maxY - 2.0))
        path.lineWidth = 2.05
        path.lineCapStyle = .round
        path.lineJoinStyle = .round
        return path
    }

    private static func clearBidirectionalArrows(in rect: NSRect) {
        clearShape {
            bidirectionalPaths(in: rect).forEach { $0.fill() }
        }
    }

    private static func drawBidirectionalArrows(in rect: NSRect) {
        NSColor.black.setFill()
        bidirectionalPaths(in: rect).forEach { $0.fill() }
    }

    private static func bidirectionalPaths(in rect: NSRect) -> [NSBezierPath] {
        let up = NSBezierPath()
        up.move(to: NSPoint(x: rect.minX + 3.4, y: rect.maxY - 1.8))
        up.line(to: NSPoint(x: rect.minX + 1.6, y: rect.midY + 0.1))
        up.line(to: NSPoint(x: rect.minX + 2.8, y: rect.midY + 0.1))
        up.line(to: NSPoint(x: rect.minX + 2.8, y: rect.minY + 1.8))
        up.line(to: NSPoint(x: rect.minX + 4.1, y: rect.minY + 1.8))
        up.line(to: NSPoint(x: rect.minX + 4.1, y: rect.midY + 0.1))
        up.line(to: NSPoint(x: rect.minX + 5.2, y: rect.midY + 0.1))
        up.close()

        let down = NSBezierPath()
        down.move(to: NSPoint(x: rect.maxX - 3.4, y: rect.minY + 1.8))
        down.line(to: NSPoint(x: rect.maxX - 1.6, y: rect.midY - 0.1))
        down.line(to: NSPoint(x: rect.maxX - 2.8, y: rect.midY - 0.1))
        down.line(to: NSPoint(x: rect.maxX - 2.8, y: rect.maxY - 1.8))
        down.line(to: NSPoint(x: rect.maxX - 4.1, y: rect.maxY - 1.8))
        down.line(to: NSPoint(x: rect.maxX - 4.1, y: rect.midY - 0.1))
        down.line(to: NSPoint(x: rect.maxX - 5.2, y: rect.midY - 0.1))
        down.close()

        return [up, down]
    }
}

private enum MenuBarStatusItemIconAppearanceVariant: Hashable {
    case template
    case lightFailure
    case darkFailure

    init(activity: MenuBarActivityState, appearance: NSAppearance?) {
        guard activity == .failure else {
            self = .template
            return
        }
        self = appearance?.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            ? .darkFailure
            : .lightFailure
    }
}

private struct MenuBarStatusItemIconKey: Hashable {
    let activity: MenuBarActivityState
    let isDev: Bool
    let appearance: MenuBarStatusItemIconAppearanceVariant

    init(activity: MenuBarActivityState, isDev: Bool, appearance: NSAppearance?) {
        self.activity = activity
        self.isDev = isDev
        self.appearance = MenuBarStatusItemIconAppearanceVariant(activity: activity, appearance: appearance)
    }
}

final class MenuBarStatusItemImageStore {
    private var appliedKey: MenuBarStatusItemIconKey?
    private var cachedImages: [MenuBarStatusItemIconKey: NSImage] = [:]

    func imageIfNeeded(
        for activity: MenuBarActivityState,
        isDev: Bool,
        appearance: NSAppearance?
    ) -> NSImage? {
        let key = MenuBarStatusItemIconKey(activity: activity, isDev: isDev, appearance: appearance)
        guard key != appliedKey else { return nil }
        appliedKey = key

        if let cachedImage = cachedImages[key] {
            return cachedImage
        }

        let image = MenuBarStatusItemIcon.image(for: activity, isDev: isDev, appearance: appearance)
        cachedImages[key] = image
        return image
    }
}
