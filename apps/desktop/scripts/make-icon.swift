// Renders assets/icon.icns: the Lorevia mark on a white macOS tile.
// Usage: swift scripts/make-icon.swift <mark.png> <output.icns>
import AppKit

let arguments = CommandLine.arguments
guard arguments.count == 3, let mark = NSImage(contentsOfFile: arguments[1]) else {
  FileHandle.standardError.write("usage: make-icon.swift <mark.png> <output.icns>\n".data(using: .utf8)!)
  exit(1)
}

// Apple's macOS icon grid: a 824pt tile centred on a 1024pt canvas.
func render(_ size: Int) -> Data {
  let scale = CGFloat(size) / 1024
  let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size,
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
  NSGraphicsContext.saveGraphicsState()
  NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
  let context = NSGraphicsContext.current!.cgContext
  context.scaleBy(x: scale, y: scale)

  let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
  let shape = CGPath(roundedRect: tile, cornerWidth: 185, cornerHeight: 185, transform: nil)

  context.saveGState()
  context.setShadow(offset: CGSize(width: 0, height: -10), blur: 22,
    color: NSColor(white: 0, alpha: 0.22).cgColor)
  context.addPath(shape)
  context.setFillColor(NSColor.white.cgColor)
  context.fillPath()
  context.restoreGState()

  // A barely-there top-to-bottom falloff keeps the white tile from looking flat.
  context.saveGState()
  context.addPath(shape)
  context.clip()
  let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: [
    NSColor(white: 1, alpha: 1).cgColor, NSColor(white: 0.955, alpha: 1).cgColor,
  ] as CFArray, locations: [0, 1])!
  context.drawLinearGradient(gradient, start: CGPoint(x: 512, y: tile.maxY),
    end: CGPoint(x: 512, y: tile.minY), options: [])
  context.restoreGState()

  context.addPath(CGPath(roundedRect: tile.insetBy(dx: 1, dy: 1), cornerWidth: 184, cornerHeight: 184, transform: nil))
  context.setStrokeColor(NSColor(white: 0, alpha: 0.07).cgColor)
  context.setLineWidth(2)
  context.strokePath()

  // The mark's visual mass sits low in its square, so lift it slightly.
  let side: CGFloat = 470
  mark.draw(in: CGRect(x: 512 - side / 2, y: 512 - side / 2 + 14, width: side, height: side),
    from: .zero, operation: .sourceOver, fraction: 1)

  NSGraphicsContext.restoreGraphicsState()
  return rep.representation(using: .png, properties: [:])!
}

// ICNS contains PNG representations. Write the container directly so icon
// generation also works in build environments without iconutil's image service.
func length(_ value: Int) -> Data {
  var bigEndian = UInt32(value).bigEndian
  return withUnsafeBytes(of: &bigEndian) { Data($0) }
}
var representations = Data()
for (type, size) in [("icp4", 16), ("icp5", 32), ("icp6", 64),
                     ("ic07", 128), ("ic08", 256), ("ic09", 512), ("ic10", 1024),
                     ("ic11", 32), ("ic12", 64), ("ic13", 256), ("ic14", 512)] {
  let png = render(size)
  representations.append(type.data(using: .ascii)!)
  representations.append(length(png.count + 8))
  representations.append(png)
}
var icon = "icns".data(using: .ascii)!
icon.append(length(representations.count + 8))
icon.append(representations)
try icon.write(to: URL(fileURLWithPath: arguments[2]))
