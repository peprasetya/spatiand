//  RoomRenderer.swift — the room, drawn for two eyes.
//
//  Each window is a handful of strips the Rust side has already bent round the wearer; this draws
//  them, textured with the newest decoded picture, once for each eye into the two halves of the
//  glasses' side-by-side display (or once, full width, when the glasses are in their flat mode).
//  The cursor is drawn last. The room is black, which on these glasses is no light at all: the
//  windows float in whatever is really there.

import AppKit
import CoreVideo
import Metal
import QuartzCore

final class RoomRenderer {
    let device: MTLDevice
    private let queue: MTLCommandQueue
    private let windowPipeline: MTLRenderPipelineState
    private let cursorPipeline: MTLRenderPipelineState
    private var cache: CVMetalTextureCache?
    private var cursorTexture: MTLTexture
    private let arrowTexture: MTLTexture
    private let core: RoomCore

    /// Decoded pictures by window; owned by the main thread.
    var decoders: [UInt16: PictureSource] = [:]
    /// Spatiand's own panels, which draw themselves: the menu.
    var panels: [UInt32: MTLTexture] = [:]
    private var cached: [UInt16: (generation: Int, texture: CVMetalTexture)] = [:]

    private static let capacity = 600_000
    private var buffers: [MTLBuffer] = []
    private var next = 0
    private let inFlight = DispatchSemaphore(value: 3)

    private(set) var framesDrawn = 0

    init?(core: RoomCore, device: MTLDevice? = nil) {
        guard let device = device ?? MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else { return nil }
        self.device = device
        self.queue = queue
        self.core = core
        do {
            let library = try device.makeLibrary(source: Self.shaders, options: nil)
            func pipeline(_ vertex: String, _ fragment: String, blend: Bool) throws -> MTLRenderPipelineState {
                let d = MTLRenderPipelineDescriptor()
                d.vertexFunction = library.makeFunction(name: vertex)
                d.fragmentFunction = library.makeFunction(name: fragment)
                d.colorAttachments[0].pixelFormat = .bgra8Unorm
                if blend {
                    let a = d.colorAttachments[0]!
                    a.isBlendingEnabled = true
                    // The cursor's picture is premultiplied.
                    a.sourceRGBBlendFactor = .one
                    a.destinationRGBBlendFactor = .oneMinusSourceAlpha
                    a.sourceAlphaBlendFactor = .one
                    a.destinationAlphaBlendFactor = .oneMinusSourceAlpha
                }
                return try device.makeRenderPipelineState(descriptor: d)
            }
            windowPipeline = try pipeline("vertex_room", "fragment_window", blend: false)
            cursorPipeline = try pipeline("vertex_room", "fragment_cursor", blend: true)
        } catch {
            print("room: could not build the shaders: \(error)")
            return nil
        }
        CVMetalTextureCacheCreate(nil, nil, device, nil, &cache)
        guard let cursor = Self.makeCursor(device) else { return nil }
        cursorTexture = cursor
        arrowTexture = cursor
        for _ in 0..<3 {
            guard let b = device.makeBuffer(length: Self.capacity * 4, options: .storageModeShared) else { return nil }
            buffers.append(b)
        }
    }

