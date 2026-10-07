//  BannerTest.swift — `Spatiand --selftest-banners`
//
//  The Mac's notifications, taken for the glasses: a notification is made (`osascript`, which macOS shows as from Script
//  Editor), and the banner that macOS draws is looked for in the picture of the Notification Center window. Writes what
//  was found to /tmp/spatiand-banner.png. A Mac that is set not to show banners (Do Not Disturb, or the notification
//  settings of Script Editor) shows none, and the test says that and not that it failed.

import AppKit

enum BannerTest {
    static func run() {
        var failures = 0
        func check(_ name: String, _ ok: Bool) {
            print((ok ? "ok    " : "FAIL  ") + name)
            if !ok { failures += 1 }
        }
        NSApplication.shared.setActivationPolicy(.accessory)
        let banners = NotificationBanners()
        var latest: NotificationBanners.Shown?
        var seen = 0
        banners.onChange = { shown in latest = shown; if shown != nil { seen += 1 } }
        Task {
            await banners.start()
            await MainActor.run {
                check("the Notification Center window is being watched", banners.running)
                print("note  window \(banners.windowID) of pid \(banners.pid)")
                let make = Process()
                make.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
                make.arguments = ["-e", "display notification \"This is a test of Spatiand\" with title \"Spatiand\" subtitle \"notifications in the glasses\""]
                try? make.run()
                DispatchQueue.main.asyncAfter(deadline: .now() + 6) {
                    if let shown = latest ?? nil {
                        check("a banner was found (\(shown.width)x\(shown.height) pixels, \(Int(Double(shown.width) / Double(shown.scale))) points wide)", true)
                        check("and it is about as wide as a banner is", Double(shown.width) / Double(shown.scale) > 250 && Double(shown.width) / Double(shown.scale) < 440)
                        check("at the top right of the screen", shown.origin.y < 120)
                        // Written out as the picture it is.
                        let (w, h) = (shown.width, shown.height)
                        shown.pixels.withUnsafeBytes { bytes in
                            if let provider = CGDataProvider(data: Data(bytes) as CFData),
                               let image = CGImage(width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: w * 4, space: CGColorSpaceCreateDeviceRGB(),
                                                   bitmapInfo: CGBitmapInfo(rawValue: CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.premultipliedFirst.rawValue),
                                                   provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent) {
                                try? NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: "/tmp/spatiand-banner.png"))
                                print("wrote /tmp/spatiand-banner.png")
                            }
                        }
                    } else {
                        print("note  no banner appeared: this Mac may not be showing banners for Script Editor (Do Not Disturb, or its notification settings)")
                    }
                    banners.stop()
                    print(failures == 0 ? "all passed" : "\(failures) failed")
                    exit(failures == 0 ? 0 : 1)
                }
            }
        }
        NSApplication.shared.run()
    }
}
