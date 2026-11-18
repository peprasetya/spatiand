//  RoomBluetooth.swift — this Mac's Bluetooth devices, for the room's Bluetooth page.
//
//  The page is the Deck's (crates/spatiand/src/bluetooth.rs): the paired devices, whether each is
//  connected, connect, disconnect, forget, Bluetooth on and off. Where the Deck asks BlueZ, the
//  compositor asks here (SP_BLUETOOTH_LIST, SP_BLUETOOTH_DO) and waits for the answer
//  (sp_bluetooth_*); see crates/spatiand-mac/src/mac/bluetooth.rs. Adding a device is System
//  Settings' Bluetooth pane in the room, as the Deck's is its desktop's wizard.
//
//  Two things here are not IOBluetooth's public interface, and are named as such: forgetting a
//  device is its `remove` method, and switching Bluetooth on and off is
//  IOBluetoothPreferenceSetControllerPowerState -- what every tool that does either (blueutil
//  among them) uses, since macOS offers no public way. If a macOS drops them, the page says it
//  could not, and System Settings' pane still can.

import Foundation
import IOBluetooth
import CSpatiand

enum RoomBluetooth {
    /// Tell the compositor what there is: whether Bluetooth is on, then every paired device.
    static func say() {
        let powered: Int32
        if let controller = IOBluetoothHostController.default(), controller.addressAsString() != nil {
            powered = controller.powerState == kBluetoothHCIPowerStateON ? 1 : 0
        } else {
            powered = -1
        }
        sp_bluetooth_begin(powered)
        for case let device as IOBluetoothDevice in IOBluetoothDevice.pairedDevices() ?? [] {
            guard let address = device.addressString else { continue }
            sp_bluetooth_device(address, device.nameOrAddress ?? address, UInt32(device.deviceClassMajor),
                                UInt32(device.deviceClassMinor), device.isConnected())
        }
        sp_bluetooth_end()
    }

    /// Do what the page asked: `connect AA:BB:..`, `disconnect ..`, `forget ..`, `power on|off`.
    /// Answered with sp_bluetooth_done under `number`, once, whatever happens.
    static func act(_ text: String, number: UInt32) {
        let done = { (error: String?) in sp_bluetooth_done(number, error) }
        let parts = text.split(separator: " ", maxSplits: 1).map(String.init)
        guard parts.count == 2 else { done("Not understood: \(text)"); return }
        let (verb, argument) = (parts[0], parts[1])
        if verb == "power" {
            power(argument == "on", done)
            return
        }
        guard let device = IOBluetoothDevice(addressString: argument) else {
            done("No device \(argument).")
            return
        }
        switch verb {
        case "connect":
            Connecting.start(device, done)
        case "disconnect":
            // **Not on the main thread**: closeConnection waits for the device to let go, which
            // one out of reach takes seconds to do -- the room's pointer and keys with it.
            disconnecting.async {
                let status = device.closeConnection()
                done(status == kIOReturnSuccess ? nil : "It would not disconnect (\(status)).")
            }
        case "forget":
            // Not public: IOBluetoothDevice's own `remove`, which System Settings' Forget is.
            let remove = Selector(("remove"))
            if device.responds(to: remove) {
                device.perform(remove)
                done(nil)
            } else {
                done("This macOS does not let it be forgotten from here: use System Settings.")
            }
        default:
            done("Not understood: \(text)")
        }
    }

    fileprivate static let disconnecting = DispatchQueue(label: "spatiand.bluetooth.disconnect", qos: .userInitiated)

    /// Bluetooth on or off, through IOBluetooth's own preference call (not public; see the top).
    private static func power(_ on: Bool, _ done: @escaping (String?) -> Void) {
        typealias SetPower = @convention(c) (Int32) -> Void
        guard let framework = dlopen("/System/Library/Frameworks/IOBluetooth.framework/IOBluetooth", RTLD_LAZY),
              let symbol = dlsym(framework, "IOBluetoothPreferenceSetControllerPowerState") else {
            done("This macOS does not let Bluetooth be switched from here: use System Settings.")
            return
        }
        unsafeBitCast(symbol, to: SetPower.self)(on ? 1 : 0)
        // The controller takes a moment to come up or go down; the compositor judges by the list.
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { done(nil) }
    }

    /// A connect, which IOBluetooth finishes later on the main run loop: the answer is given
    /// then, or after the Deck's fifteen seconds if it never says.
    private final class Connecting: NSObject {
        private static var going: [Connecting] = []
        private var answered = false
        private let done: (String?) -> Void

        private init(_ done: @escaping (String?) -> Void) {
            self.done = done
        }

        static func start(_ device: IOBluetoothDevice, _ done: @escaping (String?) -> Void) {
            if device.isConnected() { done(nil); return }
            let connecting = Connecting(done)
            going.append(connecting)
            // A page timeout in slots of 0.625 ms: the most there is, about 41 s, which the
            // fifteen below cut short.
            let status = device.openConnection(connecting, withPageTimeout: BluetoothHCIPageTimeout(0xFFFF), authenticationRequired: false)
            if status != kIOReturnSuccess {
                connecting.answer("It did not connect (\(status)).")
                return
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 15) {
                // Given up on: the page still under way is called off, or the device would
                // connect later by itself, after the room said it had not.
                if !connecting.answered { RoomBluetooth.disconnecting.async { _ = device.closeConnection() } }
                connecting.answer("It did not answer.")
            }
        }

        @objc func connectionComplete(_ device: IOBluetoothDevice!, status: IOReturn) {
            answer(status == kIOReturnSuccess ? nil : "It did not connect (\(status)).")
        }

        private func answer(_ error: String?) {
            guard !answered else { return }
            answered = true
            Connecting.going.removeAll { $0 === self }
            done(error)
        }
    }
}
