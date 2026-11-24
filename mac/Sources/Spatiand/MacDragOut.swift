//  Files dragged out of an application of the room's -- SpatiShelf -- onto a window of this
//  Mac's own.
//
//  The room's drag is Wayland's, between its clients; a Mac application takes a drop only from
//  the Mac's own drag. So while the room's drag is over one of this Mac's windows the same files
//  are dragged there as the Mac's own, by this application, and let go of where the room's are.
//  The application of the room's knows nothing of it: its drag ends over a window that took
//  nothing from it, and what becomes of the files is the Mac application's doing.
//
//  **A workaround, named as one** (the owner said yes to it, 2026-11-24): macOS lets an
//  application start a drag only in answer to a press of the mouse in a window of its own. There
//  is no press: the button is held in the room, which has the mouse. So a small window nobody
//  can see is put under the Mac's cursor and a press is posted into it, which is what the drag
//  is begun with; then the window is taken out of the way. The proper way would be for macOS to
//  let a drag be begun without one, which it has no call for.

import AppKit

final class MacDragOut: NSObject, NSDraggingSource {
    static let shared = MacDragOut()

    private enum Phase {
        /// Nothing of the Mac's is being dragged.
        case idle
        /// The press has been posted into the window, and the drag not yet begun with it.
        case starting(since: Date)
        case dragging
        /// The button has been let go of, and the drag's end not yet heard.
        case ending
    }

    private var phase = Phase.idle
    private var files: [URL] = []
    /// Where the files are, on the Mac's screen (from its top left, as events are).
    private var target = CGPoint.zero
    /// Let go of, or gone from this Mac's windows, before the drag had begun.
    private var dropWanted = false, leaveWanted = false
    /// Over one of this Mac's windows again before the last drag's end was heard.
    private var againWanted = false
    private var panel: NSPanel?
    private let source: CGEventSource? = {
        let source = CGEventSource(stateID: .hidSystemState)
        source?.localEventsSuppressionInterval = 0
        return source
    }()

    /// The files are at `p` of the Mac's screen, over one of its windows; `files` the first time.
    func over(_ p: CGPoint, files new: [URL]?) {
        DispatchQueue.main.async { [self] in
            if let new { files = new }
            target = p
            leaveWanted = false
            switch phase {
            case .idle: begin()
            case .starting: break
            case .dragging: move()
            case .ending: againWanted = true
            }
        }
    }

    /// Let go of, where they last were.
    func drop() {
        DispatchQueue.main.async { [self] in
            switch phase {
            case .starting: dropWanted = true
            case .dragging: letGo()
            case .idle, .ending: break
            }
        }
    }

    /// Gone from this Mac's windows without being let go of: the Mac's drag is called off.
    func leave() {
        DispatchQueue.main.async { [self] in
            againWanted = false
            switch phase {
            case .starting: leaveWanted = true
            case .dragging: callOff()
            case .idle, .ending: break
            }
        }
    }

    // MARK: the drag

