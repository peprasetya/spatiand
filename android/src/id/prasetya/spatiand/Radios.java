package id.prasetya.spatiand;

import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothClass;
import android.bluetooth.BluetoothDevice;
import android.bluetooth.BluetoothManager;
import android.bluetooth.BluetoothProfile;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.os.SystemClock;
import android.util.Log;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * The phone's Bluetooth for the room's own Bluetooth page (crates/spatiand-android/src/android/
 * bluetooth.rs), and Android's settings pages as windows in the room.
 *
 * <p>What an ordinary app may do is done here: whether Bluetooth is on, the paired devices and
 * which are connected (once the wearer allows BLUETOOTH_CONNECT, asked on the phone's screen as
 * BluetoothBridge asks it). What Android keeps for the system -- connecting a device, letting go
 * of it, forgetting it, switching Bluetooth -- the shell helper does (ShellHelper.bluetooth).
 */
final class Radios {
    static final String TAG = "spatiand";

    /** The profiles a connection is looked for in, with their proxies once Android gives them. */
    private static final int[] PROFILES = {
        BluetoothProfile.HEADSET, BluetoothProfile.A2DP, 4 /* HID_HOST */, BluetoothProfile.LE_AUDIO,
        BluetoothProfile.HEARING_AID,
    };
    private static final BluetoothProfile[] proxies = new BluetoothProfile[PROFILES.length];
    private static boolean asked;
    private static long askedAt;

    private Radios() {}

    private static boolean permitted(Context c) {
        return c.checkSelfPermission(android.Manifest.permission.BLUETOOTH_CONNECT) == PackageManager.PERMISSION_GRANTED;
    }

    /** Ask the wearer for Bluetooth on the phone's screen, at most once in half a minute. */
    private static void ask() {
        long now = SystemClock.elapsedRealtime();
        if (askedAt != 0 && now - askedAt < 30_000) return;
        askedAt = now;
        BluetoothBridge.ask();
    }

    /** The proxies for the profiles, asked for once; each arrives when Android has it. */
    private static synchronized void watchProfiles(Context c, BluetoothAdapter adapter) {
        if (asked) return;
        asked = true;
        for (int i = 0; i < PROFILES.length; i++) {
            final int at = i;
            try {
                adapter.getProfileProxy(c, new BluetoothProfile.ServiceListener() {
                    @Override
                    public void onServiceConnected(int profile, BluetoothProfile proxy) {
                        synchronized (Radios.class) {
                            proxies[at] = proxy;
                        }
                    }

                    @Override
                    public void onServiceDisconnected(int profile) {
                        synchronized (Radios.class) {
                            proxies[at] = null;
                        }
                    }
                }, PROFILES[i]);
            } catch (Exception e) {
                Log.w(TAG, "bluetooth: no proxy for profile " + PROFILES[i] + ": " + e);
            }
        }
    }

    /** The addresses of the devices connected through any profile Android has a proxy for. */
    private static synchronized Set<String> connectedByProfile(Context c) {
        Set<String> found = new HashSet<>();
        for (BluetoothProfile p : proxies) {
            if (p == null) continue;
            try {
                for (BluetoothDevice d : p.getConnectedDevices()) found.add(d.getAddress());
            } catch (Exception e) {
                // That profile's service went away meanwhile.
            }
        }
        try {
            BluetoothManager m = c.getSystemService(BluetoothManager.class);
            for (BluetoothDevice d : m.getConnectedDevices(BluetoothProfile.GATT)) found.add(d.getAddress());
        } catch (Exception e) {
            // No GATT here.
        }
        return found;
    }

    /** Whether a device is connected, as Android itself says (hidden, not refused) or by profile. */
    private static boolean connected(BluetoothDevice d, Set<String> byProfile) {
        try {
            return (Boolean) BluetoothDevice.class.getMethod("isConnected").invoke(d);
        } catch (Exception hidden) {
            return byProfile.contains(d.getAddress());
        }
    }

    private static int battery(BluetoothDevice d) {
        try {
            return (Integer) BluetoothDevice.class.getMethod("getBatteryLevel").invoke(d);
        } catch (Exception hidden) {
            return -1;
        }
    }

