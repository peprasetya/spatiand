//  RoomMenu.swift — Spatiand's menu, inside the glasses.
//
//  With the glasses on, the Mac's menu bar is out of sight, so Ctrl-Space opens this instead: a
//  panel in the room where the wearer is looking, aimed at with the same pointer as everything
//  else. Two columns -- what to open on the host, and what to do about the view -- drawn as an
//  image with CoreGraphics and handed to the room as a window of Spatiand's own.

import AppKit
import Metal

final class RoomMenu {
    static let panelID: UInt16 = 0xFFF2

    private struct Row {
        var title: String
        var detail: String?
        var rect: CGRect
        var enabled = true
        /// Returns whether the menu should close once it has done it.
        var action: () -> Bool
    }

    private unowned let controller: RoomController
    private(set) var isOpen = false
    private var rows: [Row] = []
    private var hovered: Int?
    private var target: UInt16?
    private var texture: MTLTexture?
    private enum Page { case main, macWindows }
    private var page = Page.main

    // The panel's pixels and its size in the room: 1200 px across at 0.9 m is a pixel to about
    // 1.5 mm, which at the distance it floats is a little finer than the glasses can show.
    private let size = CGSize(width: 1200, height: 660)
    private let rowHeight: CGFloat = 56
    private let columnWidth: CGFloat = 552
    private let top: CGFloat = 92

    // What `RoomCore.nextCorner` and `toggleSize` have done, mirrored for the labels.
    private let corners = ["Bottom right", "Bottom left", "Top left", "Top right"]

    init(controller: RoomController) { self.controller = controller }

    var panelTexture: MTLTexture? { texture }

    // MARK: opening and closing

    func toggle() { isOpen ? close() : open() }

    func open() {
        guard !isOpen else { return }
        isOpen = true
        page = .main
        controller.refreshMacList()
        controller.hint.update()
        hovered = nil
        // What it acts on is what was being pointed at, or failing that, what has the keyboard.
        target = controller.core.aim().window.flatMap { $0 < 0xFFF0 ? $0 : nil } ?? controller.core.focused
        controller.core.setPanel(Self.panelID, widthPx: Int(size.width), heightPx: Int(size.height), widthM: 0.9, radiusM: 1.6)
        rebuild()
    }

    func close() {
        guard isOpen else { return }
        isOpen = false
        controller.core.remove(UInt16(Self.panelID))
        texture = nil
        controller.renderer?.panels[UInt32(Self.panelID)] = nil
        controller.hint.update()
    }

    // MARK: what is in it

