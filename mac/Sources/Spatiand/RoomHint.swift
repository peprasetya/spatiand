//  RoomHint.swift — what to say to someone in the glasses when there is nothing to look at.
//
//  A room with no windows is black, and black glasses look like glasses that are not working. So
//  while there is nothing in it, a small panel says what Spatiand is waiting for and how to get
//  the menu, and goes away the moment a window arrives.

import AppKit
import Metal

final class RoomHint {
    static let panelID: UInt16 = 0xFFF3

    private unowned let controller: RoomController
    private(set) var shown = false
    private var lastText = ""
    private let size = CGSize(width: 1100, height: 300)

    init(controller: RoomController) { self.controller = controller }

    /// Show or take away the panel to suit what the room holds now.
    func update() {
        let model = Model.shared
        let wanted = controller.active && !controller.hasWindows && !controller.menu.isOpen
        guard wanted else {
            if shown { remove() }
            return
        }
        let second: String
        if model.connected {
            second = "Nothing is open on \(model.host?.name ?? "the computer"). Press Ctrl-Space for the menu."
        } else if model.host != nil {
            second = "Connecting to \(model.host?.name ?? "the computer")\u{2026}"
        } else {
            second = "Not connected to a computer. Give the mouse back to the Mac (Ctrl-Option-G) and add one from the menu bar."
        }
        let text = "Spatiand\n" + second + "\nCtrl-Option-R recentres \u{00B7} Ctrl-Option-G gives the mouse and keyboard back to the Mac"
        if shown, text == lastText { return }
        lastText = text
        if !shown {
            controller.core.setPanel(Self.panelID, widthPx: Int(size.width), heightPx: Int(size.height), widthM: 0.8, radiusM: 1.6)
            shown = true
        }
        redraw(second)
    }

    private func remove() {
        shown = false
        lastText = ""
        controller.core.remove(Self.panelID)
        controller.renderer?.panels[Self.panelID] = nil
    }

    private func redraw(_ second: String) {
        guard let renderer = controller.renderer else { return }
        let w = Int(size.width), h = Int(size.height)
        let info = CGBitmapInfo(rawValue: CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.premultipliedFirst.rawValue)
        guard let context = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                                      space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: info.rawValue) else { return }
        context.translateBy(x: 0, y: CGFloat(h))
        context.scaleBy(x: 1, y: -1)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: true)
        defer { NSGraphicsContext.restoreGraphicsState() }
        NSColor(white: 0.09, alpha: 1).setFill()
        NSBezierPath(rect: CGRect(origin: .zero, size: size)).fill()
        NSColor(white: 0.32, alpha: 1).setStroke()
        let frame = NSBezierPath(rect: CGRect(x: 2, y: 2, width: size.width - 4, height: size.height - 4))
        frame.lineWidth = 4
        frame.stroke()

        func text(_ s: String, y: CGFloat, size: CGFloat, weight: NSFont.Weight, colour: NSColor, lines: CGFloat = 1) {
            let style = NSMutableParagraphStyle()
            style.alignment = .center
            style.lineBreakMode = .byWordWrapping
            let attributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: size, weight: weight), .foregroundColor: colour, .paragraphStyle: style,
            ]
            NSAttributedString(string: s, attributes: attributes)
                .draw(in: CGRect(x: 40, y: y, width: self.size.width - 80, height: size * 1.3 * lines))
        }
        text("Spatiand", y: 28, size: 52, weight: .semibold, colour: .white)
        text(second, y: 108, size: 30, weight: .regular, colour: NSColor(white: 0.85, alpha: 1), lines: 2)
        text("Ctrl-Option-R recentres  \u{00B7}  Ctrl-Option-G gives the mouse and keyboard back to the Mac",
             y: 232, size: 22, weight: .regular, colour: NSColor(white: 0.55, alpha: 1), lines: 1)

        guard let data = context.data else { return }
        var texture = renderer.panels[Self.panelID]
        if texture == nil {
            let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .bgra8Unorm, width: w, height: h, mipmapped: false)
            texture = renderer.device.makeTexture(descriptor: d)
        }
        texture?.replace(region: MTLRegionMake2D(0, 0, w, h), mipmapLevel: 0, withBytes: data, bytesPerRow: w * 4)
        renderer.panels[Self.panelID] = texture
    }
}
