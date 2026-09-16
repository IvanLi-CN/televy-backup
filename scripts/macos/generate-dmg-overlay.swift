import AppKit
import Foundation

struct WindowLayout: Decodable {
    let width: Int
    let height: Int
}

struct OverlayLayout: Decodable {
    let brandText: String
    let instruction: String
    let instructionCenter: [Double]
    let brandCenter: [Double]
    let labelBackplateCenters: [[Double]]
    let arrowStart: [Double]
    let arrowEnd: [Double]
}

struct DMGLayout: Decodable {
    let window: WindowLayout
    let overlay: OverlayLayout
}

let arguments = CommandLine.arguments
let layoutURL: URL
let backgroundURL: URL?
let outputURL: URL
if arguments.count == 4 && arguments[1] == "--layout" {
    layoutURL = URL(fileURLWithPath: arguments[2])
    backgroundURL = nil
    outputURL = URL(fileURLWithPath: arguments[3])
} else if arguments.count == 6 && arguments[1] == "--layout" && arguments[3] == "--background" {
    layoutURL = URL(fileURLWithPath: arguments[2])
    backgroundURL = URL(fileURLWithPath: arguments[4])
    outputURL = URL(fileURLWithPath: arguments[5])
} else {
    fputs("usage: generate-dmg-overlay.swift --layout LAYOUT [--background BACKGROUND] OUTPUT\n", stderr)
    exit(2)
}

let layout: DMGLayout
do {
    let data = try Data(contentsOf: layoutURL)
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    layout = try decoder.decode(DMGLayout.self, from: data)
} catch {
    fputs("failed to read DMG layout: \(error)\n", stderr)
    exit(1)
}

func point(_ values: [Double], name: String) -> NSPoint {
    guard values.count == 2 else {
        fputs("DMG layout point must contain two values: \(name)\n", stderr)
        exit(1)
    }
    return NSPoint(x: values[0], y: values[1])
}

let canvas = NSSize(width: layout.window.width, height: layout.window.height)
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
    string: layout.overlay.instruction,
    attributes: [
        .font: titleFont,
        .foregroundColor: NSColor(calibratedWhite: 1.0, alpha: 0.94),
        .kern: 0.2,
        .shadow: textShadow,
    ]
)
let brand = NSAttributedString(
    string: layout.overlay.brandText,
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

for (index, center) in layout.overlay.labelBackplateCenters.enumerated() {
    drawLabelBackplate(center: point(center, name: "overlay.label_backplate_centers[\(index)]"))
}

let instructionCenter = point(layout.overlay.instructionCenter, name: "overlay.instruction_center")
let brandCenter = point(layout.overlay.brandCenter, name: "overlay.brand_center")
let titleRect = NSRect(x: instructionCenter.x - title.size().width / 2, y: instructionCenter.y - title.size().height / 2, width: title.size().width, height: title.size().height)
title.draw(in: titleRect)
let brandRect = NSRect(x: brandCenter.x - brand.size().width / 2, y: brandCenter.y - brand.size().height / 2, width: brand.size().width, height: brand.size().height)
brand.draw(in: brandRect)

func drawArrow(lineWidth: CGFloat, color: NSColor) {
    let start = point(layout.overlay.arrowStart, name: "overlay.arrow_start")
    let end = point(layout.overlay.arrowEnd, name: "overlay.arrow_end")
    let headLength: CGFloat = 15
    let shaft = NSBezierPath()
    shaft.lineWidth = lineWidth
    shaft.lineCapStyle = .round
    shaft.move(to: start)
    shaft.line(to: end)
    color.setStroke()
    shaft.stroke()

    let head = NSBezierPath()
    head.lineWidth = lineWidth
    head.lineCapStyle = .round
    head.lineJoinStyle = .round
    head.move(to: NSPoint(x: end.x - headLength, y: end.y + headLength))
    head.line(to: end)
    head.line(to: NSPoint(x: end.x - headLength, y: end.y - headLength))
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
