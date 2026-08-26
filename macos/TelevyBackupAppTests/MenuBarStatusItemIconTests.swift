import AppKit
import Foundation

@discardableResult
private func expectMenuBarIcon(_ condition: @autoclosure () -> Bool, _ message: String) -> Bool {
    if !condition() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
    return true
}

private func testTemplateSemantics() {
    for activity in [
        MenuBarActivityState.idle,
        .backup,
        .restore,
        .verify,
        .bidirectional,
    ] {
        let image = MenuBarStatusItemIcon.image(for: activity, isDev: false)
        expectMenuBarIcon(image.isTemplate, "\(activity) should remain a monochrome template image")
        let expectedSize = activity == .idle ? MenuBarStatusItemIcon.idleSize : MenuBarStatusItemIcon.activeSize
        expectMenuBarIcon(image.size == expectedSize, "\(activity) image size changed")
    }

    let failure = MenuBarStatusItemIcon.image(for: .failure, isDev: false)
    expectMenuBarIcon(!failure.isTemplate, "failure must preserve its colored marker")
}

private func testFailureMarkerIsSingleRedColor() {
    let image = MenuBarStatusItemIcon.image(for: .failure, isDev: false)
    guard let tiff = image.tiffRepresentation,
          let bitmap = NSBitmapImageRep(data: tiff)
    else {
        expectMenuBarIcon(false, "could not rasterize failure icon")
        return
    }

    var hasRed = false
    var hasYellow = false
    var hasUnexpectedChromaticColor = false
    var baseContainsWarningColor = false
    for x in 0..<18 {
        for y in 0..<18 {
            guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB) else { continue }
            let isRed = color.redComponent - color.greenComponent > 0.15
                && color.redComponent - color.blueComponent > 0.15
            let isYellow = color.redComponent > 0.7
                && color.greenComponent > 0.55
                && color.blueComponent < 0.3
            hasRed = hasRed || isRed
            hasYellow = hasYellow || isYellow
            let isChromatic = max(color.redComponent, color.greenComponent, color.blueComponent)
                - min(color.redComponent, color.greenComponent, color.blueComponent) > 0.2
            if isChromatic && !isRed {
                hasUnexpectedChromaticColor = true
            }
            if x < 7, isRed {
                baseContainsWarningColor = true
            }
        }
    }
    expectMenuBarIcon(hasRed, "failure marker must retain its red warning triangle")
    expectMenuBarIcon(!hasYellow, "failure marker must not introduce a second yellow color")
    expectMenuBarIcon(!hasUnexpectedChromaticColor, "failure marker must use one foreground color")
    expectMenuBarIcon(!baseContainsWarningColor, "failure color must not alter the product drive")
}

private func testActivityBadgesAreDistinct() {
    let idle = MenuBarStatusItemIcon.image(for: .idle, isDev: false).tiffRepresentation
    let activities: [MenuBarActivityState] = [.backup, .restore, .verify, .bidirectional]
    let renders = activities.compactMap { activity in
        MenuBarStatusItemIcon.image(for: activity, isDev: false).tiffRepresentation
    }

    expectMenuBarIcon(renders.count == activities.count, "activity badge rasterization failed")
    for render in renders {
        expectMenuBarIcon(render != idle, "activity badge must differ from the idle drive")
    }
    for (index, render) in renders.enumerated() {
        for comparison in renders.dropFirst(index + 1) {
            expectMenuBarIcon(render != comparison, "each activity badge needs a unique rendering")
        }
    }
}

private func testActivityBadgesPreserveProductDriveGeometry() {
    expectMenuBarIcon(
        MenuBarStatusItemIcon.idleSize == NSSize(width: 18, height: 18),
        "idle drive must retain its 18pt intrinsic size"
    )
    expectMenuBarIcon(
        MenuBarStatusItemIcon.activeBaseRect.size == NSSize(width: 18, height: 18),
        "active drive must retain its 18pt intrinsic size"
    )
    expectMenuBarIcon(
        MenuBarStatusItemIcon.statusBadgeRect == NSRect(x: 7, y: 0, width: 11, height: 11),
        "status overlay must remain in the original icon's lower-right corner"
    )
    expectMenuBarIcon(
        MenuBarStatusItemIcon.statusClearanceRect == NSRect(x: 6, y: 0, width: 12, height: 12),
        "status clearance must remain a one-point separation from the product icon"
    )
    expectMenuBarIcon(
        MenuBarStatusItemIcon.statusBadgeRect.maxX == MenuBarStatusItemIcon.activeSize.width
            && MenuBarStatusItemIcon.statusBadgeRect.minY == 0,
        "status overlay must not reserve space on the right or below"
    )
}

private func testDevIdlePreservesTheExistingIcon() {
    let release = MenuBarStatusItemIcon.image(for: .idle, isDev: false)
    let dev = MenuBarStatusItemIcon.image(for: .idle, isDev: true)
    expectMenuBarIcon(dev.isTemplate, "dev idle icon must retain template semantics")
    expectMenuBarIcon(dev.size == MenuBarStatusItemIcon.idleSize, "dev idle must not reserve status-marker space")
    expectMenuBarIcon(
        dev.tiffRepresentation != release.tiffRepresentation,
        "dev idle must retain its existing DEV marker"
    )
}

@main
enum MenuBarStatusItemIconTestsMain {
    static func main() {
        testTemplateSemantics()
        testFailureMarkerIsSingleRedColor()
        testActivityBadgesAreDistinct()
        testActivityBadgesPreserveProductDriveGeometry()
        testDevIdlePreservesTheExistingIcon()
        print("OK: MenuBarStatusItemIconTests")
    }
}
