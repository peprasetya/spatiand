//  RoomWindows.swift — windows of this Mac's own applications, in the room.
//
//  A window brought into the room is captured (`MacCapture`) and each picture handed to the
//  compositor as it is, a GPU surface with nothing copied (`sp_window_picture`). There it is an
//  ordinary window: the compositor frames it, moves it, pushes it away and points at it, and what
//  the wearer does *inside* it comes back here as "the pointer is at this pixel of window 7",
//  "this key went down" -- which is then done to the real window.
//
//  **Done as a person at the Mac would do it.** The real pointer is put over that spot of the real
//  window and the click happens there, with the application in front. Events addressed to a
//  window in the background were tried first and most things took them; what did not -- pop-up
//  buttons in Electron applications, anything that asks the system where the mouse is -- is a
//  long tail not worth chasing. A real click at a real place is what every application handles.
//  The Mac's own screen is out of sight while this happens, so its pointer moving there is not
//  something the wearer sees.

import AppKit
import ApplicationServices
import CSpatiand
import CoreVideo

final class RoomWindows {
    static let shared = RoomWindows()

    private struct Entry {
        let info: MacWindowInfo
        let capture: MacCapture
    }

    /// Everything here is touched on this queue only: the compositor's callbacks arrive on its
    /// own threads, in the order things happened, and are done in that order.
    private let queue = DispatchQueue(label: "spatiand.roomwindows", qos: .userInteractive)
    private var entries: [UInt32: Entry] = [:]
    private var nextID: UInt32 = 1
    /// Applications whose windows are brought in as they open.
    private var following: Set<String> = []
    private var scan: Timer?
    private var focused: UInt32 = 0
    /// Where the pointer is in each window, in its pixels, as last said.
    private var at: [UInt32: CGPoint] = [:]
    private var down: Set<UInt32> = []
    private var lastClick: (Date, CGPoint, Int) = (.distantPast, .zero, 0)
    private let source = CGEventSource(stateID: .hidSystemState)

    /// The windows in the room, for the menu.
    var titles: [String] { queue.sync { entries.values.map { $0.info.app + ($0.info.title.isEmpty ? "" : " \u{2014} " + $0.info.title) } } }

    func has(_ windowID: CGWindowID) -> Bool { queue.sync { entries.values.contains { $0.info.windowID == windowID } } }

    // MARK: bringing windows in

    /// Bring one window into the room.
    func bring(_ info: MacWindowInfo) {
        queue.async { [self] in
            guard !entries.values.contains(where: { $0.info.windowID == info.windowID }) else { return }
            let id = nextID
            nextID += 1
            let capture = MacCapture(info)
            capture.onFrame = { pixels, _ in
                guard let surface = CVPixelBufferGetIOSurface(pixels)?.takeUnretainedValue() else { return }
                sp_window_picture(id, Unmanaged.passUnretained(surface).toOpaque())
            }
            capture.onEnded = { [weak self] in self?.gone(id) }
            entries[id] = Entry(info: info, capture: capture)
            sp_window_open(id, info.bundle, info.title.isEmpty ? info.app : info.title)
            Task {
                do { try await capture.start() } catch {
                    print("windows: could not capture \(info.app): \(error)")
                    self.gone(id)
                }
            }
            print("windows: \(info.app) \u{201C}\(info.title)\u{201D} is in the room as \(id)")
        }
    }

    /// Bring every window an application has, and the ones it opens from now on.
    func follow(bundle: String) {
        queue.async { self.following.insert(bundle) }
        DispatchQueue.main.async { [self] in
            if scan == nil {
                let t = Timer(timeInterval: 1.0, repeats: true) { [weak self] _ in self?.rescan() }
                RunLoop.main.add(t, forMode: .common)
                scan = t
            }
            rescan()
        }
    }

    private func rescan() {
        Task {
            let all = await MacWindows.list()
            queue.async { [self] in
                for info in all where following.contains(info.bundle) && !entries.values.contains(where: { $0.info.windowID == info.windowID }) {
                    DispatchQueue.main.async { self.bring(info) }
                }
                // A window that has closed on the Mac leaves the room; one retitled is retitled.
                for (id, entry) in entries {
                    if let now = all.first(where: { $0.windowID == entry.info.windowID }) {
                        if now.title != entry.info.title, !now.title.isEmpty {
                            entries[id] = Entry(info: now, capture: entry.capture)
                            sp_window_title(id, now.title)
                        }
                    }
                }
            }
        }
    }

    private func gone(_ id: UInt32) {
        queue.async { [self] in
            guard let entry = entries.removeValue(forKey: id) else { return }
            entry.capture.invalidate()
            at[id] = nil
            down.remove(id)
            if focused == id { focused = 0 }
            sp_window_close(id)
            print("windows: \(entry.info.app) has left the room")
        }
    }

    /// Everything out of the room, as the session ends.
    func clear() {
        queue.async { [self] in
            for (id, entry) in entries {
                entry.capture.invalidate()
                sp_window_close(id)
            }
            entries.removeAll()
            following.removeAll()
        }
    }

    // MARK: what the compositor asks

    /// Called from the compositor's threads; see `spatiand_mac.h` for what each means.
    func asked(_ what: Int32, id: UInt32, a: Double, b: Double, c: Double) {
        queue.async { [self] in
            guard what == SP_WINDOW_FOCUS || entries[id] != nil else { return }
            switch Int(what) {
            case SP_WINDOW_FOCUS: focus(id)
            case SP_WINDOW_MOTION: motion(id, x: a, y: b)
            case SP_WINDOW_LEAVE: at[id] = nil
            case SP_WINDOW_BUTTON: button(id, code: Int(a), pressed: b != 0)
            case SP_WINDOW_SCROLL: scroll(id, across: a, down: b)
            case SP_WINDOW_KEY: if a >= 0 { key(UInt16(a), down: b != 0) }
            case SP_WINDOW_RESIZE: resize(id, width: a, height: b)
            case SP_WINDOW_CLOSE: close(id)
            default: break
            }
        }
    }

