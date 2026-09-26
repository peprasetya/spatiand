package id.prasetya.spatiand;

import android.hardware.usb.UsbDevice;
import android.os.Bundle;
import android.os.IBinder;
import android.os.UserHandle;

import java.lang.reflect.Method;

/**
 * What only the shell may do, done once, from adb: see android/setup.sh.
 *
 *   CLASSPATH=<spatiand.apk> app_process / id.prasetya.spatiand.Setup usb <uid>
 *
 * Gives the app a *persistent* permission for every pair of XREAL glasses plugged in, which
 * survives unplugging and reboots. It is the only way to one on a Beam Pro: its framework
 * hands XREAL glasses to Nebula as a fixed handler, so Android never asks which app should
 * have them and there is no "Always" to tick -- and with Nebula disabled, nobody is asked at
 * all. docs/beam-pro.md.
 */
public final class Setup {
    static final int XREAL_VENDOR = 0x3318;

    public static void main(String[] args) throws Exception {
        if (args.length != 2 || !args[0].equals("usb")) {
            System.err.println("usage: Setup usb <uid>");
            System.exit(2);
        }
        int uid = Integer.parseInt(args[1]);

        IBinder binder = (IBinder) Class.forName("android.os.ServiceManager")
                .getMethod("getService", String.class).invoke(null, "usb");
        Object usb = Class.forName("android.hardware.usb.IUsbManager$Stub")
                .getMethod("asInterface", IBinder.class).invoke(null, binder);
        Bundle devices = new Bundle();
        usb.getClass().getMethod("getDeviceList", Bundle.class).invoke(usb, devices);
        Method persist = usb.getClass().getMethod("setDevicePersistentPermission",
                UsbDevice.class, int.class, UserHandle.class, boolean.class);
        UserHandle user = UserHandle.getUserHandleForUid(uid);

        int granted = 0;
        for (String key : devices.keySet()) {
            UsbDevice device = devices.getParcelable(key, UsbDevice.class);
            if (device == null || device.getVendorId() != XREAL_VENDOR) continue;
            persist.invoke(usb, device, uid, user, true);
            System.out.println("granted " + device.getProductName() + " ("
                    + Integer.toHexString(device.getVendorId()) + ":"
                    + Integer.toHexString(device.getProductId()) + ") to uid " + uid + ", persistently");
            granted++;
        }
        if (granted == 0) {
            System.err.println("no XREAL glasses plugged in: plug them in and run this again");
            System.exit(1);
        }
    }
}
