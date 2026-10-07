//  RoomRenderer.swift — the room, drawn for two eyes.
//
//  Each window is a handful of strips the Rust side has already bent round the wearer; this draws
//  them, textured with the newest decoded picture, once for each eye into the two halves of the
//  glasses' side-by-side display (or once, full width, when the glasses are in their flat mode).
//  The cursor is drawn last. The room is black, which on these glasses is no light at all: the
//  windows float in whatever is really there.

import AppKit
import CSpatiand
import CoreVideo
import Metal
import QuartzCore

final class RoomRenderer {
    let device: MTLDevice
    private let queue: MTLCommandQueue
    private let windowPipeline: MTLRenderPipelineState
    private let cursorPipeline: MTLRenderPipelineState
    private let skyPipeline: MTLRenderPipelineState
    private let bubblePipeline: MTLRenderPipelineState
    /// The Deck's studio: a dark sky, a key light, a horizon and a floor grid. What tells the eyes
    /// how far away the windows are; without it they hang in a void and read as flat.
    private var currentSky: (texture: MTLTexture, info: sp_sky_info)?
    private let skyLock = NSLock()
    private var skyTexture: MTLTexture? { skyLock.lock(); defer { skyLock.unlock() }; return currentSky?.texture }
    private var cache: CVMetalTextureCache?
    private var cursorTexture: MTLTexture
    private let arrowTexture: MTLTexture
    /// The double arrow over a window's frame.
    private let resizeTexture: MTLTexture
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
    private let statsLock = NSLock()
    private var gpuTotal = 0.0, gpuWorst = 0.0, gpuFrames = 0, dropped = 0