    private func rebuild() {
        let model = Model.shared
        let core = controller.core
        var rows: [Row] = []
        func place(_ column: Int, _ index: Int) -> CGRect {
            CGRect(x: 24 + CGFloat(column) * (columnWidth + 24), y: top + CGFloat(index) * rowHeight, width: columnWidth, height: rowHeight - 6)
        }

        // Left: what the host can run, or the windows of this Mac.
        switch page {
        case .main:
            for (i, app) in model.apps.prefix(7).enumerated() {
                rows.append(Row(title: "Open " + app.name, detail: nil, rect: place(0, i)) {
                    model.launch(app)
                    return true
                })
            }
            rows.append(Row(title: "Windows of this Mac\u{2026}", detail: nil, rect: place(0, min(model.apps.count, 7))) { [weak self] in
                self?.page = .macWindows
                self?.controller.refreshMacList()
                self?.rebuild()
                return false
            })
        case .macWindows:
            if !MacWindows.allowed(ask: false) {
                rows.append(Row(title: "Allow Screen Recording\u{2026}", detail: nil, rect: place(0, 0)) {
                    _ = MacWindows.allowed(ask: true)
                    return true
                })
            }
            let listed = controller.macList.prefix(7)
            for (i, info) in listed.enumerated() {
                let row = (MacWindows.allowed(ask: false) ? 0 : 1) + i
                rows.append(Row(title: info.app, detail: String(info.title.prefix(24)), rect: place(0, row)) { [weak self] in
                    self?.controller.bringMacWindow(info)
                    return true
                })
            }
            rows.append(Row(title: "Back", detail: nil, rect: place(0, 8)) { [weak self] in
                self?.page = .main
                self?.rebuild()
                return false
            })
        }

        // Right: windows, then the view.
        var r = 0
        for window in model.openWindows.prefix(2) {
            let id = window.id
            rows.append(Row(title: "Bring here", detail: String(window.title.prefix(26)), rect: place(1, r)) { [weak self] in
                core.bringHere(id)
                core.focused = id
                self?.controller.focusChanged(id)
                return true
            })
            r += 1
        }
        let hasTarget = target != nil
        let pinned = target.map { core.isPinned($0) } ?? false
        rows.append(Row(title: "Recentre the view", detail: "Ctrl-Option-R", rect: place(1, r)) { [weak self] in
            self?.controller.recentre()
            return true
        })
        r += 1
        rows.append(Row(title: pinned ? "Let the window go" : "Pin the window to the glass", detail: nil, rect: place(1, r), enabled: hasTarget) { [weak self] in
            if let id = self?.target { core.setPinned(id, !core.isPinned(id)) }
            return true
        })
        r += 1
        rows.append(Row(title: "Resize the window to fit", detail: "drag its corner", rect: place(1, r), enabled: hasTarget) { [weak self] in
            if let id = self?.target { self?.controller.fit(id) }
            return true
        })
        r += 1
        rows.append(Row(title: "Close the window", detail: nil, rect: place(1, r), enabled: hasTarget) { [weak self] in
            if let id = self?.target { model.link.say(["Close": ["window": Int(id)]]) }
            return true
        })
        r += 1
        rows.append(Row(title: "Pinned windows", detail: corners[controller.cornerIndex], rect: place(1, r)) { [weak self] in
            self?.controller.cornerIndex = ((self?.controller.cornerIndex ?? 0) + 1) % 4
            core.nextCorner()
            self?.rebuild()
            return false
        })
        r += 1
        rows.append(Row(title: "Pinned size", detail: controller.pinnedLarge ? "Large" : "Small", rect: place(1, r)) { [weak self] in
            self?.controller.pinnedLarge.toggle()
            core.toggleSize()
            self?.rebuild()
            return false
        })
        r += 1
        let limits = [4_000, 8_000, 10_000, 16_000, 25_000, 40_000]
        rows.append(Row(title: "Video limit", detail: "\(Settings.maxKbit / 1000) Mbit/s", rect: place(1, r)) { [weak self] in
            let at = limits.firstIndex(of: Settings.maxKbit) ?? 2
            Settings.maxKbit = limits[(at + 1) % limits.count]
            model.sendBandwidth()
            self?.rebuild()
            return false
        })
        r += 1
        rows.append(Row(title: "Mouse back to the Mac", detail: "Ctrl-Option-G", rect: place(1, r)) { [weak self] in
            self?.controller.input.stop()
            return true
        })
        self.rows = rows
        redraw()
    }

    /// The list of Mac windows has changed, and is on show.
    func macListChanged() {
        if isOpen, page == .macWindows { rebuild() }
    }

    // MARK: the pointer

    /// The pointer is at this point of the panel.
    func hover(_ x: Double, _ y: Double) {
        let now = rows.firstIndex { $0.rect.contains(CGPoint(x: x, y: y)) && $0.enabled }
        if now != hovered {
            hovered = now
            redraw()
        }
    }

    /// A click at this point. Anywhere off a row does nothing: the menu closes by its own chord.
    func click(_ x: Double, _ y: Double) {
        guard let row = rows.first(where: { $0.rect.contains(CGPoint(x: x, y: y)) && $0.enabled }) else { return }
        if row.action() { close() }
    }

    /// A game controller's D-pad, A and B (see `PadInput`): up and left step back through the rows,
    /// down and right forward, A does the row and B closes the menu.
    func padPress(_ bits: UInt32) {
        guard isOpen else { return }
        let usable = rows.indices.filter { rows[$0].enabled }
        guard !usable.isEmpty else { if bits & 0x20 != 0 { close() }; return }
        let at = hovered.flatMap { usable.firstIndex(of: $0) }
        var next = at
        if bits & 0b0101 != 0 { next = at.map { max(0, $0 - 1) } ?? usable.count - 1 }
        if bits & 0b1010 != 0 { next = at.map { min(usable.count - 1, $0 + 1) } ?? 0 }
        if let next, next != at { hovered = usable[next]; redraw() }
        if bits & 0x10 != 0, let h = hovered, rows.indices.contains(h), rows[h].enabled {
            if rows[h].action() { close() }
        }
        if bits & 0x20 != 0 { close() }
    }

