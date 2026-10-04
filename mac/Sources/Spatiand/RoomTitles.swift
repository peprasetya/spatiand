//  RoomTitles.swift — the strip above each window in the room: its name, and the buttons.
//
//  The room's windows are not framed by anyone, so each carries a bar of its own: what it is
//  called, a button to pin it to the glass, and one to close it. The bar is also what a window is
//  taken by -- aim at it, press, and the window follows the pointer round the room until let go.
//
//  The room says where the bar is and what part of it is aimed at (in `BAR_PX`, 1024 by 46); this
//  draws its picture, once for each state it can be in, and says what a press on each part means.

import AppKit
import Metal

final class RoomTitles {
    enum Zone { case close, pin, fit, drag }

    /// The bar's picture size, as the room's maths has it.
    static let size = CGSize(width: 1024, height: 46)
    /// The bit of the right-hand end each button takes.
    static let button: CGFloat = 46

    private struct State: Equatable {
        var title: String
        var focused: Bool
        var hover: Zone?
    }

    private unowned let controller: RoomController
    private var textures: [UInt16: MTLTexture] = [:]
    private var drawn: [UInt16: State] = [:]
    private(set) var titles: [UInt16: String] = [:]
    /// What the pointer is over, if it is a bar.
    var hover: (id: UInt16, zone: Zone)?

    init(controller: RoomController) { self.controller = controller }

    static func zone(atX x: Double) -> Zone {
        if x >= Double(size.width - button) { return .close }
        if x >= Double(size.width - 2 * button) { return .pin }
        if x >= Double(size.width - 3 * button) { return .fit }
        return .drag
    }

    func set(_ id: UInt16, title: String) { titles[id] = title }

    func drop(_ id: UInt16) {
        titles[id] = nil
        drawn[id] = nil
        textures[id] = nil
        controller.renderer?.panels[UInt32(id) | 0x10000] = nil
        if hover?.id == id { hover = nil }
    }

    func dropAll() { for id in Array(titles.keys) { drop(id) } }

    /// Redraw the bars whose state has changed; cheap when none has.
    func update() {
        guard let renderer = controller.renderer else { return }
        for (id, title) in titles {
            let state = State(title: title, focused: controller.core.focused == id, hover: hover?.id == id ? hover?.zone : nil)
            if drawn[id] == state, textures[id] != nil { continue }
            drawn[id] = state
            guard let texture = texture(for: id, renderer) else { continue }
            draw(state, into: texture)
            renderer.panels[UInt32(id) | 0x10000] = texture
        }
    }

    private func texture(for id: UInt16, _ renderer: RoomRenderer) -> MTLTexture? {
        if let t = textures[id] { return t }
        let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .bgra8Unorm, width: Int(Self.size.width), height: Int(Self.size.height), mipmapped: false)
        let t = renderer.device.makeTexture(descriptor: d)
        textures[id] = t
        return t
    }

    private func draw(_ state: State, into texture: MTLTexture) {
        let w = Int(Self.size.width), h = Int(Self.size.height)
        let info = CGBitmapInfo(rawValue: CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.premultipliedFirst.rawValue)
        guard let context = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                                      space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: info.rawValue) else { return }
        context.translateBy(x: 0, y: CGFloat(h))
        context.scaleBy(x: 1, y: -1)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: true)
        defer { NSGraphicsContext.restoreGraphicsState() }

        NSColor(white: state.focused ? 0.24 : 0.14, alpha: 1).setFill()
        NSBezierPath(rect: CGRect(origin: .zero, size: Self.size)).fill()
        // A line where it meets the window, in the colour the focused one is known by.
        (state.focused ? NSColor(calibratedRed: 0.25, green: 0.52, blue: 0.96, alpha: 1) : NSColor(white: 0.3, alpha: 1)).setFill()
        NSBezierPath(rect: CGRect(x: 0, y: Self.size.height - 3, width: Self.size.width, height: 3)).fill()

        let style = NSMutableParagraphStyle()
        style.lineBreakMode = .byTruncatingTail
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 21, weight: .medium),
            .foregroundColor: NSColor(white: state.focused ? 1.0 : 0.72, alpha: 1),
            .paragraphStyle: style,
        ]
        NSAttributedString(string: state.title, attributes: attributes)
            .draw(in: CGRect(x: 16, y: 9, width: Self.size.width - 3 * Self.button - 32, height: 30))

        func box(_ index: Int, _ zone: Zone, _ glyph: String, hot: NSColor) {
            let rect = CGRect(x: Self.size.width - CGFloat(index + 1) * Self.button, y: 0, width: Self.button, height: Self.size.height - 3)
            if state.hover == zone {
                hot.setFill()
                NSBezierPath(rect: rect).fill()
            }
            let g: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 22, weight: .semibold), .foregroundColor: NSColor.white,
                .paragraphStyle: { let p = NSMutableParagraphStyle(); p.alignment = .center; return p }(),
            ]
            NSAttributedString(string: glyph, attributes: g).draw(in: CGRect(x: rect.minX, y: rect.minY + 8, width: rect.width, height: 30))
        }
        box(0, .close, "\u{2715}", hot: NSColor(calibratedRed: 0.85, green: 0.2, blue: 0.2, alpha: 1))
        box(2, .fit, "\u{2922}", hot: NSColor(calibratedRed: 0.18, green: 0.42, blue: 0.85, alpha: 1))
        box(1, .pin, "\u{25C9}", hot: NSColor(calibratedRed: 0.18, green: 0.42, blue: 0.85, alpha: 1))

        guard let data = context.data else { return }
        texture.replace(region: MTLRegionMake2D(0, 0, w, h), mipmapLevel: 0, withBytes: data, bytesPerRow: w * 4)
    }
}
