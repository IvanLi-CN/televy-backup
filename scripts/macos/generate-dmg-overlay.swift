import AppKit
import Foundation

let arguments = CommandLine.arguments
let backgroundURL: URL?
let outputURL: URL
if arguments.count == 2 {
    backgroundURL = nil
    outputURL = URL(fileURLWithPath: arguments[1])
} else if arguments.count == 4 && arguments[1] == "--background" {
    backgroundURL = URL(fileURLWithPath: arguments[2])
    outputURL = URL(fileURLWithPath: arguments[3])
} else {
    fputs("usage: generate-dmg-overlay.swift [--background BACKGROUND] OUTPUT\n", stderr)
    exit(2)
}
let canvas = NSSize(width: 760, height: 520)
let bitmap = NSBitmapImageRep(
    bitmapDataPlanes: nil,
    pixelsWide: Int(canvas.width),
    pixelsHigh: Int(canvas.height),
    bitsPerSample: 8,
    samplesPerPixel: 4,
    hasAlpha: true,
    isPlanar: false,
    colorSpaceName: .deviceRGB,
    bitmapFormat: [],
    bytesPerRow: 0,
    bitsPerPixel: 0
)

guard let bitmap else {
    fputs("failed to allocate overlay bitmap\n", stderr)
    exit(1)
}

guard let context = NSGraphicsContext(bitmapImageRep: bitmap) else {
    fputs("failed to create overlay graphics context\n", stderr)
    exit(1)
}

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = context
let canvasRect = NSRect(origin: .zero, size: canvas)
if let backgroundURL, let background = NSImage(contentsOf: backgroundURL) {
    background.draw(in: canvasRect, from: .zero, operation: .copy, fraction: 1.0)
} else {
    NSColor.clear.setFill()
    canvasRect.fill()
}

let titleFont = NSFont(name: "Helvetica Neue", size: 18) ?? NSFont.systemFont(ofSize: 18)
let brandFont = NSFont(name: "Helvetica Neue", size: 13) ?? NSFont.systemFont(ofSize: 13)
let textShadow: NSShadow = {
    let shadow = NSShadow()
    shadow.shadowColor = NSColor(calibratedWhite: 0.0, alpha: 0.72)
    shadow.shadowBlurRadius = 3
    shadow.shadowOffset = NSSize(width: 0, height: -1)
    return shadow
}()
let title = NSAttributedString(
    string: "Drag TelevyBackup to Applications",
    attributes: [
        .font: titleFont,
        .foregroundColor: NSColor(calibratedWhite: 1.0, alpha: 0.94),
        .kern: 0.2,
        .shadow: textShadow,
    ]
)
let brand = NSAttributedString(
    string: "TELEVYBACKUP",
    attributes: [
        .font: brandFont,
        .foregroundColor: NSColor(calibratedRed: 0.48, green: 0.82, blue: 1.0, alpha: 0.92),
        .kern: 2.0,
        .shadow: textShadow,
    ]
)

func drawLabelBackplate(center: NSPoint) {
    let plate = NSRect(x: center.x - 80, y: center.y - 14, width: 160, height: 28)
    let path = NSBezierPath(roundedRect: plate, xRadius: 14, yRadius: 14)
    let gradient = NSGradient(colors: [
        NSColor(calibratedWhite: 1.0, alpha: 0.60),
        NSColor(calibratedWhite: 0.94, alpha: 0.28),
    ])
    gradient?.draw(in: path, angle: 90)
    NSColor(calibratedWhite: 1.0, alpha: 0.18).setStroke()
    path.lineWidth = 1
    path.stroke()
}

drawLabelBackplate(center: NSPoint(x: 210, y: 160))
drawLabelBackplate(center: NSPoint(x: 550, y: 160))

let titleRect = NSRect(x: 380 - title.size().width / 2, y: 420 - title.size().height / 2, width: title.size().width, height: title.size().height)
title.draw(in: titleRect)
let brandRect = NSRect(x: 380 - brand.size().width / 2, y: 382 - brand.size().height / 2, width: brand.size().width, height: brand.size().height)
brand.draw(in: brandRect)

func drawArrow(lineWidth: CGFloat, color: NSColor) {
    let shaft = NSBezierPath()
    shaft.lineWidth = lineWidth
    shaft.lineCapStyle = .round
    shaft.move(to: NSPoint(x: 300, y: 270))
    shaft.line(to: NSPoint(x: 460, y: 270))
    color.setStroke()
    shaft.stroke()

    let head = NSBezierPath()
    head.lineWidth = lineWidth
    head.lineCapStyle = .round
    head.lineJoinStyle = .round
    head.move(to: NSPoint(x: 445, y: 285))
    head.line(to: NSPoint(x: 460, y: 270))
    head.line(to: NSPoint(x: 445, y: 255))
    head.stroke()
}

drawArrow(lineWidth: 10, color: NSColor(calibratedWhite: 0.0, alpha: 0.38))
drawArrow(lineWidth: 7, color: NSColor(calibratedRed: 0.26, green: 0.73, blue: 0.96, alpha: 0.92))
drawArrow(lineWidth: 3, color: NSColor(calibratedWhite: 0.92, alpha: 0.96))

NSGraphicsContext.restoreGraphicsState()

guard let data = bitmap.representation(using: .png, properties: [:]) else {
    fputs("failed to encode overlay PNG\n", stderr)
    exit(1)
}

do {
    try data.write(to: outputURL, options: .atomic)
} catch {
    fputs("failed to write overlay: \(error)\n", stderr)
    exit(1)
}