    private func begin() {
        guard !files.isEmpty else { return }
        dropWanted = false
        againWanted = false
        let panel = self.panel ?? made()
        self.panel = panel
        place(panel, at: target)
        panel.orderFrontRegardless()
        phase = .starting(since: Date())
        print("drag out: \(files.count) file(s) of the room's over a window of this Mac's; pressing in a window of this application's to begin the Mac's drag")
        // A moment for the window to be the one on top there, then the press into it.
        let at = target
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.04) { [self] in
            guard case .starting = phase else { return }
            RoomTap.shared.warp(to: at)
            post(.leftMouseDown, at: at, from: at)
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) { [self] in
            guard case .starting = phase else { return }
            // The press never came to the window: nothing is dragged, and the button is let go.
            print("drag out: the press did not reach this application's window; no drag of the Mac's begun")
            post(.leftMouseUp, at: at, from: at)
            panel.orderOut(nil)
            phase = .idle
        }
    }

    private func made() -> NSPanel {
        let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 24, height: 24), styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isOpaque = false
        // Not quite see-through: a window with nothing drawn in it is not pressed at all.
        panel.backgroundColor = NSColor(white: 0.5, alpha: 0.02)
        panel.hasShadow = false
        panel.level = .popUpMenu
        panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle, .fullScreenAuxiliary]
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        let view = PressView(frame: NSRect(x: 0, y: 0, width: 24, height: 24))
        view.pressed = { [weak self] event, view in self?.pressed(event, in: view) }
        panel.contentView = view
        return panel
    }

    /// The window with its middle at `p` (counted from the top left of the first screen, as
    /// events are; a window is placed from the bottom left).
    private func place(_ panel: NSPanel, at p: CGPoint) {
        let height = NSScreen.screens.first?.frame.height ?? 0
        let size = panel.frame.size
        panel.setFrameOrigin(NSPoint(x: p.x - size.width / 2, y: height - p.y - size.height / 2))
    }

    /// The press has come to the window: the drag is begun with it.
    private func pressed(_ event: NSEvent, in view: NSView) {
        guard case .starting = phase else { return }
        let items = files.enumerated().map { index, url -> NSDraggingItem in
            let item = NSDraggingItem(pasteboardWriter: url as NSURL)
            let icon = NSWorkspace.shared.icon(forFile: url.path)
            icon.size = NSSize(width: 48, height: 48)
            let shift = CGFloat(min(index, 4)) * 4
            item.setDraggingFrame(NSRect(x: shift - 12, y: -shift - 12, width: 48, height: 48), contents: icon)
            return item
        }
        let session = view.beginDraggingSession(with: items, event: event, source: self)
        session.animatesToStartingPositionsOnCancelOrFail = false
    }

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        // Copied, or taken as the application takes a file (opened, attached, linked to): never
        // moved, which the room's application was not asked about.
        context == .outsideApplication ? [.copy, .generic, .link] : []
    }

    func draggingSession(_ session: NSDraggingSession, willBeginAt screenPoint: NSPoint) {
        phase = .dragging
        // Out of the way: what is under the cursor is to be the window the files are over.
        if let panel, let screen = panel.screen ?? NSScreen.screens.first {
            panel.setFrameOrigin(NSPoint(x: screen.frame.minX, y: screen.frame.minY))
        }
        print("drag out: the Mac's drag has begun")
        move()
        // Once more in a moment, so that the window under it has heard it come and move.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.06) { [self] in
            guard case .dragging = phase else { return }
            if leaveWanted { callOff() } else {
                nudge()
                if dropWanted { letGo() }
            }
        }
    }

    func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint, operation: NSDragOperation) {
        print("drag out: the Mac's drag ended; \(operation.isEmpty ? "nothing took the files" : "taken (operation \(operation.rawValue))")")
        panel?.orderOut(nil)
        phase = .idle
        dropWanted = false
        leaveWanted = false
        if againWanted { againWanted = false; begin() }
    }

    func ignoreModifierKeys(for session: NSDraggingSession) -> Bool { true }

    private func move() {
        let here = CGEvent(source: nil)?.location ?? target
        RoomTap.shared.warp(to: target)
        post(.leftMouseDragged, at: target, from: here)
    }

    /// A movement of a point and back, where the files are: for the window under them.
    private func nudge() {
        let beside = CGPoint(x: target.x + 1, y: target.y)
        post(.leftMouseDragged, at: beside, from: target)
        post(.leftMouseDragged, at: target, from: beside)
    }

    private func letGo() {
        guard case .dragging = phase else { return }
        dropWanted = false
        phase = .ending
        move()
        nudge()
        // The window under them says what it would do with them at a movement; a moment for
        // that, and the button is let go of.
        let at = target
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { [self] in
            post(.leftMouseUp, at: at, from: at)
            ended(after: 3)
        }
    }

    /// Called off: let go of over this application's own window, which takes nothing.
    private func callOff() {
        guard case .dragging = phase, let panel else { return }
        leaveWanted = false
        phase = .ending
        let at = CGEvent(source: nil)?.location ?? target
        place(panel, at: at)
        panel.orderFrontRegardless()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { [self] in
            post(.leftMouseDragged, at: CGPoint(x: at.x + 1, y: at.y), from: at)
            post(.leftMouseDragged, at: at, from: at)
            post(.leftMouseUp, at: at, from: at)
            ended(after: 3)
        }
    }

    /// The drag's end is heard from macOS; if it is not, this is not left waiting for ever.
    private func ended(after seconds: Double) {
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { [self] in
            guard case .ending = phase else { return }
            print("drag out: the Mac's drag's end was not heard; taken as ended")
            panel?.orderOut(nil)
            phase = .idle
            if againWanted { againWanted = false; begin() }
        }
    }

    private func post(_ type: CGEventType, at p: CGPoint, from: CGPoint) {
        guard let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: p, mouseButton: .left) else { return }
        event.setIntegerValueField(.mouseEventClickState, value: 1)
        event.setIntegerValueField(.mouseEventDeltaX, value: Int64((p.x - from.x).rounded()))
        event.setIntegerValueField(.mouseEventDeltaY, value: Int64((p.y - from.y).rounded()))
        event.setIntegerValueField(.eventSourceUserData, value: spatiandEventMark)
        event.post(tap: .cghidEventTap)
    }
}

/// What the window that is pressed holds: it says it was pressed, first press or not.
private final class PressView: NSView {
    var pressed: ((NSEvent, NSView) -> Void)?
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseDown(with event: NSEvent) { pressed?(event, self) }
}