    /** What kind of thing a device is, by the names BlueZ gives its icons, which the page reads. */
    static String kind(BluetoothClass c) {
        if (c == null) return "";
        int device = c.getDeviceClass();
        switch (c.getMajorDeviceClass()) {
            case BluetoothClass.Device.Major.PERIPHERAL:
                // Its minor class says keyboard (0x40), pointer (0x80), or both; joysticks and pads
                // in the low bits.
                if ((device & 0x40) != 0) return "input-keyboard";
                if ((device & 0x80) != 0) return "input-mouse";
                if ((device & 0x0C) != 0) return "input-gaming";
                return "input-tablet";
            case BluetoothClass.Device.Major.AUDIO_VIDEO:
                if (device == BluetoothClass.Device.AUDIO_VIDEO_HEADPHONES) return "audio-headphones";
                if (device == BluetoothClass.Device.AUDIO_VIDEO_LOUDSPEAKER
                        || device == BluetoothClass.Device.AUDIO_VIDEO_HIFI_AUDIO
                        || device == BluetoothClass.Device.AUDIO_VIDEO_PORTABLE_AUDIO
                        || device == BluetoothClass.Device.AUDIO_VIDEO_CAR_AUDIO) {
                    return "audio-card";
                }
                return "audio-headset";
            case BluetoothClass.Device.Major.PHONE:
                return "phone";
            case BluetoothClass.Device.Major.COMPUTER:
                return "computer";
            default:
                return "";
        }
    }

    /**
     * Bluetooth as the page shows it, as JSON: "powered" (true, false, or null for no adapter or
     * not allowed), "devices" (each "address", "name", "kind", "connected", "battery"), "note".
     */
    static String state(Context c) {
        try {
            JSONObject o = new JSONObject();
            BluetoothManager m = c.getSystemService(BluetoothManager.class);
            BluetoothAdapter adapter = m == null ? null : m.getAdapter();
            JSONArray devices = new JSONArray();
            o.put("devices", devices);
            if (adapter == null) {
                o.put("powered", JSONObject.NULL);
                o.put("note", "The phone has no Bluetooth.");
                return o.toString();
            }
            if (!permitted(c)) {
                ask();
                o.put("powered", JSONObject.NULL);
                o.put("note", "Allow Spatiand to use Bluetooth: answer on the phone's screen.");
                return o.toString();
            }
            o.put("powered", adapter.isEnabled());
            if (!adapter.isEnabled()) return o.toString();
            watchProfiles(c, adapter);
            Set<String> byProfile = connectedByProfile(c);
            for (BluetoothDevice d : adapter.getBondedDevices()) {
                String name = d.getAlias();
                if (name == null || name.isEmpty()) name = d.getName();
                devices.put(new JSONObject()
                        .put("address", d.getAddress())
                        .put("name", name == null ? "" : name)
                        .put("kind", kind(d.getBluetoothClass()))
                        .put("connected", connected(d, byProfile))
                        .put("battery", battery(d)));
            }
            return o.toString();
        } catch (Exception e) {
            Log.w(TAG, "bluetooth: " + e);
            return "{\"powered\":null,\"devices\":[],\"note\":\"Android's Bluetooth could not be asked.\"}";
        }
    }

    /** What Shell.bluetoothDo answers with no helper, and without the wearer's leave. */
    static final int HELPER_MISSING = -2, NOT_ALLOWED = -3;

    /** Connect, disconnect, forget ("connect", "disconnect", "forget") or switch ("on", "off"). */
    static int act(Context c, String op, String address) {
        if (!op.equals("on") && !op.equals("off") && !permitted(c)) {
            ask();
            return NOT_ALLOWED;
        }
        if (!Shell.ready()) return HELPER_MISSING;
        return Shell.bluetoothByHelper(op, address);
    }

    /**
     * The activity that answers one of Android's settings actions, as "package/class", for the
     * room to open it as one of the phone's apps; null if none does.
     */
    static String settingsComponent(Context c, String action) {
        PackageManager pm = c.getPackageManager();
        List<ResolveInfo> found = pm.queryIntentActivities(new Intent(action),
                PackageManager.ResolveInfoFlags.of(PackageManager.MATCH_DEFAULT_ONLY | PackageManager.MATCH_SYSTEM_ONLY));
        for (ResolveInfo r : found) {
            if (r.activityInfo != null) return r.activityInfo.packageName + "/" + r.activityInfo.name;
        }
        return null;
    }
}
