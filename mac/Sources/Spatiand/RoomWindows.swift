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
        var info: MacWindowInfo
        /// Capturing only while the window is on show in the room.
        var capture: MacCapture?
        /// Scans in a row that did not find it on the Mac.
        var missed = 0
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
    private let source: CGEventSource? = {
        let source = CGEventSource(stateID: .hidSystemState)
        // Without this, each event posted silences the real mouse for a quarter of a second:
        // over a window of this Mac's, where the pointer is posted as it moves, the trackpad
        // all but stopped moving it.
        source?.localEventsSuppressionInterval = 0
        source?.setLocalEventsFilterDuringSuppressionState([.permitLocalMouseEvents, .permitLocalKeyboardEvents, .permitSystemDefinedEvents], state: .eventSuppressionStateSuppressionInterval)
        return source
    }()
    /// Whether the room's pointer is inside one of these windows now. Read from the event tap.
    private let insideLock = NSLock()
    private var insideNow = false
    var pointerInside: Bool {
        insideLock.lock()
        defer { insideLock.unlock() }
        return insideNow
    }
    private func setInside(_ on: Bool) {
        insideLock.lock()
        insideNow = on
        insideLock.unlock()
    }

    /// The windows in the room, for the menu.
    /// Whether a window is on show in the room.
    func has(_ windowID: CGWindowID) -> Bool {
        insideLock.lock()
        defer { insideLock.unlock() }
        return showing.contains { $0.info.windowID == windowID }
    }

    /// The applications with a window on show, and a process of each: whose sound is placed.
    var onShow: [String: pid_t] {
        insideLock.lock()
        defer { insideLock.unlock() }
        var out: [String: pid_t] = [:]
        for (_, info) in showing where !info.bundle.isEmpty { out[info.bundle] = info.pid }
        return out
    }

    /// The windows on show, kept for whoever asks from another thread. **Nothing waits for the
    /// queue**: it does things to other applications that take as long as those applications
    /// like, and the main thread -- which is where the mouse and keyboard come in -- waiting
    /// behind that is a pointer that has stopped and a button that never comes up.
    private var showing: [(id: UInt32, info: MacWindowInfo)] = []
    private var shownCaptures: [UInt32: MacCapture] = [:]
    private func publish() {
        let now = entries.filter { $0.value.capture != nil }.map { (id: $0.key, info: $0.value.info) }
        var captures: [UInt32: MacCapture] = [:]
        for (id, entry) in entries { if let capture = entry.capture { captures[id] = capture } }
        insideLock.lock()
        shownCaptures = captures
        showing = now
        insideLock.unlock()
    }

    /// The window the left button is down in, while it is: a drag there is the real mouse's.
    private var dragIn: UInt32?
    private func noteDrag(_ id: UInt32?) {
        insideLock.lock()
        dragIn = id
        insideLock.unlock()
    }

    /// While a drag is going on in one of these windows: how far the room's pointer is from
    /// where the Mac's real cursor is, in the window's pixels (nil with no drag, or no way to say).
    func dragLag(cursor: CGPoint) -> CGPoint? {
        insideLock.lock()
        let id = dragIn
        let info = id.flatMap { id in showing.first { $0.id == id } }
        let at = id.flatMap { pointerNow[$0] }
        let capture = id.flatMap { shownCaptures[$0] }
        insideLock.unlock()
        guard id != nil, info != nil else { return nil }
        guard let at, let capture else { return .zero }
        let inWindow = capture.pixel(at: cursor)
        return CGPoint(x: inWindow.x - at.x, y: inWindow.y - at.y)
    }

    /// For tests: where the room's pointer is in a window, in its picture's pixels.
    func pointerIn(_ windowID: CGWindowID) -> CGPoint? {
        insideLock.lock()
        defer { insideLock.unlock() }
        guard let id = showing.first(where: { $0.info.windowID == windowID })?.id else { return nil }
        return pointerNow[id]
    }
    private var pointerNow: [UInt32: CGPoint] = [:]
    private func notePointer(_ id: UInt32, _ p: CGPoint?) {
        insideLock.lock()
        pointerNow[id] = p
        insideLock.unlock()
    }

    /// For tests: the room's number and the details of an application's window that is on show.
    func shownWindow(bundle: String) -> (id: UInt32, info: MacWindowInfo)? {
        insideLock.lock()
        defer { insideLock.unlock() }
        return showing.first { $0.info.bundle == bundle }
    }

    /// A drag by a window's corner asks for a new size with every frame, and a real window takes
    /// a good part of a second over each. So while the sizes keep coming the picture is only
    /// stretched, which the room does by itself, and the real window is resized once, to the last
    /// size asked for, when they stop: the button let go, or the hand held still.
    private let sizes = SettledResize()

    private func wantSize(_ id: UInt32, width: Double, height: Double) {
        sizes.want(id) { [self] in
            insideLock.lock()
            let info = showing.first { $0.id == id }?.info
            insideLock.unlock()
            guard let info else { return }
            let scale = NSScreen.main?.backingScaleFactor ?? 2
            MacWindows.resize(info, toPoints: CGSize(width: width / scale, height: height / scale))
        }
    }

    // MARK: the Mac's windows, all of them

    /// **Every window open on this Mac is in the room's list of windows, put away.** They are
    /// announced as they are found and cost nothing while they are only listed; one is captured
    /// from the moment it is brought out -- from that list, from the launcher by opening its
    /// application, or from the app's menu -- until it is put away again.
    func start() {
        DispatchQueue.main.async { [self] in
            guard scan == nil else { return }
            let t = Timer(timeInterval: 2.0, repeats: true) { [weak self] _ in self?.rescan() }
            RunLoop.main.add(t, forMode: .common)
            scan = t
            rescan()
        }
    }

    /// Announce a window, on this queue. Returns its number in the room.
    private func announce(_ info: MacWindowInfo, hidden: Bool) -> UInt32 {
        if let known = entries.first(where: { $0.value.info.windowID == info.windowID })?.key { return known }
        let id = nextID
        nextID += 1
        entries[id] = Entry(info: info, capture: nil)
        let scale = NSScreen.main?.backingScaleFactor ?? 2
        sp_window_open(id, info.bundle, info.title.isEmpty ? info.app : info.app + " \u{2014} " + info.title,
                       UInt32(info.frame.width * scale), UInt32(info.frame.height * scale), hidden)
        return id
    }

    /// Bring one window out, in front of the wearer.
    func bring(_ info: MacWindowInfo) {
        queue.async { [self] in
            let id = announce(info, hidden: true)
            sp_window_show(id)
        }
    }

    /// Bring out every window an application has, and the ones it opens from now on.
    func follow(bundle: String) {
        queue.async { [self] in
            following.insert(bundle)
            for (id, entry) in entries where entry.info.bundle == bundle { sp_window_show(id) }
        }
        start()
        DispatchQueue.main.async { self.rescan() }
    }

    private func rescan() {
        guard MacWindows.allowed(ask: false) else { return }
        Task {
            let all = await MacWindows.list()
            queue.async { [self] in
                for info in all where !entries.values.contains(where: { $0.info.windowID == info.windowID }) {
                    // A new window of an application that was opened from the room comes out at
                    // once; any other waits in the list.
                    let wanted = following.contains(info.bundle)
                    let id = announce(info, hidden: true)
                    if wanted { sp_window_show(id) }
                }
                for (id, entry) in entries {
                    if let now = all.first(where: { $0.windowID == entry.info.windowID }) {
                        entries[id]?.missed = 0
                        if now.title != entry.info.title, !now.title.isEmpty {
                            entries[id]?.info = now
                            sp_window_title(id, now.app + " \u{2014} " + now.title)
                        }
                    } else if entry.capture == nil {
                        // Gone from the Mac's screen, and not one being shown (whose capture
                        // says when it ends): after a second look, gone from the list.
                        entries[id]?.missed += 1
                        if entry.missed >= 1 { gone(id) }
                    }
                }
            }
        }
    }

    /// The window is on show in the room: capture it.
    private func shown(_ id: UInt32) {
        guard var entry = entries[id], entry.capture == nil else { return }
        let info = entry.info
        let capture = MacCapture(info)
        capture.onFrame = { [weak capture] pixels, _ in
            guard let surface = CVPixelBufferGetIOSurface(pixels)?.takeUnretainedValue() else { return }
            let c = capture?.lastContent ?? .zero
            sp_window_picture(id, Unmanaged.passUnretained(surface).toOpaque(), UInt32(max(0, c.minX)), UInt32(max(0, c.minY)), UInt32(max(0, c.width)), UInt32(max(0, c.height)))
        }
        capture.onEnded = { [weak self] in self?.gone(id) }
        entry.capture = capture
        entries[id] = entry
        publish()
        Task {
            do { try await capture.start() } catch {
                print("windows: could not capture \(info.app): \(error)")
                self.gone(id)
            }
        }
        print("windows: \(info.app) \u{201C}\(info.title)\u{201D} is on show as \(id)")
    }

    /// Put away: nothing needs its picture.
    private func hidden(_ id: UInt32) {
        guard let capture = entries[id]?.capture else { return }
        capture.invalidate()
        entries[id]?.capture = nil
        publish()
        at[id] = nil
        setInside(!at.isEmpty)
    }

    private func gone(_ id: UInt32) {
        queue.async { [self] in
            guard let entry = entries.removeValue(forKey: id) else { return }
            entry.capture?.invalidate()
            publish()
            at[id] = nil
            setInside(!at.isEmpty)
            down.remove(id)
            if focused == id { focused = 0 }
            sp_window_close(id)
        }
    }

    /// Everything out of the room, as the session ends.
    func clear() {
        DispatchQueue.main.async { self.scan?.invalidate(); self.scan = nil }
        queue.async { [self] in
            for (id, entry) in entries {
                entry.capture?.invalidate()
                sp_window_close(id)
            }
            entries.removeAll()
            following.removeAll()
            publish()
        }
    }

    // MARK: what the compositor asks

    /// Called from the compositor's threads; see `spatiand_mac.h` for what each means.
    func asked(_ what: Int32, id: UInt32, a: Double, b: Double, c: Double) {
        if what == SP_WINDOW_RESIZE { wantSize(id, width: a, height: b); return }
        queue.async { [self] in
            guard what == SP_WINDOW_FOCUS || entries[id] != nil else { return }
            switch Int(what) {
            case SP_WINDOW_SHOWN: shown(id)
            case SP_WINDOW_HIDDEN: hidden(id)
            case SP_WINDOW_FOCUS: focus(id)
            case SP_WINDOW_MOTION: motion(id, x: a, y: b)
            case SP_WINDOW_LEAVE:
                at[id] = nil
                notePointer(id, nil)
                setInside(!at.isEmpty)
            case SP_WINDOW_BUTTON: button(id, code: Int(a), pressed: b != 0)
            case SP_WINDOW_SCROLL: scroll(id, across: a, down: b)
            case SP_WINDOW_KEY: if a >= 0 { key(UInt16(a), down: b != 0) }
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
        guard let capture = entries[id]?.capture, let p = at[id] else { return nil }
        return capture.screenPoint(x: Double(p.x), y: Double(p.y))
    }

    private func motion(_ id: UInt32, x: Double, y: Double) {
        // Onto a window it was not on: that window comes to the top, so that what is under the
        // real pointer is this window and not whichever lay over it on the Mac's own screen.
        // (It is not brought to the front for being pointed at: that took a quarter of a second
        // each time, and pointing across one window on the way to the room's keyboard made it
        // the Mac's front window, which is where the keys then went. A press brings it forward.)
        at[id] = CGPoint(x: x, y: y)
        notePointer(id, at[id])
        setInside(true)
        guard let p = point(id) else { return }
        // A real pointer, really there: what makes hover, tooltips and the cursor's shape work.
        // **A drag is the real mouse's, untouched.** While the button is down in one of these
        // windows the hand's own events go straight to the application (see `RoomTap`), with
        // its own travel and its own cursor -- which an application turning a 3D object holds
        // still, or hides, as it likes -- and the room's pointer is the one that follows.
        if down.contains(id) { return }
        // Only pointed at, the real cursor is left alone and the application is told by an
        // event of its own: enough for what lights up under a pointer.
        if let pid = entries[id]?.info.pid, let event = CGEvent(mouseEventSource: source, mouseType: .mouseMoved, mouseCursorPosition: p, mouseButton: .left) {
            event.setIntegerValueField(.eventSourceUserData, value: spatiandEventMark)
            event.postToPid(pid)
        }
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
            RoomTap.shared.warp(to: p)
            // Twice in the same place, soon enough, is a double click: said in the event, since
            // the system that would have counted them never saw them.
            let now = Date()
            let near = hypot(p.x - lastClick.1.x, p.y - lastClick.1.y) < 6
            let count = button == .left && near && now.timeIntervalSince(lastClick.0) < NSEvent.doubleClickInterval ? lastClick.2 + 1 : 1
            if button == .left { lastClick = (now, p, count); down.insert(id); noteDrag(id) }
            let event = CGEvent(mouseEventSource: source, mouseType: downType, mouseCursorPosition: p, mouseButton: button)
            event?.setIntegerValueField(.mouseEventClickState, value: Int64(count))
            post(event)
        } else {
            let dragged = button == .left && down.contains(id)
            if button == .left { down.remove(id); noteDrag(nil) }
            // Let go where the real cursor is, which is where the drag has taken it.
            let p = dragged ? (CGEvent(source: nil)?.location ?? p) : p
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
        // To the window the room says has the keys, whatever the Mac has in front by now.
        if down, focused != 0, let pid = entries[focused]?.info.pid, NSWorkspace.shared.frontmostApplication?.processIdentifier != pid {
            raise(focused)
        }
        let event = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(code), keyDown: down)
        event?.flags = RoomTap.shared.flags
        if isRepeat { event?.setIntegerValueField(.keyboardEventAutorepeat, value: 1) }
        post(event)
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

/// Sizes asked for in a stream, given once the stream stops. For this Mac's own windows only: a
/// Wayland application resizes as fast as it is asked and is not treated so.
final class SettledResize {
    private let queue = DispatchQueue(label: "spatiand.resize", qos: .userInitiated)
    private let lock = NSLock()
    private var waiting: [UInt32: DispatchWorkItem] = [:]
    /// How long nothing new must have been asked for.
    var quiet = 0.15

    func want(_ window: UInt32, _ apply: @escaping () -> Void) {
        let item = DispatchWorkItem(block: apply)
        lock.lock()
        waiting[window]?.cancel()
        waiting[window] = item
        lock.unlock()
        queue.asyncAfter(deadline: .now() + quiet, execute: item)
    }
}
