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

    /** Start the session, once; it runs for the life of the process. */
    static native void begin();

    /** Take the glasses through a UsbDeviceConnection's descriptor, which is duplicated. */
    static native boolean start(int usbFd);

    /** Let go of the glasses, putting them back in 2D. */
    static native void stop();

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

    /** 0 the orange key (STEAM), 1 home (⋯), 2 back (B). */
    static native void button(int which, boolean down);

    /** A key, by KeyEvent code. Returns whether the session takes it. */
    static native boolean key(int code, boolean down);

    /** Text Android's keyboard committed. */
    static native void text(String text);

    /** Recentre the room, and aim the phone where the head faces. */
    static native void recenter();

    /** A line for the phone's screen. */
    static native String status();
}
