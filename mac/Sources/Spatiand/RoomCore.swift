//  RoomCore.swift — the glasses' world, as the Rust side keeps it.
//
//  Where the windows are, where the head is, what is being pointed at, and what to draw are all
//  arithmetic, and live in crates/spatiand-mac-core/src/room.rs where they are tested without a
//  headset. This is a thin Swift face on that: no logic of its own.

import CSpatiand
import Foundation

final class RoomCore {
    private let room: OpaquePointer
    /// A window's id that stands for the cursor in what is drawn.
    static let cursor: UInt32 = 0xFFFF

    init() { room = sp_room_new() }
    deinit { sp_room_free(room) }

    // The head.
    func imu(timestamp: UInt64, gyro: SIMD3<Double>, accel: SIMD3<Double>, mag: SIMD3<Double>) {
        var g = [gyro.x, gyro.y, gyro.z], a = [accel.x, accel.y, accel.z], m = [mag.x, mag.y, mag.z]
        sp_room_imu(room, timestamp, &g, &a, &m)
    }
    func recentre() { sp_room_recentre(room) }
    var hasHead: Bool { sp_room_has_head(room) != 0 }
    /// For a preview with no sensors.
    func holdHead(yaw: Double, pitch: Double) { sp_room_set_head(room, 1, yaw, pitch) }
    var headDegrees: (yaw: Double, pitch: Double, roll: Double) {
        var out = [0.0, 0.0, 0.0]
        sp_room_head_euler(room, &out)
        return (out[0], out[1], out[2])
    }
    func cursorShape(hotX: Double, hotY: Double, width: Double, height: Double) {
        sp_room_set_cursor_shape(room, hotX, hotY, width, height)
    }
    func perEye(_ width: Int, _ height: Int) { sp_room_set_per_eye(room, UInt32(width), UInt32(height)) }

    // The windows.
    func setWindow(_ id: UInt16, width: Int, height: Int) { sp_room_set_window(room, UInt32(id), UInt32(width), UInt32(height)) }
    func setPanel(_ id: UInt16, widthPx: Int, heightPx: Int, widthM: Double, radiusM: Double) {
        sp_room_set_panel(room, UInt32(id), UInt32(widthPx), UInt32(heightPx), widthM, radiusM)
    }
    func setApp(_ id: UInt16, _ app: String) { sp_room_set_app(room, UInt32(id), app) }

    /// An application's sound placed in the room and folded to two ears, interleaved stereo.
    func audio(app: String, channels: Int, pcm: Data) -> [Float] {
        let capacity = pcm.count / 2 / max(1, channels) * 2 + 16
        var out = [Float](repeating: 0, count: capacity)
        let written = pcm.withUnsafeBytes { raw -> Int in
            guard let base = raw.bindMemory(to: UInt8.self).baseAddress else { return 0 }
            return sp_room_audio(room, app, UInt16(channels), base, pcm.count, &out, capacity)
        }
        return Array(out.prefix(written))
    }
    func show(_ id: UInt16) { sp_room_show(room, UInt32(id)) }
    func remove(_ id: UInt16) { sp_room_remove(room, UInt32(id)) }
    func clear() { sp_room_clear(room) }
    var focused: UInt16? {
        get { let f = sp_room_focused(room); return f >= 0 ? UInt16(f) : nil }
        set { sp_room_focus(room, newValue.map(Int32.init) ?? -1) }
    }

    // The pointer.
    func movePointer(dx: Double, dy: Double) { sp_room_move_pointer(room, dx, dy) }
    func centrePointer() { sp_room_centre_pointer(room) }

    struct Aim { var window: UInt16?; var x: Double; var y: Double; var title = false; var corner = false }
    func aim() -> Aim {
        var out = sp_aim()
        sp_room_aim(room, &out)
        return Aim(window: out.window >= 0 ? UInt16(out.window) : nil, x: out.x, y: out.y, title: out.title != 0, corner: out.corner != 0)
    }
    /// Where the pointer is in one window's own pixels, even off its edge.
    func aim(at id: UInt16) -> (x: Double, y: Double)? {
        var x = 0.0, y = 0.0
        return sp_room_aim_at(room, UInt32(id), &x, &y) != 0 ? (x, y) : nil
    }

