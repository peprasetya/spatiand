//  RoomTitles.swift — each window's frame, title bar and buttons, as the Deck draws them.
//
//  The room says what a window's chrome shows (its title, whether it has the keyboard, which button
//  the pointer is on, whether it is pinned, whether its application is making a sound, its icon) and
//  draws it as one picture; this is the join that notices when the picture is out of date, has the
//  room make it again and hands it to the renderer as a texture. The picture is the Deck's: see
//  `chrome.rs`.

import AppKit
import Metal

final class RoomTitles {
    /// How wide, in pixels, a window's chrome is drawn.
    private static let width = 1536

    private unowned let controller: RoomController
    private var versions: [UInt16: UInt64] = [:]
    private var textures: [UInt16: MTLTexture] = [:]
    private(set) var titles: [UInt16: String] = [:]

    init(controller: RoomController) { self.controller = controller }

    func set(_ id: UInt16, title: String) {
        titles[id] = title
        controller.core.setTitle(id, title)
    }

    func drop(_ id: UInt16) {
        titles[id] = nil
        versions[id] = nil
        textures[id] = nil
        controller.renderer?.panels[UInt32(id) | 0x10000] = nil
    }

    func dropAll() { for id in Array(titles.keys) { drop(id) } }

    /// Draw again the chrome of every window whose has changed; cheap when none has.
    func update() {
        guard let renderer = controller.renderer else { return }
        for id in titles.keys {
            let version = controller.core.chromeVersion(id)
            guard version != 0, versions[id] != version else { continue }
            guard let image = controller.core.chromeImage(id, width: Self.width) else { continue }
            versions[id] = version
            var texture = textures[id]
            if texture == nil || texture!.width != image.width || texture!.height != image.height {
                let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .bgra8Unorm, width: image.width, height: image.height, mipmapped: false)
                texture = renderer.device.makeTexture(descriptor: d)
                textures[id] = texture
            }
            image.pixels.withUnsafeBytes {
                texture?.replace(region: MTLRegionMake2D(0, 0, image.width, image.height), mipmapLevel: 0,
                                 withBytes: $0.baseAddress!, bytesPerRow: image.width * 4)
            }
            renderer.panels[UInt32(id) | 0x10000] = texture
        }
    }
}
