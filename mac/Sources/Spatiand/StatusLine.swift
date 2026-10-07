//  StatusLine.swift — the time, the battery and how many windows, in the corner of the view.
//
//  The Deck's status bar: its words are written by the same code (`spatiand_room::status`); a Mac only says what the
//  battery is, which the power-sources service knows. Held to the head in the upper left by the room.

import CSpatiand
import Foundation
import IOKit.ps

enum StatusLine {
    /// The charge in percent and whether power is going in, or nil for a Mac without a battery.
    static func battery() -> (percent: Int, charging: Bool)? {
        guard let info = IOPSCopyPowerSourcesInfo()?.takeRetainedValue(),
              let list = IOPSCopyPowerSourcesList(info)?.takeRetainedValue() as? [CFTypeRef] else { return nil }
        for source in list {
            guard let d = IOPSGetPowerSourceDescription(info, source)?.takeUnretainedValue() as? [String: Any],
                  (d[kIOPSTypeKey] as? String) == kIOPSInternalBatteryType,
                  let current = d[kIOPSCurrentCapacityKey] as? Int, let max = d[kIOPSMaxCapacityKey] as? Int, max > 0 else { continue }
            // Plugged in and full counts as charging, as on the Deck: the question is whether it is going down.
            let onPower = (d[kIOPSPowerSourceStateKey] as? String) == kIOPSACPowerValue
            return (min(100, current * 100 / max), (d[kIOPSIsChargingKey] as? Bool) == true || onPower)
        }
        return nil
    }

    /// The line, as the Deck would write it.
    static func text(windows: Int) -> String {
        let b = battery()
        guard let c = sp_status_line(UInt32(max(0, windows)), Int32(b?.percent ?? -1), (b?.charging ?? false) ? 1 : 0) else { return "" }
        defer { sp_free_string(c) }
        return String(cString: c)
    }
}
