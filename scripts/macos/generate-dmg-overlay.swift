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
let title = NSAttributedString(
    string: "Drag TelevyBackup to Applications",
    attributes: [
        .font: titleFont,
        .foregroundColor: NSColor(calibratedWhite: 1.0, alpha: 0.94),
        .kern: 0.2,
    ]
)
let brand = NSAttributedString(
    string: "TELEVYBACKUP",
    attributes: [
        .font: brandFont,
        .foregroundColor: NSColor(calibratedRed: 0.48, green: 0.82, blue: 1.0, alpha: 0.92),
        .kern: 2.0,
    ]
)

let titleRect = NSRect(x: 380 - title.size().width / 2, y: 91, width: title.size().width, height: title.size().height)
title.draw(in: titleRect)
let brandRect = NSRect(x: 380 - brand.size().width / 2, y: 52, width: brand.size().width, height: brand.size().height)
brand.draw(in: brandRect)

let arrow = NSBezierPath()
arrow.lineWidth = 3
arrow.lineCapStyle = .round
arrow.move(to: NSPoint(x: 300, y: 270))
arrow.line(to: NSPoint(x: 460, y: 270))
NSColor(calibratedRed: 0.48, green: 0.82, blue: 1.0, alpha: 0.95).setStroke()
arrow.stroke()

let head = NSBezierPath()
head.lineWidth = 3
head.lineCapStyle = .round
head.move(to: NSPoint(x: 447, y: 282))
head.line(to: NSPoint(x: 460, y: 270))
head.line(to: NSPoint(x: 447, y: 258))
head.stroke()

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