    /// A key held down and repeating, which only the window that has the keys is told.
    func repeated(_ code: UInt16) {
        queue.async { [self] in if focused != 0, entries[focused] != nil { key(code, down: true, isRepeat: true) } }
    }

    private func post(_ event: CGEvent?) {
        guard let event else { return }
        event.setIntegerValueField(.eventSourceUserData, value: spatiandEventMark)
        event.post(tap: .cghidEventTap)
    }

    /// Bring the window's application to the front and the window to the top of it, and wait --
    /// briefly -- until it is: a click made before then lands on whatever was there.
    private func raise(_ id: UInt32) {
        guard let entry = entries[id] else { return }
        let info = entry.info
        if let ax = MacWindows.axWindow(info) {
            AXUIElementPerformAction(ax, kAXRaiseAction as CFString)
            AXUIElementSetAttributeValue(ax, kAXMainAttribute as CFString, kCFBooleanTrue)
        }
        let app = NSRunningApplication(processIdentifier: info.pid)
        if app?.isActive != true {
            app?.activate(options: [])
            let deadline = Date().addingTimeInterval(0.25)
            while app?.isActive != true, Date() < deadline { usleep(5_000) }
        }
    }

    private func focus(_ id: UInt32) {
        focused = id
        if id != 0 { raise(id) }
    }

    private func point(_ id: UInt32) -> CGPoint? {
        guard let entry = entries[id], let p = at[id] else { return nil }
        return entry.capture.screenPoint(x: Double(p.x), y: Double(p.y))
    }

    private func motion(_ id: UInt32, x: Double, y: Double) {
        at[id] = CGPoint(x: x, y: y)
        guard let p = point(id) else { return }
        // A real pointer, really there: what makes hover, tooltips and the cursor's shape work.
        let type: CGEventType = down.contains(id) ? .leftMouseDragged : .mouseMoved
        post(CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: p, mouseButton: .left))
    }

    private func button(_ id: UInt32, code: Int, pressed: Bool) {
        guard let p = point(id) else { return }
        let (button, downType, upType): (CGMouseButton, CGEventType, CGEventType)
        switch code {
        case 0x111: (button, downType, upType) = (.right, .rightMouseDown, .rightMouseUp)
        case 0x112: (button, downType, upType) = (.center, .otherMouseDown, .otherMouseUp)
        default: (button, downType, upType) = (.left, .leftMouseDown, .leftMouseUp)
        }
        if pressed {
            if focused != id { focus(id) } else { raise(id) }
            // Twice in the same place, soon enough, is a double click: said in the event, since
            // the system that would have counted them never saw them.
            let now = Date()
            let near = hypot(p.x - lastClick.1.x, p.y - lastClick.1.y) < 6
            let count = button == .left && near && now.timeIntervalSince(lastClick.0) < NSEvent.doubleClickInterval ? lastClick.2 + 1 : 1
            if button == .left { lastClick = (now, p, count); down.insert(id) }
            let event = CGEvent(mouseEventSource: source, mouseType: downType, mouseCursorPosition: p, mouseButton: button)
            event?.setIntegerValueField(.mouseEventClickState, value: Int64(count))
            post(event)
        } else {
            if button == .left { down.remove(id) }
            let event = CGEvent(mouseEventSource: source, mouseType: upType, mouseCursorPosition: p, mouseButton: button)
            event?.setIntegerValueField(.mouseEventClickState, value: Int64(button == .left ? lastClick.2 : 1))
            post(event)
        }
    }

    private func scroll(_ id: UInt32, across: Double, down: Double) {
        guard let p = point(id) else { return }
        // The compositor's units are a wheel's: fifteen to a notch, and near enough to pixels.
        let event = CGEvent(scrollWheelEvent2Source: source, units: .pixel, wheelCount: 2, wheel1: Int32(-down.rounded()), wheel2: Int32(-across.rounded()), wheel3: 0)
        event?.location = p
        post(event)
    }

    private func key(_ code: UInt16, down: Bool, isRepeat: Bool = false) {
        // Modifier keys are not posted as keys: they are the flags on the keys that follow, and
        // the flags are the real keyboard's, which is the one being typed on.
        if [0x38, 0x3C, 0x3B, 0x3E, 0x3A, 0x3D, 0x37, 0x36, 0x39].contains(code) {
            let event = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(code), keyDown: down)
            event?.type = .flagsChanged
            event?.flags = RoomTap.shared.flags
            post(event)
            return
        }
        let event = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(code), keyDown: down)
        event?.flags = RoomTap.shared.flags
        if isRepeat { event?.setIntegerValueField(.keyboardEventAutorepeat, value: 1) }
        post(event)
    }

    private func resize(_ id: UInt32, width: Double, height: Double) {
        guard let entry = entries[id] else { return }
        let scale = entry.capture.scale
        MacWindows.resize(entry.info, toPoints: CGSize(width: width / scale, height: height / scale))
    }

    private func close(_ id: UInt32) {
        guard let entry = entries[id] else { return }
        // The window's own close button, pressed: so a document that is unsaved asks, as it would.
        if let ax = MacWindows.axWindow(entry.info) {
            var button: CFTypeRef?
            if AXUIElementCopyAttributeValue(ax, kAXCloseButtonAttribute as CFString, &button) == .success, let button {
                AXUIElementPerformAction(button as! AXUIElement, kAXPressAction as CFString)
                return
            }
        }
        gone(id)
    }
}
