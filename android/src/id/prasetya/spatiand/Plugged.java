package id.prasetya.spatiand;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbManager;
import android.util.Log;

/**
 * Plugging the glasses in opens Spatiand.
 *
 * The attach intent that would normally do it goes to Nebula, which the Beam Pro's framework
 * fixes as the handler for XREAL glasses whatever else is installed. But the framework also
 * broadcasts the attach first, to anyone listening, and this is listening. Starting an
 * activity from here is allowed because the app may draw over other apps, which it needs
 * anyway to be seen above XREAL's placeholder.
 */
public class Plugged extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        UsbDevice device = intent.getParcelableExtra(UsbManager.EXTRA_DEVICE, UsbDevice.class);
        if (device == null || device.getVendorId() != MainActivity.VENDOR) return;
        Log.i(MainActivity.TAG, "glasses plugged in; opening Spatiand");
        context.startActivity(new Intent(context, MainActivity.class)
                .setAction(UsbManager.ACTION_USB_DEVICE_ATTACHED)
                .putExtra(UsbManager.EXTRA_DEVICE, device)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_SINGLE_TOP));
    }
}
