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
        after(6) { snapshot("/tmp/spatiand-window-1.png", model) }
        after(7) {
            // The "Network" button of the host's settings, in the picture's own pixels.
            guard let w = model.windows.values.first else { return }
            func now() -> Int { Int(ProcessInfo.processInfo.systemUptime * 1000) }
            func say(_ input: Any) { w.view.send?(["InputAt": ["window": Int(w.id), "input": input, "time_ms": now()]]) }
            say(["Motion": ["x": 80.0, "y": 316.0]])
            after(0.3) {
                say(["Button": ["button": 0x110, "pressed": true]])
                after(0.15) { say(["Button": ["button": 0x110, "pressed": false]]) }
            }
        }
        after(10.5) { snapshot("/tmp/spatiand-window-2.png", model); exit(0) }
    }

    private static func snapshot(_ path: String, _ model: Model) {
        guard let w = model.windows.values.first else { print("no window to photograph"); return }
        let id = CGWindowID(w.window.windowNumber)
        guard let image = CGWindowListCreateImage(.null, .optionIncludingWindow, id, [.boundsIgnoreFraming]) else {
            print("could not capture the window"); return
        }
        let rep = NSBitmapImageRep(cgImage: image)
        try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
        print("wrote \(path): \(image.width)x\(image.height)")
    }
}
