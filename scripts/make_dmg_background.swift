// Renders the background picture for the MediaForge disk image window.
//
// Usage: swift Scripts/make_dmg_background.swift <output.tiff> [--internal]
//
// The picture is drawn at 1x and 2x and combined into one TIFF so it is sharp on Retina displays. Positions
// here must match Packaging/dmg-settings.py. Finder draws icon names in black or white depending on the
// viewer's appearance setting, so each icon sits on a mid-grey tile that is legible with either.
import AppKit

let arguments = CommandLine.arguments
guard arguments.count >= 2 else {
    FileHandle.standardError.write(Data("usage: make_dmg_background.swift <output.tiff> [--internal]\n".utf8))
    exit(2)
}
let output = URL(fileURLWithPath: arguments[1])
let isInternal = arguments.contains("--internal")

// Window content size in points, and where Finder will place each icon's centre (measured from the top left).
// The internal build's window is taller, to make room for the "Read Me First" file beneath the instructions.
let canvas = CGSize(width: 680, height: isInternal ? 560 : 430)
let appCentre = CGPoint(x: 180, y: 200)
let applicationsCentre = CGPoint(x: 500, y: 200)
let tileSize = CGSize(width: 176, height: 196)
let readmeCentre = CGPoint(x: 340, y: 456)

func colour(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat, _ a: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: r, green: g, blue: b, alpha: a)
}

func gradient(_ colours: [CGColor], _ locations: [CGFloat]) -> CGGradient {
    CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB)!, colors: colours as CFArray, locations: locations)!
}

/// Converts a point measured from the top left (as Finder does) to Core Graphics' bottom-left origin.
func flipped(_ point: CGPoint) -> CGPoint {
    CGPoint(x: point.x, y: canvas.height - point.y)
}

func drawText(_ string: String, centredAt point: CGPoint, size: CGFloat, weight: NSFont.Weight, colour: NSColor, tracking: CGFloat = 0) {
    let attributes: [NSAttributedString.Key: Any] = [
        .font: NSFont.systemFont(ofSize: size, weight: weight),
        .foregroundColor: colour,
        .kern: tracking,
    ]
    let text = NSAttributedString(string: string, attributes: attributes)
    let bounds = text.size()
    let origin = flipped(point)
    text.draw(at: CGPoint(x: origin.x - bounds.width / 2, y: origin.y - bounds.height / 2))
}

func render(scale: CGFloat) -> NSBitmapImageRep {
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: Int(canvas.width * scale), pixelsHigh: Int(canvas.height * scale),
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0
    )!
    rep.size = canvas
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let ctx = NSGraphicsContext.current!.cgContext

    // Backdrop: the app's graphite, with a little warmth rising from the bottom.
    ctx.drawLinearGradient(
        gradient([colour(0.145, 0.150, 0.180), colour(0.085, 0.088, 0.108)], [0, 1]),
        start: CGPoint(x: 0, y: canvas.height), end: .zero, options: []
    )
    ctx.drawRadialGradient(
        gradient([colour(1.0, 0.48, 0.22, 0.22), colour(1.0, 0.40, 0.30, 0)], [0, 1]),
        startCenter: CGPoint(x: canvas.width / 2, y: -80), startRadius: 0,
        endCenter: CGPoint(x: canvas.width / 2, y: -80), endRadius: 460, options: []
    )

    // A tile behind each icon and its name.
    var tiles = [appCentre, applicationsCentre].map { centre in
        CGRect(x: centre.x - tileSize.width / 2, y: centre.y - 84, width: tileSize.width, height: tileSize.height)
    }
    if isInternal {
        tiles.append(CGRect(x: readmeCentre.x - 100, y: readmeCentre.y - 60, width: 200, height: 150))
    }
    for tile in tiles {
        let origin = flipped(CGPoint(x: tile.minX, y: tile.maxY))
        let rect = CGRect(origin: origin, size: tile.size)
        let path = CGPath(roundedRect: rect, cornerWidth: 22, cornerHeight: 22, transform: nil)
        ctx.addPath(path)
        ctx.setFillColor(colour(0.47, 0.48, 0.50))
        ctx.fillPath()
        ctx.addPath(path)
        ctx.setStrokeColor(colour(1, 1, 1, 0.14))
        ctx.setLineWidth(1)
        ctx.strokePath()
    }

    // Arrow from the app to the Applications folder.
    let y = canvas.height - (appCentre.y + 14)
    let startX = appCentre.x + tileSize.width / 2 + 22
    let endX = applicationsCentre.x - tileSize.width / 2 - 22
    let arrow = CGMutablePath()
    arrow.move(to: CGPoint(x: startX, y: y))
    arrow.addLine(to: CGPoint(x: endX - 4, y: y))
    arrow.move(to: CGPoint(x: endX - 20, y: y + 16))
    arrow.addLine(to: CGPoint(x: endX, y: y))
    arrow.addLine(to: CGPoint(x: endX - 20, y: y - 16))
    ctx.saveGState()
    ctx.addPath(arrow)
    ctx.setLineWidth(7)
    ctx.setLineCap(.round)
    ctx.setLineJoin(.round)
    ctx.replacePathWithStrokedPath()
    ctx.clip()
    ctx.drawLinearGradient(
        gradient([colour(1.00, 0.66, 0.24), colour(1.00, 0.33, 0.38)], [0, 1]),
        start: CGPoint(x: startX, y: y), end: CGPoint(x: endX, y: y), options: []
    )
    ctx.restoreGState()

    drawText(
        "Drag MediaForge to Applications to install",
        centredAt: CGPoint(x: canvas.width / 2, y: 352), size: 15, weight: .medium,
        colour: NSColor(white: 1, alpha: 0.82)
    )

    if isInternal {
        // Unmissable, and part of the picture so it can't be separated from the installer.
        let band = CGRect(x: 0, y: canvas.height - 40, width: canvas.width, height: 40)
        ctx.setFillColor(colour(1.0, 0.70, 0.13))
        ctx.fill(band)
        drawText(
            "INTERNAL TESTING BUILD  ·  NOT FOR DISTRIBUTION",
            centredAt: CGPoint(x: canvas.width / 2, y: 20), size: 13, weight: .bold,
            colour: NSColor(srgbRed: 0.16, green: 0.10, blue: 0.0, alpha: 1), tracking: 1.2
        )
        drawText(
            "Unsigned and not notarised by Apple. See “Read Me First” before installing.",
            centredAt: CGPoint(x: canvas.width / 2, y: 378), size: 12, weight: .regular,
            colour: NSColor(white: 1, alpha: 0.6)
        )
    }

    NSGraphicsContext.restoreGraphicsState()
    return rep
}

let image = NSImage(size: canvas)
image.addRepresentation(render(scale: 1))
image.addRepresentation(render(scale: 2))
guard let tiff = image.tiffRepresentation(using: .lzw, factor: 0) else {
    FileHandle.standardError.write(Data("could not encode background\n".utf8))
    exit(1)
}
try tiff.write(to: output)
print("Wrote \(output.path) (\(Int(canvas.width))x\(Int(canvas.height)) points, 1x and 2x)")
