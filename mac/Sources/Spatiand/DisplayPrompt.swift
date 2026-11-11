//  DisplayPrompt.swift — macOS's question about a display just plugged in, answered for the glasses.
//
//  macOS 27 asks, of every display it has not been told about in this session, "What do you want
//  to show on “Air”?": everything (mirror), one window, or an additional display. Until that is
//  answered the display does not exist for anyone else: it is in no CoreGraphics list. The question
//  is put on the Mac's own screen, and with the glasses on a Mac mini that is Spatiand's virtual
//  screen, which nobody sees. So the wearer had blank glasses and a room saying "plug them in
//  again", which only brought the question back; and it is asked twice, since the glasses come
//  back as another display when the room switches them to two eyes (littleotter, 2026-11-11).
//
//  **A workaround, and named as one**: the question is answered here by pressing its buttons
//  through Accessibility, as a person would ("an additional display", then the confirming button).
//  The proper way would be a setting or a call that tells macOS beforehand what a known display is
//  for; none was found. It depends on the shape of Apple's panel (three choices, a checkbox, two
//  buttons), is checked against that shape before anything is pressed, and when the shape is not
//  that, or Accessibility is not allowed, the room says aloud what is being asked instead.

import AppKit
import ApplicationServices
import IOKit

enum DisplayPrompt {
    /// The process that puts the question up, if it has a window on a screen now.
    private static func asker() -> pid_t? {
        guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else { return nil }
        for window in list {
            guard let owner = window[kCGWindowOwnerName as String] as? String,
                  owner.replacingOccurrences(of: " ", with: "") == "ControlCenterHelper",
                  let pid = window[kCGWindowOwnerPID as String] as? Int else { continue }
            return pid_t(pid)
        }
        return nil
    }

    /// Whether macOS has a question up at all (known without Accessibility).
    static var isUp: Bool { asker() != nil }

    /// The name macOS has for the headset's display, from the framebuffer that holds its EDID.
    private static func headsetName() -> String? {
        var iterator: io_iterator_t = 0
        guard IOServiceGetMatchingServices(kIOMainPortDefault, IOServiceMatching("IOMobileFramebuffer"), &iterator) == KERN_SUCCESS else { return nil }
        defer { IOObjectRelease(iterator) }
        while case let entry = IOIteratorNext(iterator), entry != 0 {
            defer { IOObjectRelease(entry) }
            guard let value = IORegistryEntryCreateCFProperty(entry, "DisplayAttributes" as CFString, kCFAllocatorDefault, 0)?.takeRetainedValue(),
                  let attributes = value as? [String: Any],
                  let product = attributes["ProductAttributes"] as? [String: Any],
                  let vendor = product["LegacyManufacturerID"] as? Int, let model = product["ProductID"] as? Int,
                  Headsets.isHeadsetDisplay(vendor: UInt32(vendor), product: UInt32(model)),
                  let name = product["ProductName"] as? String, !name.isEmpty else { continue }
            return name
        }
        return nil
    }

    private static func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
        var value: CFTypeRef?
        return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
    }

    private static func children(_ element: AXUIElement) -> [AXUIElement] {
        (attribute(element, kAXChildrenAttribute as String) as? [AXUIElement]) ?? []
    }

    private static func role(_ element: AXUIElement) -> String {
        (attribute(element, kAXRoleAttribute as String) as? String) ?? ""
    }

    private static func width(_ element: AXUIElement) -> CGFloat {
        guard let value = attribute(element, kAXSizeAttribute as String), CFGetTypeID(value) == AXValueGetTypeID() else { return 0 }
        var size = CGSize.zero
        AXValueGetValue(value as! AXValue, .cgSize, &size)
        return size.width
    }

    /// The element whose own children are the question: words naming the headset, the choices and
    /// the two buttons.
    private static func panel(under element: AXUIElement, naming name: String, depth: Int = 0) -> [AXUIElement]? {
        let own = children(element)
        let words = own.filter { role($0) == (kAXStaticTextRole as String) }.compactMap { attribute($0, kAXValueAttribute as String) as? String }
        let buttons = own.filter { role($0) == (kAXButtonRole as String) }
        if buttons.count == 5, words.contains(where: { $0.contains(name) }) { return buttons }
        guard depth < 6 else { return nil }
        for child in own {
            if let found = panel(under: child, naming: name, depth: depth + 1) { return found }
        }
        return nil
    }

    private static var lastTry = Date.distantPast
    private static var refusedSaid = false

    /// If macOS is asking what to show on the headset: "an additional display". True when the
    /// buttons were pressed; the display then comes as any does.
    @discardableResult
    static func answerForHeadset() -> Bool {
        guard Date().timeIntervalSince(lastTry) > 1.5 else { return false }
        guard let pid = asker() else { return false }
        lastTry = Date()
        guard AXIsProcessTrusted() else {
            if !refusedSaid {
                refusedSaid = true
                print("glasses: macOS is asking what to show on a display; not answered, Accessibility is not allowed")
            }
            return false
        }
        guard let name = headsetName() else { return false }
        let app = AXUIElementCreateApplication(pid)
        guard let windows = attribute(app, kAXWindowsAttribute as String) as? [AXUIElement] else { return false }
        for window in windows {
            guard let buttons = panel(under: window, naming: name) else { continue }
            // The three choices in their order (everything, one window, an additional display),
            // then the two that close it: the wider of those confirms.
            let extend = buttons[2]
            let confirm = width(buttons[4]) >= width(buttons[3]) ? buttons[4] : buttons[3]
            guard AXUIElementPerformAction(extend, kAXPressAction as CFString) == .success else { return false }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) {
                let done = AXUIElementPerformAction(confirm, kAXPressAction as CFString) == .success
                print("glasses: macOS asked what to show on “\(name)”; answered for the wearer: an additional display\(done ? "" : " (the confirming button did not take)")")
            }
            return true
        }
        if !refusedSaid {
            refusedSaid = true
            print("glasses: macOS is asking something, but not in the shape known here; left alone")
        }
        return false
    }
}
