package id.prasetya.spatiand;

import android.view.Surface;

/** libspatiand.so: crates/spatiand-android. Every call returns quickly. */
final class Native {
    static {
        // First: it is what Smithay's dlopen("libEGL.so.1") finds. See android/eglshim.
        System.loadLibrary("eglshim");
        System.loadLibrary("spatiand");
    }

    private Native() {}

    /** Where the session keeps things, as its XDG directories. Call first. */
    static native void configDir(String files, String cache);

    /** Where the keyboard layouts were unpacked. Before begin(). */
    static native void xkbDir(String path);

    /** Where recordings are written before they are published. Before begin(). */
    static native void recordingsDir(String path);

    /** A finished recording's path, to be moved to Movies/Spatiand; null for none. */
    static native String takeRecording();

    /** Start the session, once; it runs for the life of the process. */
    static native void begin();

    /** Take the glasses through a UsbDeviceConnection's descriptor, which is duplicated. */
    static native boolean start(int usbFd);

    /** Let go of the glasses, putting them back in 2D. */
    static native void stop();

    /**
     * Whether the glasses' display is the stand-in tools/beam-stand-in makes: the room then draws
     * its world there with a headset that is not there, and leaves the hosts alone.
     */
    static native void standIn(boolean on);

    /** Draw the room into the glasses' window. */
    static native void surface(Surface surface);

    /** The glasses' window is going; returns once nothing draws into it. */
    static native void surfaceGone();

    /** Draw the monitors into the phone's touch area. */
    static native void phoneSurface(Surface surface);

    static native void phoneSurfaceGone();

    /** A touch on the touch area: action 0 down, 1 move, 2 up, 3 cancel; x and y 0..1. */
    static native void touch(int action, int id, float x, float y);

    /** The phone's orientation, from its game rotation vector. */
    static native void rotation(float x, float y, float z, float w);

    /** 0 settings (STEAM), 1 the launcher (⋯), 2 back (B). */
    static native void button(int which, boolean down);

    /** A key, by KeyEvent code. Returns whether the session takes it. */
    static native boolean key(int code, boolean down);

    /** Text Android's keyboard committed. */
    static native void text(String text);

    /** Recentre the room, and aim the phone where the head faces. */
    static native void recenter();

    /** Play the room's sound to this audio device; 0 for wherever Android routes it. */
    static native void audioOutput(int deviceId);

    /** Record the microphone sent to hosts from this audio device; 0 for Android's choice. */
    static native void audioInput(int deviceId);

    /** A gamepad's button, by KeyEvent code. Returns whether it was one. */
    static native boolean padButton(int device, int code, boolean down);

    /** A gamepad's sticks (+y down), triggers and hat. */
    static native void padAxes(int device, float lx, float ly, float rx, float ry,
            float lt, float rt, float hatX, float hatY);

    /** A gamepad's touchpad, 0..1 from its top-left. */
    static native void padTouch(int device, float x, float y, boolean touched, boolean clicked);

    /** A gamepad's gyro, radians a second. */
    static native void padGyro(int device, float x, float y, float z);

    static native void padGone(int device);

    /** A mouse or a keyboard's trackpad: travel in counts (+y down), MotionEvent button bits,
     *  and the wheel in notches. */
    static native void mouse(float dx, float dy, int buttons, float wheelUp, float wheelRight);

    /** What a game asked the pad's motors to do: strong << 16 | weak, or -1 for nothing new. */
    static native long rumble();

    /** The next buzz for the phone: 0 click, 1 tick, 2 alert; waits up to a second, -1 for none. */
    static native int nextBuzz();

    /** A line for the phone's screen. */
    static native String status();

    /**
     * The phone's apps, for the room's launcher (Shell.listApps): a JSON list of objects with
     * "name", "exec" ("package/activity") and "icon" (a PNG's path).
     */
    static native void apps(String json);

    /** Whether Spatiand's shell helper is there, which Android's apps in the room need. */
    static native void shellReady(boolean ready);

    /** A display the room shows an app on has no app left on it: its window goes. */
    static native void appGone(int display);

    /** Open one of the phone's apps ("package/activity") in the room, as the launcher does. */
    static native void openApp(String component, String label);

    /** The helper saw an app begin, or stop, hiding its picture (a FLAG_SECURE window). */
    static native void appSecure(int display, boolean on);

    /** The glasses lent to one of the phone's apps, flat, or given back to the room. */
    static native void flat(boolean on);

    /** A click at x,y in the newest of the phone's apps' windows, through the room's own input. */
    static native void appsTap(int x, int y);

    /** A word for the wearer about the phone's apps: "Android apps", a line break, then what. */
    static native void appsSay(String text);

    /**
     * Pair the key at keyPath (PKCS#8 DER) with the phone's Wireless debugging, listed there as
     * name. Blocks; "" when paired, or what went wrong. crates/spatiand-adb.
     */
    static native String adbPair(String host, int port, String code, String keyPath, String name);

    /** Run a command as the shell user through Wireless debugging. Blocks; its output, or "error: ...". */
    static native String adbRun(String host, int port, String keyPath, String command);
}
