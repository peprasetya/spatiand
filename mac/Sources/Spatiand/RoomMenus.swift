//  RoomMenus.swift — an application's own menus, as a list in the room.
//
//  A Mac application's menus are not in its window: they are on the Mac's menu bar, which is on
//  the Mac's screen and out of sight in the glasses. So a Mac window in the room has a menu
//  button on its title bar, and what it opens is the same menus read through Accessibility and
//  shown as one of the room's own lists -- File, Edit, View and the rest, then what is in the
//  one chosen, a level at a time -- and what is chosen there is pressed in the real menu.
//
//  Read and pressed, never drawn by the Mac: nothing drops down on the Mac's screen, and it does
//  not matter which application the Mac has in front.
//
//  The same for a Mac window streamed to a Deck, a Beam Pro or another Mac: this Mac as a host
//  (`MacHost`) has a reader of its own whose levels go over the link (`HostMessage::Menu`), and
//  the room there shows the same list (`remote::menu` in the compositor).

import AppKit
import ApplicationServices
import CSpatiand

final class RoomMenus {
    /// The Mac's own room's: what it reads is said to the compositor here.
    static let shared = RoomMenus(say: { title, rows, _ in
        sp_menu_begin(title)
        for row in rows { sp_menu_row(row.label, row.detail) }
        sp_menu_end()
    }, done: { sp_menu_close() })

    /// A level read, said: its title and each row's words and what is said under them.
    typealias Say = (_ title: String, _ rows: [(label: String, detail: String)], _ level: UInt32) -> Void
    private let sayLevel: Say
    /// The menu is done with.
    private let sayDone: () -> Void

    init(say: @escaping Say, done: @escaping () -> Void) {
        sayLevel = say
        sayDone = done
    }

    /// One row of the level on show: the real item, and whether choosing it opens another level.
    private struct Row {
        let element: AXUIElement
        let opens: Bool
    }

    private var application = ""
    /// The number of the level last said: new for each, so that a choice made in a level since
    /// left (a session's second press before the next level reached it) is known and passed over.
    private var level: UInt32 = 0
    /// The levels gone down through: what each is called, and its rows.
    private var levels: [(title: String, rows: [Row])] = []
    /// Said when something was chosen that may have opened a window.
    var onChosen: (() -> Void)?

    private static func value<T>(_ element: AXUIElement, _ attribute: String) -> T? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success else { return nil }
        return value as? T
    }

    private static func children(_ element: AXUIElement) -> [AXUIElement] { value(element, kAXChildrenAttribute as String) ?? [] }

    /// What is in a menu: the items of the menu an item of the bar, or an item with more under it, holds.
    private static func items(under element: AXUIElement) -> [AXUIElement] {
        children(element).flatMap { child -> [AXUIElement] in
            let role: String = value(child, kAXRoleAttribute as String) ?? ""
            return role == (kAXMenuRole as String) ? children(child) : []
        }
    }

    /// The keys that do the same, as the menu shows them.
    private static func shortcut(_ item: AXUIElement) -> String {
        guard let key: String = value(item, kAXMenuItemCmdCharAttribute as String), !key.isEmpty else { return "" }
        let modifiers: Int = value(item, kAXMenuItemCmdModifiersAttribute as String) ?? 0
        var out = ""
        if modifiers & 4 != 0 { out += "\u{2303}" }
        if modifiers & 2 != 0 { out += "\u{2325}" }
        if modifiers & 1 != 0 { out += "\u{21E7}" }
        if modifiers & 8 == 0 { out += "\u{2318}" }
        return out + key
    }

    /// Say the level on top to the room.
    private func say() {
        guard let level = levels.last else { sayDone(); return }
        var said: [(label: String, detail: String)] = []
        for row in level.rows {
            let title: String = Self.value(row.element, kAXTitleAttribute as String) ?? ""
            let enabled: Bool = Self.value(row.element, kAXEnabledAttribute as String) ?? true
            let mark: String = Self.value(row.element, kAXMenuItemMarkCharAttribute as String) ?? ""
            let keys = Self.shortcut(row.element)
            let label = (mark.isEmpty ? "" : "\u{2713} ") + title + (row.opens ? "  \u{203A}" : "") + (keys.isEmpty ? "" : "    " + keys)
            said.append((label, !enabled ? "Not available now" : row.opens ? "Opens more" : keys.isEmpty ? "" : "Also \(keys) on the keyboard"))
        }
        self.level &+= 1
        sayLevel(level.title, said, self.level)
    }

    private func rows(_ items: [AXUIElement]) -> [Row] {
        items.compactMap { item in
            // A line between groups has no words, and is nothing to choose.
            let title: String = Self.value(item, kAXTitleAttribute as String) ?? ""
            guard !title.isEmpty else { return nil }
            return Row(element: item, opens: !Self.items(under: item).isEmpty)
        }
    }

    /// The menus of an application, from the top. Not on the main thread: an application answers when it likes.
    func open(pid: pid_t, name: String) {
        let app = AXUIElementCreateApplication(pid)
        AXUIElementSetMessagingTimeout(app, 1.0)
        var bar: CFTypeRef?
        guard AXUIElementCopyAttributeValue(app, kAXMenuBarAttribute as CFString, &bar) == .success, let bar else {
            print("menus: \(name) would not say what its menus are")
            sayDone()
            return
        }
        application = name
        // The bar's own items are levels to open, every one. The first is the Apple menu, which
        // is the Mac's and not the application's: left out.
        let top = Self.children(bar as! AXUIElement).dropFirst().compactMap { item -> Row? in
            let title: String = Self.value(item, kAXTitleAttribute as String) ?? ""
            return title.isEmpty ? nil : Row(element: item, opens: true)
        }
        levels = [(name, Array(top))]
        say()
    }

    /// Row `index` of the level on show; `in` the number of the level it was chosen in, where the
    /// one who chose says it (a session; an older one does not).
    func choose(_ index: Int, in chosenIn: UInt32? = nil) {
        if let chosenIn, chosenIn != self.level {
            print("menus: a choice made in a level since left, passed over")
            return
        }
        guard let level = levels.last, index >= 0, index < level.rows.count else { sayDone(); return }
        let row = level.rows[index]
        if row.opens {
            let title: String = Self.value(row.element, kAXTitleAttribute as String) ?? ""
            levels.append((levels.count == 1 ? "\(application) \u{2014} \(title)" : title, rows(Self.items(under: row.element))))
            say()
            return
        }
        guard Self.value(row.element, kAXEnabledAttribute as String) ?? true else { return }
        let result = AXUIElementPerformAction(row.element, kAXPressAction as CFString)
        if result != .success { print("menus: \(application) would not do that (\(result.rawValue))") }
        levels = []
        sayDone()
        onChosen?()
    }

    func back() {
        if !levels.isEmpty { levels.removeLast() }
        say()
    }
}
