//  WindowTest.swift — `Spatiand --selftest-window <address> <fingerprint> <app>`
//
//  Opens the app's window on this Mac for real, drives it through the same path a mouse would
//  (a click on the host's "Network" page button), and photographs the window itself -- not the
//  screen, which this process may not be allowed to read -- before and after. Exits when done.

import AppKit
import CoreGraphics

enum WindowTest {
    static func run(address: String, fingerprint: String, app: String) {
        let model = Model.shared
        let host = PairedHost(name: "test", address: address, fingerprint: fingerprint)
        model.onProblem = { print("problem: \($0)") }
        model.connect(host)

        func after(_ seconds: Double, _ body: @escaping () -> Void) {
            DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: body)
        }
        after(2) { if let a = model.apps.first(where: { $0.id == app }) { model.launch(a) } else { print("no such app: \(model.apps.map(\.id))") } }
        after(4) {
            // Mac to host: the host's log says whether it took the offer.
            model.link.say(["Clipboard": ["Offer": ["mime_types": ["text/plain;charset=utf-8"], "text": "mac-to-host-clipboard-test", "bytes": 26]]])
        }
        after(6) { snapshot("/tmp/spatiand-window-1.png", model) }
        if ProcessInfo.processInfo.environment["SPATIAND_TEST_RESIZE"] != nil {
            // Drag the frame bigger, the way a person does, and say what each end then believes.
            after(7) {
                guard let w = model.windows.values.compactMap({ $0 as? RemoteWindow }).max(by: { $0.id < $1.id }) else { return }
                print("before: view \(w.view.bounds.size) host \(w.view.hostSize)")
                // A drag: many small steps, a tenth of a second or less apart.
                for step in 1...12 {
                    DispatchQueue.main.asyncAfter(deadline: .now() + Double(step) * 0.08) {
                        var frame = w.window.frame
                        frame.size.width += 25; frame.size.height += 15
                        w.window.setFrame(frame, display: true)
                    }
                }
            }
            after(11) {
                guard let w = model.windows.values.compactMap({ $0 as? RemoteWindow }).max(by: { $0.id < $1.id }) else { return }
                print("after: view \(w.view.bounds.size) host \(w.view.hostSize)")
                snapshot("/tmp/spatiand-window-resized.png", model)
                exit(0)
            }
            return
        }
        after(7) {
            // The "Network" button of the host's settings, in the picture's own pixels.
            guard let w = model.windows.values.compactMap({ $0 as? RemoteWindow }).max(by: { $0.id < $1.id }) else { return }
            func now() -> Int { Int(ProcessInfo.processInfo.systemUptime * 1000) }
            func say(_ input: Any) { w.view.send?(["InputAt": ["window": Int(w.id), "input": input, "time_ms": now()]]) }
            // The settings page's Network button; in a terminal, a right click where text would be.
            let button = 0x110
            if app == "qterminal" || ProcessInfo.processInfo.environment["SPATIAND_TEST_KEYS"] != nil {
                // The keyboard: "ls" and Return, as evdev codes, the way VideoView sends them.
                // This process is not the frontmost app, so the window loses key status at once and
                // would tell the host nobody has the keyboard. The test says who has it instead.
                w.onFocus = nil
                w.view.send?(["Focus": ["window": Int(w.id)]])
                var delay = 0.4
                // "ls" and Return in a terminal; Down three times and Return in the settings app.
                let codes = app == "qterminal" ? [38, 31, 28] : [108, 108, 108, 28]
                for code in codes {
                    after(delay) { say(["Key": ["code": code, "pressed": true]]) }
                    after(delay + 0.08) { say(["Key": ["code": code, "pressed": false]]) }
                    delay += 0.25
                }
                return
            }
            // The settings page's Network button.
            // Keyboard focus first, as a window the owner had clicked would have.
            w.view.send?(["Focus": ["window": Int(w.id)]])
            say(["Motion": app == "qterminal" ? ["x": 32.0, "y": 13.0] : ["x": 80.0, "y": 316.0]])
            after(0.3) {
                say(["Button": ["button": button, "pressed": true]])
                after(0.15) { say(["Button": ["button": button, "pressed": false]]) }
            }
        }
        after(10.5) {
            snapshot("/tmp/spatiand-window-2.png", model)
            // A menu, if the click opened one: its own panel, hung off the window.
            for (id, surface) in model.windows where surface is RemotePopup {
                if let number = surface.view.window?.windowNumber,
                   let image = CGWindowListCreateImage(.null, .optionIncludingWindow, CGWindowID(number), [.boundsIgnoreFraming]) {
                    let rep = NSBitmapImageRep(cgImage: image)
                    try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: "/tmp/spatiand-popup-\(id).png"))
                    print("popup \(id): \(image.width)x\(image.height)")
                }
            }
            print("surfaces: \(model.windows.count)")
            // `SPATIAND_TEST_QUIT=1` ends the application again, so a test of a heavy one leaves
            // the host as it found it.
            if ProcessInfo.processInfo.environment["SPATIAND_TEST_QUIT"] != nil {
                model.link.say(["ForceQuit": ["app": app]])
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { exit(0) }
            } else {
                exit(0)
            }
        }
    }

    /// Every window, named by its host id, so a session that already had windows open is no
    /// surprise.
    private static func snapshot(_ path: String, _ model: Model) {
        for (id, surface) in model.windows {
            guard let number = surface.view.window?.windowNumber,
                  let image = CGWindowListCreateImage(.null, .optionIncludingWindow, CGWindowID(number), [.boundsIgnoreFraming])
            else { continue }
            let rep = NSBitmapImageRep(cgImage: image)
            let name = path.replacingOccurrences(of: ".png", with: "-id\(id).png")
            try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: name))
            print("wrote \(name): \(image.width)x\(image.height)\(surface is RemotePopup ? " (popup)" : "")")
        }
    }
}