    /// The host's own pointer picture (premultiplied BGRA), or the plain arrow when there is none.
    func setCursorPicture(_ pixels: Data?, width: Int, height: Int) {
        guard let pixels, width > 0, height > 0, pixels.count >= width * height * 4 else {
            cursorTexture = arrowTexture
            return
        }
        let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .bgra8Unorm, width: width, height: height, mipmapped: false)
        guard let texture = device.makeTexture(descriptor: d) else { return }
        pixels.withUnsafeBytes {
            texture.replace(region: MTLRegionMake2D(0, 0, width, height), mipmapLevel: 0, withBytes: $0.baseAddress!, bytesPerRow: width * 4)
        }
        cursorTexture = texture
    }

    // MARK: the decoders

    func decoder(for id: UInt16, onTrouble: @escaping () -> Void) -> RoomDecoder {
        if let existing = decoders[id] as? RoomDecoder { return existing }
        let d = RoomDecoder()
        d.onTrouble = onTrouble
        decoders[id] = d
        return d
    }

    func drop(_ id: UInt16) {
        decoders[id]?.invalidate()
        decoders[id] = nil
        cached[id] = nil
    }

    func dropAll() {
        for id in Array(decoders.keys) { drop(id) }
    }

    private func texture(for key: UInt32, keep: inout [CVMetalTexture]) -> MTLTexture? {
        if let panel = panels[key] { return panel }
        // A title bar that has no picture yet is not drawn; a window's id is 16 bits.
        guard key < 0x10000 else { return nil }
        let id = UInt16(key)
        guard let cache, let (buffer, generation) = decoders[id]?.current() else { return nil }
        if let hit = cached[id], hit.generation == generation {
            keep.append(hit.texture)
            return CVMetalTextureGetTexture(hit.texture)
        }
        var made: CVMetalTexture?
        let status = CVMetalTextureCacheCreateTextureFromImage(
            nil, cache, buffer, nil, .bgra8Unorm,
            CVPixelBufferGetWidth(buffer), CVPixelBufferGetHeight(buffer), 0, &made)
        guard status == kCVReturnSuccess, let made else { return nil }
        cached[id] = (generation, made)
        keep.append(made)
        return CVMetalTextureGetTexture(made)
    }

    // MARK: drawing

    /// Draw the room into `target`. `sideBySide` is two eyes across it, else one eye over it all.
    /// Returns once the commands are queued; `done` is called when the GPU has finished.
    func render(into target: MTLTexture, sideBySide: Bool, present: CAMetalDrawable? = nil, done: (() -> Void)? = nil) {
        inFlight.wait()
        let buffer = buffers[next]
        next = (next + 1) % buffers.count
        let floats = buffer.contents().bindMemory(to: Float.self, capacity: Self.capacity)
        guard let frame = core.frame(vertices: floats, capacity: Self.capacity),
              let commands = queue.makeCommandBuffer() else {
            inFlight.signal()
            return
        }
        var keep: [CVMetalTexture] = []
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = target
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 1)
        pass.colorAttachments[0].storeAction = .store
        guard let encoder = commands.makeRenderCommandEncoder(descriptor: pass) else {
            inFlight.signal()
            return
        }
        let width = target.width, height = target.height
        let eyes = sideBySide ? 2 : 1
        let eyeWidth = width / eyes

        // Textures are looked up once per window, not once per eye.
        var textures: [UInt32: MTLTexture] = [:]
        for draw in frame.draws where draw.window != RoomCore.cursor {
            if let t = texture(for: draw.window, keep: &keep) { textures[draw.window] = t }
        }

        for eye in 0..<eyes {
            let rect = MTLViewport(originX: Double(eye * eyeWidth), originY: 0, width: Double(eyeWidth), height: Double(height), znear: 0, zfar: 1)
            encoder.setViewport(rect)
            encoder.setScissorRect(MTLScissorRect(x: eye * eyeWidth, y: 0, width: eyeWidth, height: height))
            var matrix = Array(frame.matrices[(eye * 16)..<(eye * 16 + 16)])
            encoder.setVertexBuffer(buffer, offset: 0, index: 0)
            encoder.setVertexBytes(&matrix, length: 64, index: 1)

            encoder.setRenderPipelineState(windowPipeline)
            for draw in frame.draws where draw.window != RoomCore.cursor {
                guard let texture = textures[draw.window] else { continue }
                // The one being looked at is as the host drew it; the rest are a little dimmer.
                var brightness: Float = (draw.focused || draw.aimed || draw.window >= 0xFFF0 || draw.window & 0x10000 != 0) ? 1.0 : 0.82
                encoder.setFragmentBytes(&brightness, length: 4, index: 0)
                encoder.setFragmentTexture(texture, index: 0)
                encoder.drawPrimitives(type: .triangle, vertexStart: draw.first, vertexCount: draw.count)
            }

            encoder.setRenderPipelineState(cursorPipeline)
            for draw in frame.draws where draw.window == RoomCore.cursor {
                encoder.setFragmentTexture(cursorTexture, index: 0)
                encoder.drawPrimitives(type: .triangle, vertexStart: draw.first, vertexCount: draw.count)
            }
        }
        encoder.endEncoding()
        if let present { commands.present(present) }
        commands.addCompletedHandler { [inFlight] _ in
            _ = keep.count
            inFlight.signal()
            done?()
        }
        commands.commit()
        framesDrawn += 1
    }

    /// One frame as an image, for looking at without glasses.
    func snapshot(width: Int, height: Int, sideBySide: Bool) -> CGImage? {
        let description = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .bgra8Unorm, width: width, height: height, mipmapped: false)
        description.usage = [.renderTarget, .shaderRead]
        description.storageMode = .managed
        guard let target = device.makeTexture(descriptor: description) else { return nil }
        let finished = DispatchSemaphore(value: 0)
        render(into: target, sideBySide: sideBySide) { finished.signal() }
        guard finished.wait(timeout: .now() + 3) == .success else { return nil }
        // A managed texture is read back through a copy the GPU has to be asked for.
        guard let commands = queue.makeCommandBuffer(), let blit = commands.makeBlitCommandEncoder() else { return nil }
        blit.synchronize(resource: target)
        blit.endEncoding()
        commands.commit()
        commands.waitUntilCompleted()
        var bytes = [UInt8](repeating: 0, count: width * height * 4)
        target.getBytes(&bytes, bytesPerRow: width * 4, from: MTLRegionMake2D(0, 0, width, height), mipmapLevel: 0)
        let colour = CGColorSpaceCreateDeviceRGB()
        let info = CGBitmapInfo(rawValue: CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.noneSkipFirst.rawValue)
        guard let provider = CGDataProvider(data: Data(bytes) as CFData) else { return nil }
        return CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: width * 4,
                       space: colour, bitmapInfo: info, provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
    }

    // MARK: the cursor

    /// An arrow, drawn once: white with a dark edge, so it reads over a page of either colour.
    private static func makeCursor(_ device: MTLDevice) -> MTLTexture? {
        let size = 64
        var pixels = [UInt8](repeating: 0, count: size * size * 4)
        let ok = pixels.withUnsafeMutableBytes { raw -> Bool in
            guard let context = CGContext(
                data: raw.baseAddress, width: size, height: size, bitsPerComponent: 8, bytesPerRow: size * 4,
                space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
            // The tip at the top left, in a context whose y runs up.
            let p = CGMutablePath()
            p.move(to: CGPoint(x: 6, y: 58))
            p.addLine(to: CGPoint(x: 6, y: 10))
            p.addLine(to: CGPoint(x: 19, y: 22))
            p.addLine(to: CGPoint(x: 27, y: 5))
            p.addLine(to: CGPoint(x: 36, y: 9))
            p.addLine(to: CGPoint(x: 28, y: 26))
            p.addLine(to: CGPoint(x: 46, y: 26))
            p.closeSubpath()
            context.addPath(p)
            context.setFillColor(CGColor(gray: 1, alpha: 1))
            context.fillPath()
            context.addPath(p)
            context.setStrokeColor(CGColor(gray: 0.05, alpha: 1))
            context.setLineWidth(4)
            context.setLineJoin(.round)
            context.strokePath()
            // The stroke covers the inside edge of the fill; draw the fill again, smaller, on top.
            context.addPath(p)
            context.setFillColor(CGColor(gray: 1, alpha: 1))
            context.fillPath()
            return true
        }
        guard ok else { return nil }
        let description = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba8Unorm, width: size, height: size, mipmapped: false)
        guard let texture = device.makeTexture(descriptor: description) else { return nil }
        texture.replace(region: MTLRegionMake2D(0, 0, size, size), mipmapLevel: 0, withBytes: pixels, bytesPerRow: size * 4)
        return texture
    }

    // MARK: shaders

    private static let shaders = """
    #include <metal_stdlib>
    using namespace metal;

    struct Vertex { packed_float3 position; packed_float2 uv; };
    struct Varying { float4 position [[position]]; float2 uv; };

    vertex Varying vertex_room(uint id [[vertex_id]],
                               const device Vertex *vertices [[buffer(0)]],
                               constant float4x4 &projection [[buffer(1)]]) {
        Varying out;
        float4 p = projection * float4(float3(vertices[id].position), 1.0);
        // The matrices are OpenGL's, whose depth runs -1 to 1; Metal's runs 0 to 1.
        p.z = (p.z + p.w) * 0.5;
        out.position = p;
        out.uv = float2(vertices[id].uv);
        return out;
    }

    constexpr sampler linear_clamped(filter::linear, address::clamp_to_edge);

    fragment float4 fragment_window(Varying in [[stage_in]],
                                    texture2d<float> picture [[texture(0)]],
                                    constant float &brightness [[buffer(0)]]) {
        float4 c = picture.sample(linear_clamped, in.uv);
        return float4(c.rgb * brightness, 1.0);
    }

    fragment float4 fragment_cursor(Varying in [[stage_in]],
                                    texture2d<float> arrow [[texture(0)]]) {
        return arrow.sample(linear_clamped, in.uv);
    }
    """
}
