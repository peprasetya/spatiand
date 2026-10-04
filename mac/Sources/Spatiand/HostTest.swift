//  HostTest.swift — `Spatiand --selftest-host [bundle id]`
//
//  This Mac hosting itself: the host listens, the client half of the same app connects to it over
//  the loopback, launches an application, and the window that comes back is photographed. It
//  exercises the whole path -- ScreenCaptureKit, the encoder, the packets, the decoder -- with no
//  second machine.

import AppKit

enum HostTest {
    static func run(bundle: String) {
        setenv("SPATIAND_HOST_TRUST_SELF", "1", 1)
        let host = MacHost.shared
        host.start()
        guard let link = host.link else { print("host: could not start"); exit(1) }
        print("host: listening, fingerprint \(link.fingerprint.prefix(8))")

        let model = Model.shared
        model.onProblem = { print("problem: \($0)") }
        model.connect(PairedHost(name: "this Mac", address: "127.0.0.1:\(MacHost.port)", fingerprint: link.fingerprint))

        func after(_ seconds: Double, _ body: @escaping () -> Void) {
            DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: body)
        }
        after(3) {
            print("client: connected \(model.connected), \(model.apps.count) applications offered, e.g. \(model.apps.prefix(4).map(\.name))")
            if let app = model.apps.first(where: { $0.id == bundle }) {
                model.launch(app)
            } else {
                print("client: no such application: \(bundle)")
                exit(1)
            }
        }
        // For the sound test: something playing in the application's own name.
        if let command = ProcessInfo.processInfo.environment["SPATIAND_TEST_PLAY"] {
            after(5) { let p = Process(); p.executableURL = URL(fileURLWithPath: "/bin/sh"); p.arguments = ["-c", command]; try? p.run() }
        }
        // For the input test: click the window whose title has this in it, type "ok", and ask for a size.
        if let title = ProcessInfo.processInfo.environment["SPATIAND_TEST_TYPE"] {
            after(6) {
                guard let id = model.infos.first(where: { $0.value.title.contains(title) })?.key else { print("host input: no such window"); return }
                func say(_ input: Any) { model.link.say(["InputAt": ["window": Int(id), "input": input, "time_ms": Int(ProcessInfo.processInfo.systemUptime * 1000)]]) }
                model.link.say(["Focus": ["window": Int(id)]])
                say(["Motion": ["x": 200.0, "y": 200.0]])
                say(["Button": ["button": 0x110, "pressed": true]]); say(["Button": ["button": 0x110, "pressed": false]])
                for code in [24, 37] { say(["Key": ["code": code, "pressed": true]]); say(["Key": ["code": code, "pressed": false]]) }   // o, k
                model.link.say(["Configure": ["window": Int(id), "width": 1700, "height": 1000]])
            }
        }
        after(11) {
            print("client: \(model.windows.count) windows")
            for (id, surface) in model.windows {
                guard let number = surface.view.window?.windowNumber,
                      let image = CGWindowListCreateImage(.null, .optionIncludingWindow, CGWindowID(number), [.boundsIgnoreFraming]) else { continue }
                let rep = NSBitmapImageRep(cgImage: image)
                let path = "/tmp/spatiand-host-test-\(id).png"
                try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
                print("wrote \(path): \(image.width)x\(image.height), \(surface.view.pictures) pictures shown")
            }
            if ProcessInfo.processInfo.environment["SPATIAND_TEST_TYPE"] != nil {
                Task {
                    let list = await MacWindows.list()
                    if let info = list.first(where: { $0.title.contains(ProcessInfo.processInfo.environment["SPATIAND_TEST_TYPE"]!) }) {
                        print("host input: text now \(MacWindows.text(info) ?? "?"), size \(MacWindows.currentFrame(info).map { "\(Int($0.width))x\(Int($0.height))" } ?? "?")")
                    }
                    exit(0)
                }
                return
            }
            print("client: \(model.audioHeard) bytes of sound heard")
            exit(0)
        }
    }
}
