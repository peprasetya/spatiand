package id.prasetya.spatiand;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.os.BatteryManager;

/**
 * The phone's battery, for the room's status pill ("17:28  82%"), as the Deck reads its own from
 * /sys/class/power_supply -- which an app on Android 12 and later may not read. Android tells
 * every receiver of ACTION_BATTERY_CHANGED at once (the intent is sticky) and again whenever the
 * level, the charger or the battery's state changes; each is handed to the room as it comes.
 *
 * Heard on the application's context for the life of the process, as the room runs: an activity
 * made again while the old one is still going cannot leave the pill without its battery.
 */
final class Battery {
    private Battery() {}

    private static final BroadcastReceiver changed = new BroadcastReceiver() {
        @Override
        public void onReceive(Context context, Intent intent) {
            hand(intent);
        }
    };

    private static boolean watching;

    /** Start telling the room, once; the battery as it is now goes at once. On the main thread. */
    static void watch(Context context) {
        if (watching) return;
        watching = true;
        // Only the system sends it (a protected broadcast), so being reachable costs nothing.
        hand(context.getApplicationContext().registerReceiver(changed,
                new IntentFilter(Intent.ACTION_BATTERY_CHANGED), Context.RECEIVER_EXPORTED));
    }

    private static void hand(Intent intent) {
        if (intent == null || !intent.getBooleanExtra(BatteryManager.EXTRA_PRESENT, true)) {
            Native.battery(0, 0, 0);
            return;
        }
        Native.battery(intent.getIntExtra(BatteryManager.EXTRA_LEVEL, -1),
                intent.getIntExtra(BatteryManager.EXTRA_SCALE, 0),
                intent.getIntExtra(BatteryManager.EXTRA_STATUS, BatteryManager.BATTERY_STATUS_UNKNOWN));
    }
}
