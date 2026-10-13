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
    static let virtualSize = (width: 1920, height: 1080)

    private static let lock = NSLock()
    private static var virtual: SPVirtualDisplay?

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
        for display in ids.prefix(Int(count)) where display != excluding && display != made {
            out.append(MacWindowInfo(windowID: windowID(for: display), pid: 0, bundle: bundle, app: "Screen", title: name(of: display),
                                     frame: CGDisplayBounds(display), display: display))
        }
        let frame = made.map { CGDisplayBounds($0) } ?? CGRect(x: 0, y: 0, width: virtualSize.width, height: virtualSize.height)
        out.append(MacWindowInfo(windowID: windowID(for: virtualToBe), pid: 0, bundle: bundle, app: "Screen",
                                 title: made == nil ? "New virtual screen" : "Virtual screen", frame: frame, display: virtualToBe))
        return out
    }

    /// The display a screen really is: itself, or the virtual one, made now if it has not been.
    static func resolve(_ display: CGDirectDisplayID) -> CGDirectDisplayID? {
        guard display == virtualToBe else { return display }
        lock.lock()
        defer { lock.unlock() }
        if let virtual, virtual.displayID != 0 { return virtual.displayID }
        guard let made = SPVirtualDisplay(width: UInt32(virtualSize.width), height: UInt32(virtualSize.height), hiDPI: false,
                                          serialNumber: 0x5350_0001, name: "Spatiand") else {
            print("screens: a virtual screen could not be made")
            return nil
        }
        virtual = made
        // Its sizes are not there to choose among the moment it exists, and left to itself
        // macOS settles on the smallest: asked for the largest until it is that.
        for _ in 0..<20 {
            usleep(150_000)
            made.selectLargestMode()
            if Int(made.currentPixelSize.width) >= virtualSize.width { break }
        }
        // A display that has just appeared is sometimes taken for a copy of another: it is its own.
        if SPVirtualDisplay.anyDisplayIsMirroring() { SPVirtualDisplay.disableAllMirroring() }
        print("screens: a virtual screen, display \(made.displayID), \(Int(made.currentPixelSize.width))x\(Int(made.currentPixelSize.height))")
        return made.displayID == 0 ? nil : made.displayID
    }
}
