// Draws the Windows installer's side panel (164×314) and header (150×57) as 24-bit BMP files, the format
// NSIS requires. Run on a Mac when the artwork changes: swift scripts/make_installer_images.swift
import AppKit

let root = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent().deletingLastPathComponent()
let icon = NSImage(contentsOf: root.appendingPathComponent("assets/icon-1024.png"))!

func render(width: Int, height: Int, draw: (CGContext) -> Void) -> Data {
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let ctx = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4, space: space,
                        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    draw(ctx)
    let pixels = ctx.data!.assumingMemoryBound(to: UInt8.self)
    let rowBytes = (width * 3 + 3) & ~3
    var bmp = Data()
    func u32(_ v: Int) { var x = UInt32(v).littleEndian; bmp.append(Data(bytes: &x, count: 4)) }
    func u16(_ v: Int) { var x = UInt16(v).littleEndian; bmp.append(Data(bytes: &x, count: 2)) }
    bmp.append(contentsOf: Array("BM".utf8)); u32(54 + rowBytes * height); u32(0); u32(54)
    u32(40); u32(width); u32(height); u16(1); u16(24); u32(0); u32(rowBytes * height); u32(2835); u32(2835); u32(0); u32(0)
    // CGContext rows run top to bottom in memory; BMP rows run bottom to top.
    for y in (0..<height).reversed() {
        var row = [UInt8]()
        for x in 0..<width {
            let p = pixels + (y * width + x) * 4
            row += [p[2], p[1], p[0]]
        }
        row += [UInt8](repeating: 0, count: rowBytes - width * 3)
        bmp.append(contentsOf: row)
    }
    return bmp
}

func colour(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat) -> CGColor { CGColor(srgbRed: r, green: g, blue: b, alpha: 1) }

func drawIcon(_ ctx: CGContext, in rect: CGRect) {
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: false)
    icon.draw(in: rect)
    NSGraphicsContext.restoreGraphicsState()
}

func drawText(_ ctx: CGContext, _ text: String, at point: CGPoint, size: CGFloat, weight: NSFont.Weight, alpha: CGFloat) {
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: false)
    let attributes: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: size, weight: weight), .foregroundColor: NSColor(white: 1, alpha: alpha)]
    NSAttributedString(string: text, attributes: attributes).draw(at: point)
    NSGraphicsContext.restoreGraphicsState()
}

let sidebar = render(width: 164, height: 314) { ctx in
    let gradient = CGGradient(colorsSpace: nil, colors: [colour(0.13, 0.11, 0.17), colour(0.07, 0.07, 0.09)] as CFArray, locations: [0, 1])!
    ctx.drawLinearGradient(gradient, start: CGPoint(x: 0, y: 314), end: CGPoint(x: 164, y: 0), options: [])
    let glow = CGGradient(colorsSpace: nil, colors: [CGColor(srgbRed: 1, green: 0.55, blue: 0.25, alpha: 0.35), CGColor(srgbRed: 1, green: 0.4, blue: 0.3, alpha: 0)] as CFArray, locations: [0, 1])!
    ctx.drawRadialGradient(glow, startCenter: CGPoint(x: 82, y: 210), startRadius: 0, endCenter: CGPoint(x: 82, y: 210), endRadius: 110, options: [])
    drawIcon(ctx, in: CGRect(x: 30, y: 158, width: 104, height: 104))
    drawText(ctx, "MediaForge", at: CGPoint(x: 26, y: 118), size: 19, weight: .bold, alpha: 0.95)
    drawText(ctx, "C2 Software", at: CGPoint(x: 49, y: 98), size: 11, weight: .regular, alpha: 0.55)
}
let header = render(width: 150, height: 57) { ctx in
    ctx.setFillColor(colour(1, 1, 1))
    ctx.fill(CGRect(x: 0, y: 0, width: 150, height: 57))
    drawIcon(ctx, in: CGRect(x: 100, y: 6, width: 45, height: 45))
}
try! sidebar.write(to: root.appendingPathComponent("src-tauri/windows/nsis-sidebar.bmp"))
try! header.write(to: root.appendingPathComponent("src-tauri/windows/nsis-header.bmp"))
print("Wrote src-tauri/windows/nsis-sidebar.bmp and nsis-header.bmp")
