//  MacScreens.swift — a whole screen of this Mac's, as a window in the room.
//
//  A window of an application is one thing; a screen is everything on it, arranged as the Mac
//  arranges it, with the Mac's own menu bar and Dock. Shown in the room -- or to a Deck this Mac
//  is a host for -- it is the Mac's desktop as a window: the picture is the display's, and the
//  pointer in it is the Mac's real one, put wherever the room's is.
//
//  A screen is either one the Mac has, or one made for the purpose: a virtual display, which
//  macOS takes for a monitor plugged in, with a desktop of its own to put windows on and nothing
//  showing it but this. It is made the first time it is asked for and lasts until Spatiand quits.
//
//  To everything else here a screen is a window whose application is called `spatiand.screen`:
//  that name is how the room's list of windows knows to put it under Screens.

import AppKit
import CSpatiand

enum MacScreens {
    static let bundle = "spatiand.screen"
    /// The display a screen stands for until the virtual one has been made.
    static let virtualToBe: CGDirectDisplayID = 0x0FFF_FFF0
    /// The sizes a virtual screen can be made: the owner's choice, in the settings.
    static let sizes: [(name: String, width: Int, height: Int)] = [
        ("1920 \u{00D7} 1080", 1920, 1080), ("2560 \u{00D7} 1440", 2560, 1440), ("3840 \u{00D7} 2160 (4K)", 3840, 2160),
        ("2560 \u{00D7} 1080 (wide)", 2560, 1080), ("3440 \u{00D7} 1440 (wide)", 3440, 1440), ("3840 \u{00D7} 1080 (very wide)", 3840, 1080),
        ("1920 \u{00D7} 1200", 1920, 1200), ("1080 \u{00D7} 1920 (tall)", 1080, 1920),
    ]
    static var virtualSize: (width: Int, height: Int) {
        let parts = (Defaults.store.string(forKey: "virtualScreen") ?? "1920x1080").split(separator: "x").compactMap { Int($0) }
        return parts.count == 2 && parts[0] >= 640 && parts[1] >= 360 ? (parts[0], parts[1]) : (1920, 1080)
    }

    /// The virtual screen is let go of: the next time it is asked for it is made anew, at the
    /// size then chosen. What was on it is moved by macOS to another screen.
    static func forget() {
        lock.lock()
        virtual = nil
        lock.unlock()
    }

    private static let lock = NSLock()
    private static var virtual: SPVirtualDisplay?
    /// The virtual screen is there for the glasses alone (`leadIfAlone`), not one asked for.
    private static var leading = false

    /// **Locked, the glasses are the Mac's only screen**: the virtual screen made for them is let
    /// go of, to be made again when the room comes back (`leadIfAlone`). Made the main screen
    /// again only by arranging (`yieldToGlasses`, which knows the glasses by their EDID), the
    /// glasses came back from a night's display sleep naming no maker, and macOS, finding a
    /// second screen, put its "mirror or extend" question on them over the lock screen: nothing
    /// there to answer it with (littleotter, 2026-11-10). A virtual screen asked for in the menu,
    /// or one beside another display, is kept.
    static func letGoForLock() {
        lock.lock()
        let made = leading ? virtual : nil
        if made != nil {
            virtual = nil
            leading = false
        }
        lock.unlock()
        if made != nil { print("screens: locked; the virtual screen made for the glasses is let go of, the glasses the only screen") }
    }

    /// The number a screen goes by among windows: clear of any window's.
    static func windowID(for display: CGDirectDisplayID) -> CGWindowID { 0xD000_0000 | (display & 0x0FFF_FFFF) }