    /// How long the GPU took over the frames since last asked, and how many were not drawn for want of it.
    func takeGPUStats() -> (average: Double, worst: Double, frames: Int, dropped: Int) {
        statsLock.lock()
        defer { statsLock.unlock() }
        let out = (gpuFrames > 0 ? gpuTotal / Double(gpuFrames) : 0, gpuWorst, gpuFrames, dropped)
        gpuTotal = 0; gpuWorst = 0; gpuFrames = 0; dropped = 0
        return out
    }

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
            skyPipeline = try pipeline("vertex_sky", "fragment_sky", blend: false)
            bubblePipeline = try pipeline("vertex_room", "fragment_bubble", blend: true)
        } catch {
            print("room: could not build the shaders: \(error)")
            return nil
        }
        CVMetalTextureCacheCreate(nil, nil, device, nil, &cache)
        guard let cursor = Self.makeCursor(device), let resize = Self.makeCursor(device, resize: true) else { return nil }
        cursorTexture = cursor
        resizeTexture = resize
        arrowTexture = cursor
        for _ in 0..<3 {
            guard let b = device.makeBuffer(length: Self.capacity * 4, options: .storageModeShared) else { return nil }
            buffers.append(b)
        }
    }

    /// What the environment picture is and how it is laid out: how it is read for each eye, whether it covers only
    /// the front, and how far it is turned. Set with the picture.
    private var skyInfo: sp_sky_info { skyLock.lock(); defer { skyLock.unlock() }; return currentSky?.info ?? sp_sky_info(width: 0, height: 0, projection: 0, stereo: 0, yaw_millideg: 0) }

    /// A new environment: its pixels are made a texture, with its mipmaps, off the main thread, and the room changes
    /// to it when it is ready -- the old one stays up and keeps tracking until then.
    func setSky(_ info: sp_sky_info, pixels: [UInt8]) {
        let (w, h) = (Int(info.width), Int(info.height))
        guard w > 0, h > 0, pixels.count >= w * h * 4 else { return }
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba8Unorm, width: w, height: h, mipmapped: true)
            guard let self, let texture = self.device.makeTexture(descriptor: d) else { return }
            texture.replace(region: MTLRegionMake2D(0, 0, w, h), mipmapLevel: 0, withBytes: pixels, bytesPerRow: w * 4)
            if let queue = self.device.makeCommandQueue(), let commands = queue.makeCommandBuffer(), let blit = commands.makeBlitCommandEncoder() {
                blit.generateMipmaps(for: texture)
                blit.endEncoding()
                commands.commit()
                commands.waitUntilCompleted()
            }
            self.skyLock.lock()
            self.currentSky = (texture, info)
            self.skyLock.unlock()
        }
    }

    /// How an eye reads the environment picture: the part of it that is that eye's (a stereo pair is packed into one),
    /// whether only the front is there, and the turn.
    private func skyParams(eye: Int) -> [Float] {
        var rect: [Float] = [0, 0, 1, 1]
        switch skyInfo.stereo {
        case 1: rect = eye == 0 ? [0, 0, 1, 0.5] : [0, 0.5, 1, 1]
        case 2: rect = eye == 0 ? [0, 0, 0.5, 1] : [0.5, 0, 1, 1]
        default: break
        }
        return rect + [Float(skyInfo.yaw_millideg) / 1000 * .pi / 180, skyInfo.projection == 1 ? 1 : 0, 0, 0]
    }

    /// The host's own pointer picture (premultiplied BGRA), or the plain arrow when there is none.
    func setCursorPicture(_ pixels: Data?, width: Int, height: Int) {
        // The Deck draws the same reticle over every window and not the application's own pointer, so
        // this does too.
        guard false, let pixels, width > 0, height > 0, pixels.count >= width * height * 4 else {
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

    /// A Mac window's picture is captured at the display's density, which is more than the glasses show of
    /// it: sampled straight, text shimmers as the head moves. So the newest picture is copied into a texture
    /// with its smaller sizes, and sampled through those.
    private var mips: [UInt16: (texture: MTLTexture, generation: Int)] = [:]

    private func mipped(_ id: UInt16, _ source: MTLTexture, generation: Int, commands: MTLCommandBuffer) -> MTLTexture {
        var entry = mips[id]
        if entry == nil || entry!.texture.width != source.width || entry!.texture.height != source.height {
            let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .bgra8Unorm, width: source.width, height: source.height, mipmapped: true)
            d.usage = .shaderRead
            d.storageMode = .private
            guard let texture = device.makeTexture(descriptor: d) else { return source }
            entry = (texture, -1)
        }
        guard var entry else { return source }
        if entry.generation != generation, let blit = commands.makeBlitCommandEncoder() {
            blit.copy(from: source, sourceSlice: 0, sourceLevel: 0, sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0),
                      sourceSize: MTLSize(width: source.width, height: source.height, depth: 1),
                      to: entry.texture, destinationSlice: 0, destinationLevel: 0, destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0))
            blit.generateMipmaps(for: entry.texture)
            blit.endEncoding()
            entry.generation = generation
        }
        mips[id] = entry
        return entry.texture
    }

    private func texture(for key: UInt32, keep: inout [CVMetalTexture], commands: MTLCommandBuffer) -> MTLTexture? {
        if let panel = panels[key] { return panel }
        // A title bar that has no picture yet is not drawn; a window's id is 16 bits.
        guard key < 0x10000 else { return nil }
        let id = UInt16(key)
        guard let cache, let (buffer, generation) = decoders[id]?.current() else { return nil }
        let isMac = MacWindows.isMac(id)
        if let hit = cached[id], hit.generation == generation {
            keep.append(hit.texture)
            guard let texture = CVMetalTextureGetTexture(hit.texture) else { return nil }
            return isMac ? mipped(id, texture, generation: generation, commands: commands) : texture
        }
        var made: CVMetalTexture?
        let status = CVMetalTextureCacheCreateTextureFromImage(
            nil, cache, buffer, nil, .bgra8Unorm,
            CVPixelBufferGetWidth(buffer), CVPixelBufferGetHeight(buffer), 0, &made)
        guard status == kCVReturnSuccess, let made, let texture = CVMetalTextureGetTexture(made) else { return nil }
        cached[id] = (generation, made)
        keep.append(made)
        return isMac ? mipped(id, texture, generation: generation, commands: commands) : texture
    }

    private func skyMatrices() -> [Float] { core.skyMatrices() }

    // MARK: drawing

    /// Draw the room into `target`. `sideBySide` is two eyes across it, else one eye over it all.
    /// Returns once the commands are queued; `done` is called when the GPU has finished.
    func render(into target: MTLTexture, sideBySide: Bool, present: CAMetalDrawable? = nil, done: (() -> Void)? = nil) {
        // A frame the GPU has not got to is dropped rather than waited for: the main thread is also the pointer, the
        // keys and the fingers, and a wait of thirty milliseconds there is felt as much as a skipped frame.
        if inFlight.wait(timeout: .now() + .milliseconds(3)) == .timedOut {
            statsLock.lock(); dropped += 1; statsLock.unlock()
            return
        }
        let buffer = buffers[next]
        next = (next + 1) % buffers.count
        let floats = buffer.contents().bindMemory(to: Float.self, capacity: Self.capacity)
        guard let frame = core.frame(vertices: floats, capacity: Self.capacity),
              let commands = queue.makeCommandBuffer() else {
            inFlight.signal()
            return
        }
        var keep: [CVMetalTexture] = []
        // Textures are looked up once per window, not once per eye.
        var textures: [UInt32: MTLTexture] = [:]
        for draw in frame.draws where draw.window != RoomCore.cursor {
            if let t = texture(for: draw.window, keep: &keep, commands: commands) { textures[draw.window] = t }
        }
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

        let sky = skyTexture != nil ? skyMatrices() : nil
        for eye in 0..<eyes {
            let rect = MTLViewport(originX: Double(eye * eyeWidth), originY: 0, width: Double(eyeWidth), height: Double(height), znear: 0, zfar: 1)
            encoder.setViewport(rect)
            encoder.setScissorRect(MTLScissorRect(x: eye * eyeWidth, y: 0, width: eyeWidth, height: height))
            if let sky, let texture = skyTexture {
                var inverse = Array(sky[(eye * 16)..<(eye * 16 + 16)])
                encoder.setRenderPipelineState(skyPipeline)
                encoder.setVertexBytes(&inverse, length: 64, index: 0)
                encoder.setFragmentBytes(&inverse, length: 64, index: 0)
                var params = skyParams(eye: eye)
                encoder.setFragmentBytes(&params, length: 32, index: 1)
                encoder.setFragmentTexture(texture, index: 0)
                encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
            }
            var matrix = Array(frame.matrices[(eye * 16)..<(eye * 16 + 16)])
            encoder.setVertexBuffer(buffer, offset: 0, index: 0)
            encoder.setVertexBytes(&matrix, length: 64, index: 1)

            for draw in frame.draws where draw.window != RoomCore.cursor {
                guard let texture = textures[draw.window] else { continue }
                if (0xFF10...0xFF2F).contains(draw.window) {
                    // A launcher bubble: glass that bends the room behind it, as the Deck's does.
                    var bubble = bubbleParams(draw, floats, focused: draw.window >= 0xFF20)
                    encoder.setRenderPipelineState(bubblePipeline)
                    encoder.setFragmentBytes(&bubble, length: 16, index: 0)
                    var params = skyParams(eye: 0)
                    encoder.setFragmentBytes(&params, length: 32, index: 1)
                    encoder.setFragmentTexture(texture, index: 0)
                    encoder.setFragmentTexture(skyTexture ?? arrowTexture, index: 1)
                    encoder.drawPrimitives(type: .triangle, vertexStart: draw.first, vertexCount: draw.count)
                    continue
                }
                if draw.window & 0x10000 != 0 || (draw.window >= UInt32(RoomCore.panelFirst) && draw.window != RoomCore.cursor) {
                    // A window's frame, or one of the menus: glass and rounded cards, with edges that are not there.
                    encoder.setRenderPipelineState(cursorPipeline)
                } else {
                    encoder.setRenderPipelineState(windowPipeline)
                    var brightness: Float = 1.0
                    encoder.setFragmentBytes(&brightness, length: 4, index: 0)
                }
                encoder.setFragmentTexture(texture, index: 0)
                encoder.drawPrimitives(type: .triangle, vertexStart: draw.first, vertexCount: draw.count)
            }

            encoder.setRenderPipelineState(cursorPipeline)
            for draw in frame.draws where draw.window == RoomCore.cursor {
                encoder.setFragmentTexture(draw.aimed ? resizeTexture : cursorTexture, index: 0)
                encoder.drawPrimitives(type: .triangle, vertexStart: draw.first, vertexCount: draw.count)
            }
        }
        encoder.endEncoding()
        if let present { commands.present(present) }
        commands.addCompletedHandler { [inFlight, weak self] finished in
            _ = keep.count
            if let self, finished.gpuEndTime > finished.gpuStartTime {
                let spent = finished.gpuEndTime - finished.gpuStartTime
                self.statsLock.lock(); self.gpuTotal += spent; self.gpuWorst = max(self.gpuWorst, spent); self.gpuFrames += 1; self.statsLock.unlock()
            }
            inFlight.signal()
            done?()
        }
        commands.commit()
        framesDrawn += 1
    }

    /// Where a bubble is and whether it is the one pointed at: its middle, from its own corners, and a flag.
    private func bubbleParams(_ draw: RoomCore.Draw, _ floats: UnsafeMutablePointer<Float>, focused: Bool) -> [Float] {
        var sum: (Float, Float, Float) = (0, 0, 0)
        let count = max(1, Int(draw.count))
        for i in 0..<count {
            let v = (Int(draw.first) + i) * 5
            sum.0 += floats[v]; sum.1 += floats[v + 1]; sum.2 += floats[v + 2]
        }
        let n = Float(count)
        return [sum.0 / n, sum.1 / n, sum.2 / n, focused ? 1 : 0]
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

    /// The Deck's pointer: a dot in a ring. Pale green, which is the colour the Deck gives a mouse's, so
    /// it is told from a thumb's.
    private static func makeCursor(_ device: MTLDevice, resize: Bool = false) -> MTLTexture? {
        let size = 64
        var pixels = [UInt8](repeating: 0, count: size * size * 4)
        if resize { sp_resize_cursor(UInt32(size), &pixels) } else { sp_reticle(UInt32(size), &pixels) }
        let tint: [Float] = [0.62, 1.0, 0.72]
        for i in stride(from: 0, to: pixels.count, by: 4) {
            let a = Float(pixels[i + 3]) / 255
            for c in 0..<3 { pixels[i + c] = UInt8(min(255, Float(pixels[i + c]) * tint[c] * a)) }
        }
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

    // The environment: every pixel's direction, looked up in an equirectangular picture. The same
    // arithmetic as the Deck's, with +X forward, +Y left and +Z up.
    struct SkyVarying { float4 position [[position]]; float2 ndc; };

    vertex SkyVarying vertex_sky(uint id [[vertex_id]]) {
        float2 corners[3] = { float2(-1, -1), float2(3, -1), float2(-1, 3) };
        SkyVarying out;
        out.position = float4(corners[id], 1.0, 1.0);
        out.ndc = corners[id];
        return out;
    }

    // How an eye reads the environment: its part of the picture, whether only the front of the room is there, and the
    // turn of it. The Deck's SkySource, as the shader gets it.
    struct SkyParams { float4 rect; float yaw; float front_only; float2 pad; };

    // The colour of a direction, or black where the picture has nothing (behind the viewer, for a 180 degree one).
    float3 sky_colour(texture2d<float> sky, constant SkyParams &p, float3 dir, bool use_mips) {
        constexpr sampler around(filter::linear, mip_filter::linear, s_address::repeat, t_address::clamp_to_edge);
        float azimuth = atan2(-dir.y, dir.x) - p.yaw;
        azimuth = azimuth - 2.0 * M_PI_F * floor((azimuth + M_PI_F) / (2.0 * M_PI_F));
        float u;
        if (p.front_only > 0.5) {
            if (fabs(azimuth) > M_PI_F * 0.5) { return float3(0.0); }
            u = 0.5 + azimuth / M_PI_F;
        } else {
            u = 0.5 + azimuth / (2.0 * M_PI_F);
        }
        float v = 0.5 - asin(clamp(dir.z, -1.0, 1.0)) / M_PI_F;
        float2 uv = float2(p.rect.x + u * (p.rect.z - p.rect.x), p.rect.y + v * (p.rect.w - p.rect.y));
        return use_mips ? sky.sample(around, uv).rgb : sky.sample(around, uv, level(0)).rgb;
    }

    fragment float4 fragment_sky(SkyVarying in [[stage_in]],
                                 texture2d<float> sky [[texture(0)]],
                                 constant float4x4 &inverse [[buffer(0)]],
                                 constant SkyParams &params [[buffer(1)]]) {
        float4 far = inverse * float4(in.ndc, 1.0, 1.0);
        float3 dir = normalize(far.xyz / far.w);
        return float4(sky_colour(sky, params, dir, true), 1.0);
    }

    constexpr sampler mipped(filter::linear, mip_filter::linear, address::clamp_to_edge, max_anisotropy(8));

    fragment float4 fragment_window(Varying in [[stage_in]],
                                    texture2d<float> picture [[texture(0)]],
                                    constant float &brightness [[buffer(0)]]) {
        float4 c = picture.sample(mipped, in.uv);
        return float4(c.rgb * brightness, 1.0);
    }

    // The Deck's glass bubble: an implicit hemisphere that refracts the sky behind it, mirrors it at the
    // rim by Fresnel, takes a highlight, and holds the application's icon inside.
    struct Bubble { packed_float3 centre; float focus; };

    float3 environment(texture2d<float> sky, constant SkyParams &params, float3 dir) {
        return sky_colour(sky, params, dir, false);
    }

    fragment float4 fragment_bubble(Varying in [[stage_in]],
                                    texture2d<float> icon [[texture(0)]],
                                    texture2d<float> sky [[texture(1)]],
                                    constant Bubble &bubble [[buffer(0)]],
                                    constant SkyParams &params [[buffer(1)]]) {
        float2 p = float2(in.uv.x, 1.0 - in.uv.y) * 2.0 - 1.0;
        float r2 = dot(p, p);
        if (r2 > 1.0) { discard_fragment(); }
        float r = sqrt(r2);
        float z = sqrt(max(1.0 - r2, 0.0));
        float3 d = normalize(float3(bubble.centre));
        float3 right = normalize(cross(d, float3(0, 0, 1)));
        float3 up = cross(right, d);
        // Facing the viewer: the surface's normal points back along the line of sight where p is zero.
        float3 normal = normalize(right * p.x + up * p.y - d * z);
        float focus = bubble.focus;

        float facing = clamp(dot(normal, -d), 0.0, 1.0);
        float fresnel = 0.04 + 0.96 * pow(1.0 - facing, 5.0);
        float3 refracted = refract(d, normal, 1.0 / 1.45);
        if (dot(refracted, refracted) < 1e-6) { refracted = d; }
        float3 colour = mix(environment(sky, params, refracted), environment(sky, params, reflect(d, normal)), fresnel);

        float3 light = normalize(float3(-0.55, 0.6, 0.58));
        float specular = pow(clamp(dot(normal, light), 0.0, 1.0), 48.0);
        float edge = smoothstep(0.72, 1.0, r);
        colour += float3(specular) * (0.5 + 0.5 * focus);
        colour += float3(0.62, 0.78, 1.0) * edge * (0.10 + 0.55 * focus);
        colour *= 1.0 - 0.25 * smoothstep(0.0, -1.0, p.y) * (1.0 - edge);

        float2 icon_uv = (in.uv - 0.5) / 0.62 + 0.5;
        if (all(icon_uv >= float2(0.0)) && all(icon_uv <= float2(1.0))) {
            float4 i = icon.sample(linear_clamped, icon_uv);
            // The picture is premultiplied.
            colour = colour * (1.0 - i.a) + i.rgb + float3(specular * 0.6) * i.a;
        }
        float alpha = (0.55 + 0.35 * focus) * (1.0 - smoothstep(0.985, 1.0, r));
        alpha = clamp(alpha + fresnel * 0.35, 0.0, 1.0);
        return float4(colour * alpha, alpha);
    }

    fragment float4 fragment_cursor(Varying in [[stage_in]],
                                    texture2d<float> arrow [[texture(0)]]) {
        return arrow.sample(linear_clamped, in.uv);
    }
    """
}