    /// Where the pointer is in a window's pixels, past its edge as well.
    func aimFree(_ id: UInt16) -> (x: Double, y: Double)? {
        var x = 0.0, y = 0.0
        return sp_room_aim_free(room, UInt32(id), &x, &y) != 0 ? (x, y) : nil
    }
    /// The size to ask the host for to keep a window's pixel density at the width it has now.
    func nativeSize(_ id: UInt16) -> (w: Int, h: Int)? {
        var w: UInt32 = 0, h: UInt32 = 0
        return sp_room_native_size(room, UInt32(id), &w, &h) != 0 ? (Int(w), Int(h)) : nil
    }
    /// Note that the host is being asked for about this size; the size to ask for, as limited.
    func requestSize(_ id: UInt16, width: Double, height: Double) -> (w: Int, h: Int)? {
        var w: UInt32 = 0, h: UInt32 = 0
        return sp_room_request_size(room, UInt32(id), width, height, &w, &h) != 0 ? (Int(w), Int(h)) : nil
    }

    // Moving windows about.
    func beginGrab(_ id: UInt16) { sp_room_begin_grab(room, UInt32(id)) }
    func drag() { sp_room_drag(room) }
    func endGrab() { sp_room_end_grab(room) }
    var isGrabbing: Bool { sp_room_grabbed(room) >= 0 }
    func scale(_ id: UInt16, by factor: Double) { sp_room_scale(room, UInt32(id), factor) }
    func nudge(_ id: UInt16, yaw: Double, pitch: Double) { sp_room_nudge(room, UInt32(id), yaw, pitch) }
    /// Move the keyboard to the next window round the room: +1 to the left, -1 to the right.
    func focusStep(_ step: Int) -> UInt16? {
        let id = sp_room_focus_step(room, Int32(step))
        return id >= 0 ? UInt16(id) : nil
    }
    /// Make a window the room: its application drew both eyes itself. Nil puts it back as a panel.
    func setProjection(_ id: UInt16?) {
        projection = id
        sp_room_set_projection(room, id.map { Int32($0) } ?? -1)
    }
    /// The application draws the pointer itself where it is over this window: the room draws none.
    func setCursorDrawn(_ id: UInt16, _ drawn: Bool) {
        sp_room_set_cursor_drawn(room, UInt32(id), drawn ? 1 : 0)
    }
    /// The window that is the room, if one is.
    private(set) var projection: UInt16?
    var handle: OpaquePointer? { room }
    /// Turn the room by the head its newest picture was drawn for, when the host says which.
    func refreshFrameHead() {
        guard let id = projection else { return }
        var q = [Float](repeating: 0, count: 4)
        if sp_frame_head(id, &q) != 0 {
            sp_room_set_frame_head(room, q)
        } else {
            sp_room_set_frame_head(room, nil)
        }
    }
    func arrange(spread: Double) { sp_room_arrange(room, spread) }
    func bringHere(_ id: UInt16) { sp_room_bring_here(room, UInt32(id)) }
    func setPinned(_ id: UInt16, _ pinned: Bool) { sp_room_set_pinned(room, UInt32(id), pinned ? 1 : 0) }
    func isPinned(_ id: UInt16) -> Bool { sp_room_is_pinned(room, UInt32(id)) != 0 }
    func nextCorner() { sp_room_next_corner(room) }
    func toggleSize() { sp_room_toggle_size(room) }

    // What to draw.
    struct Draw { var window: UInt32; var first: Int; var count: Int; var focused: Bool; var aimed: Bool; var pinned: Bool; var room: Bool }
    struct Frame { var matrices: [Float]; var floats: Int; var draws: [Draw] }

    /// Fill `vertices` (room for `capacity` floats) and say what is in it. Nil if it would not fit.
    func frame(vertices: UnsafeMutablePointer<Float>, capacity: Int) -> Frame? {
        var matrices = [Float](repeating: 0, count: 32)
        var draws = [sp_draw](repeating: sp_draw(), count: 128)
        var count: UInt32 = 0
        let written = sp_room_frame(room, &matrices, vertices, UInt32(capacity), &draws, 128, &count)
        guard count > 0 else { return nil }
        return Frame(
            matrices: matrices, floats: Int(written),
            draws: draws.prefix(Int(count)).map {
                Draw(window: $0.window, first: Int($0.first), count: Int($0.count),
                     focused: $0.flags & 1 != 0, aimed: $0.flags & 2 != 0, pinned: $0.flags & 4 != 0, room: $0.flags & 8 != 0)
            })
    }
}