    private static func name(of display: CGDirectDisplayID) -> String {
        for screen in NSScreen.screens {
            if (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == display { return screen.localizedName }
        }
        return CGDisplayIsBuiltin(display) != 0 ? "Built-in display" : "Display \(display)"
    }

    /// The virtual display's number, once it has been made.
    static var virtualDisplay: CGDirectDisplayID? {
        lock.lock()
        defer { lock.unlock() }
        return virtual.map(\.displayID).flatMap { $0 == 0 ? nil : $0 }
    }

    /// Every screen there is to show: the Mac's own, but for `excluding` -- the glasses, which
    /// are what the room is shown on -- and the virtual one, made or not.
    static func list(excluding: CGDirectDisplayID?) -> [MacWindowInfo] {
        var count: UInt32 = 0
        var ids = [CGDirectDisplayID](repeating: 0, count: 16)
        CGGetActiveDisplayList(16, &ids, &count)
        let made = virtualDisplay
        var out: [MacWindowInfo] = []
        // The glasses by what they are as well as by number: changing to two eyes they come
        // back as another display, and for that moment were offered as a screen, "Air"
        // (2026-11-11: a Screen window of the room's own picture, 3840 wide).
        for display in ids.prefix(Int(count)) where display != excluding && display != made && !GlassesScreen.isGlasses(display) {
            out.append(MacWindowInfo(windowID: windowID(for: display), pid: 0, bundle: bundle, app: "Screen", title: name(of: display),
                                     frame: CGDisplayBounds(display), display: display))
        }
        let frame = made.map { CGDisplayBounds($0) } ?? CGRect(x: 0, y: 0, width: virtualSize.width, height: virtualSize.height)
        out.append(MacWindowInfo(windowID: windowID(for: virtualToBe), pid: 0, bundle: bundle, app: "Screen",
                                 title: made == nil ? "New virtual screen" : "Virtual screen", frame: frame, display: virtualToBe))
        return out
    }

    /// **A Mac with no screen but the glasses** (a Mac mini at a desk with nothing else on it):
    /// the virtual screen is made and becomes the main one, the glasses to its left. The menu
    /// bar, the Dock and every window the Mac opens are then on a screen nobody looks at but the room, which shows them as windows; the glasses show the room alone, and
    /// the pointer the room has hidden rests there rather than over the room's picture. For this
    /// login only, so a monitor plugged in later is not found arranged around a screen gone.
    /// Nothing is changed while any other display is there. Says whether it did it.
    @discardableResult
    static func leadIfAlone(besides glasses: CGDirectDisplayID) -> Bool {
        var count: UInt32 = 0
        var ids = [CGDirectDisplayID](repeating: 0, count: 16)
        guard CGGetOnlineDisplayList(16, &ids, &count) == .success else { return false }
        let made = virtualDisplay
        // The glasses by what they are, not by number: changing to two eyes they come back as
        // another display, and the one they were is still listed for a moment.
        func isGlasses(_ id: CGDirectDisplayID) -> Bool { GlassesScreen.isGlasses(id) }
        let others = ids.prefix(Int(count)).filter { $0 != glasses && $0 != made && !isGlasses($0) && CGDisplayMirrorsDisplay($0) != glasses }
        if !others.isEmpty { print("screens: \(others.count) other screen(s) here; the virtual one is not made the main one") }
        guard others.isEmpty, let virtual = resolve(virtualToBe) else { return false }
        lock.lock(); let screen = Self.virtual; lock.unlock()
        // **Arranged, then arranged again once it has its size.** A new virtual screen has no
        // size until the displays are arranged, and taking it then, macOS put the glasses back
        // at the top left as the main screen (an M6 Mac mini, 2026-11-09).
        for attempt in 1...3 {
            if CGMainDisplayID() == virtual, CGDisplayMirrorsDisplay(virtual) == kCGNullDirectDisplay,
               CGDisplayBounds(glasses).maxX <= CGDisplayBounds(virtual).minX {
                if attempt > 1 { print("screens: the glasses are this Mac's only screen; the virtual one is the main screen (\(attempt - 1) arrangement(s))") }
                lock.lock(); leading = true; lock.unlock()
                return true
            }
            var config: CGDisplayConfigRef?
            guard CGBeginDisplayConfiguration(&config) == .success else { return false }
            // **Not a mirror of the glasses.** Plugged in again, the glasses came back in two eyes
            // with the virtual screen mirroring them (2026-11-11, after macOS's question of what
            // to show on them was answered "an additional display" both times): one desktop,
            // 3840 wide, under the room. Each is told to be its own screen before it is placed.
            if CGDisplayMirrorsDisplay(virtual) != kCGNullDirectDisplay || CGDisplayMirrorsDisplay(glasses) != kCGNullDirectDisplay {
                if attempt == 1 { print("screens: the virtual screen and the glasses were mirrors of each other; parted") }
                CGConfigureDisplayMirrorOfDisplay(config, virtual, kCGNullDirectDisplay)
                CGConfigureDisplayMirrorOfDisplay(config, glasses, kCGNullDirectDisplay)
            }
            CGConfigureDisplayOrigin(config, virtual, 0, 0)
            // **The glasses to its left**: macOS draws its arrow to the right of where it points, so
            // a cursor stopped at the virtual screen's right edge, with the glasses there, had its
            // arrow drawn over the room (RoomTap.keepOffGlasses). Stopped at the left edge, the
            // arrow is on the virtual screen.
            CGConfigureDisplayOrigin(config, glasses, -Int32(CGDisplayBounds(glasses).width), 0)
            guard CGCompleteDisplayConfiguration(config, .forSession) == .success else { break }
            if let screen { settle(screen) }
            usleep(300_000)
        }
        print("screens: the virtual screen could not be made the main one")
        return false
    }

    /// The glasses the main screen again, the virtual one to their right, once they have gone back
    /// to one eye -- for macOS's lock screen, which is drawn on the main screen. Only where the
    /// virtual screen had taken it; tried for a few seconds, as the glasses' display comes back
    /// from the change of mode (arranged while changing, it did not come back: see `leadIfAlone`).
    static func yieldToGlasses(tries: Int = 0) {
        guard let made = virtualDisplay, CGMainDisplayID() == made else { return }
        func isGlasses(_ id: CGDirectDisplayID) -> Bool { GlassesScreen.isGlasses(id) }
        var count: UInt32 = 0
        var ids = [CGDirectDisplayID](repeating: 0, count: 16)
        CGGetActiveDisplayList(16, &ids, &count)
        let glasses = ids.prefix(Int(count)).first(where: isGlasses)
        // One eye, and seen so for a moment: the room has let them go and they are settling.
        guard let glasses, (CGDisplayCopyDisplayMode(glasses)?.pixelWidth ?? 0) < 3000, tries >= 3 else {
            if tries < 40 { DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { yieldToGlasses(tries: tries + 1) } }
            return
        }
        var config: CGDisplayConfigRef?
        guard CGBeginDisplayConfiguration(&config) == .success else { return }
        CGConfigureDisplayOrigin(config, glasses, 0, 0)
        CGConfigureDisplayOrigin(config, made, Int32(CGDisplayBounds(glasses).width), 0)
        let done = CGCompleteDisplayConfiguration(config, .forSession) == .success
        print("screens: the glasses are \(done ? "" : "not ")the main screen again")
    }

    /// Its sizes are not there to choose among the moment it exists, and left to itself macOS
    /// settles on the smallest: asked for the largest until it is that. On a Mac with no other
    /// screen it had no mode at all for three seconds and then came up at 896 by 504: waited
    /// for longer, and asked again once it is the main screen.
    private static func settle(_ made: SPVirtualDisplay) {
        for _ in 0..<50 {
            made.selectLargestMode()
            if Int(made.currentPixelSize.width) >= virtualSize.width { return }
            usleep(150_000)
        }
        print("screens: the virtual screen is \(Int(made.currentPixelSize.width))x\(Int(made.currentPixelSize.height)), not the \(virtualSize.width)x\(virtualSize.height) asked for")
    }

    /// The display a screen really is: itself, or the virtual one, made now if it has not been.
    static func resolve(_ display: CGDirectDisplayID) -> CGDirectDisplayID? {
        guard display == virtualToBe else { return display }
        lock.lock()
        defer { lock.unlock() }
        if let virtual, virtual.displayID != 0 { return virtual.displayID }
        guard let made = SPVirtualDisplay(width: UInt32(virtualSize.width), height: UInt32(virtualSize.height), hiDPI: false,
                                          serialNumber: UInt32(0x5350_0000 + (virtualSize.width << 4 ^ virtualSize.height) & 0xFFFF), name: "Spatiand") else {
            print("screens: a virtual screen could not be made")
            return nil
        }
        virtual = made
        settle(made)
        // A display that has just appeared is sometimes taken for a copy of another: it is its own.
        if SPVirtualDisplay.anyDisplayIsMirroring() { SPVirtualDisplay.disableAllMirroring() }
        print("screens: a virtual screen, display \(made.displayID), \(Int(made.currentPixelSize.width))x\(Int(made.currentPixelSize.height))")
        return made.displayID == 0 ? nil : made.displayID
    }
}
