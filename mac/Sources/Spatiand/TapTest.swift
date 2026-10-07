//  TapTest.swift — `Spatiand --selftest-tap`
//
//  Whether an application's sound can be taken from the Mac's speakers: a short system sound is played over and over
//  by a program of its own (`afplay`), which is tapped, and what arrives is measured. The first time, macOS asks
//  whether Spatiand may take other applications' audio.

import AppKit
import AudioToolbox

enum TapTest {
    static func run() {
        guard #available(macOS 14.2, *) else { print("needs macOS 14.2"); exit(2) }
        var failures = 0
        func check(_ name: String, _ ok: Bool) {
            print((ok ? "ok    " : "FAIL  ") + name)
            if !ok { failures += 1 }
        }
        // Something to listen to, in one process that lasts: a few seconds of speech, made here and played once. (A loop of
        // short sounds is a new process each time, and a tap is made for the processes there were.)
        let speech = "/tmp/spatiand-tap-test.aiff"
        let say = Process()
        say.executableURL = URL(fileURLWithPath: "/usr/bin/say")
        say.arguments = ["-o", speech, "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen"]
        try? say.run()
        say.waitUntilExit()
        let player = Process()
        player.executableURL = URL(fileURLWithPath: "/usr/bin/afplay")
        player.arguments = ["-v", "0.5", speech]
        try? player.run()
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
            let tap = MacTap(bundle: "test.afplay", pid: player.processIdentifier)
            var samples = 0
            var peak: Float = 0
            let lock = NSLock()
            tap.onSound = { block, rate in
                lock.lock(); defer { lock.unlock() }
                samples += block.count / 2
                for v in block { peak = max(peak, abs(v)) }
                _ = rate
            }
            check("the player's audio process is found", !tap.processObjects().isEmpty)
            let started = tap.start()
            check("the tap starts (\(tap.lastError))", started)
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) {
                lock.lock()
                let (n, p) = (samples, peak)
                lock.unlock()
                print("note  the player was producing sound, as Core Audio sees it: \(tap.isPlaying())")
                check("sound arrives (\(n) frames, peak \(String(format: "%.3f", p)))", n > 20_000 && p > 0.001)
                tap.stop()
                // And through the whole path a Mac window's sound takes: tapped, placed by the room, handed to the engine.
                let placed = MacSound(room: Model.shared.room.core)
                let second = Process()
                second.executableURL = URL(fileURLWithPath: "/usr/bin/afplay")
                second.arguments = ["-v", "0.3", speech]
                try? second.run()
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
                    placed.reconcile(wanted: ["test.afplay": second.processIdentifier])
                    check("a tap is made for the application", placed.bundles == ["test.afplay"])
                    DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) {
                        check("its sound reaches the room's engine", AudioOut.shared.isSounding("test.afplay"))
                        placed.stopAll()
                        player.terminate()
                        second.terminate()
                        print(failures == 0 ? "all passed" : "\(failures) failed")
                        exit(failures == 0 ? 0 : 1)
                    }
                }
            }
        }
        NSApplication.shared.setActivationPolicy(.accessory)
        NSApplication.shared.run()
    }
}