    // MARK: drawing

    private func redraw() {
        guard let renderer = controller.renderer else { return }
        let w = Int(size.width), h = Int(size.height)
        let bitmap = CGBitmapInfo(rawValue: CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.premultipliedFirst.rawValue)
        guard let context = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                                      space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: bitmap.rawValue) else { return }
        // AppKit draws text top-down.
        context.translateBy(x: 0, y: CGFloat(h))
        context.scaleBy(x: 1, y: -1)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: true)
        defer { NSGraphicsContext.restoreGraphicsState() }

        let background = NSColor(white: 0.09, alpha: 1)
        background.setFill()
        NSBezierPath(rect: CGRect(origin: .zero, size: size)).fill()
        NSColor(white: 0.32, alpha: 1).setStroke()
        let frame = NSBezierPath(rect: CGRect(x: 2, y: 2, width: size.width - 4, height: size.height - 4))
        frame.lineWidth = 4
        frame.stroke()

        func text(_ s: String, at p: CGPoint, size: CGFloat, weight: NSFont.Weight, colour: NSColor, width: CGFloat? = nil, right: Bool = false) {
            let style = NSMutableParagraphStyle()
            style.lineBreakMode = .byTruncatingTail
            style.alignment = right ? .right : .left
            let attributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: size, weight: weight), .foregroundColor: colour, .paragraphStyle: style,
            ]
            let box = CGRect(x: p.x, y: p.y, width: width ?? (self.size.width - p.x - 24), height: size * 1.5)
            NSAttributedString(string: s, attributes: attributes).draw(in: box)
        }

        text("Spatiand", at: CGPoint(x: 32, y: 22), size: 38, weight: .semibold, colour: .white)
        text(Model.shared.host.map { "on " + $0.name } ?? "not connected", at: CGPoint(x: 250, y: 32), size: 26, weight: .regular, colour: NSColor(white: 0.6, alpha: 1))
        text("Ctrl-Space closes this", at: CGPoint(x: 700, y: 32), size: 24, weight: .regular, colour: NSColor(white: 0.5, alpha: 1), width: 470, right: true)

        for (i, row) in rows.enumerated() {
            if hovered == i {
                NSColor(calibratedRed: 0.18, green: 0.42, blue: 0.85, alpha: 1).setFill()
                NSBezierPath(roundedRect: row.rect, xRadius: 10, yRadius: 10).fill()
            } else {
                NSColor(white: 0.15, alpha: 1).setFill()
                NSBezierPath(roundedRect: row.rect, xRadius: 10, yRadius: 10).fill()
            }
            let colour: NSColor = row.enabled ? .white : NSColor(white: 0.45, alpha: 1)
            text(row.title, at: CGPoint(x: row.rect.minX + 18, y: row.rect.minY + 11), size: 27, weight: .medium, colour: colour, width: row.rect.width - 36)
            if let detail = row.detail {
                text(detail, at: CGPoint(x: row.rect.minX + 18, y: row.rect.minY + 11), size: 25, weight: .regular,
                     colour: NSColor(white: hovered == i ? 0.9 : 0.62, alpha: 1), width: row.rect.width - 36, right: true)
            }
        }
        if rows.isEmpty {
            text("Nothing to open: not connected to a computer.", at: CGPoint(x: 32, y: 120), size: 28, weight: .regular, colour: NSColor(white: 0.6, alpha: 1))
        }

        guard let data = context.data else { return }
        if texture == nil {
            let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .bgra8Unorm, width: w, height: h, mipmapped: false)
            texture = renderer.device.makeTexture(descriptor: d)
        }
        texture?.replace(region: MTLRegionMake2D(0, 0, w, h), mipmapLevel: 0, withBytes: data, bytesPerRow: w * 4)
        renderer.panels[UInt32(Self.panelID)] = texture
    }
}
