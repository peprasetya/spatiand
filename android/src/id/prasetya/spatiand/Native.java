package id.prasetya.spatiand;

import android.view.Surface;

/** libspatiand.so: crates/spatiand-android. Every call returns quickly. */
final class Native {
    static {
        System.loadLibrary("spatiand");
    }

    private Native() {}

    /** Take the glasses through a UsbDeviceConnection's descriptor, which is duplicated. */
    static native boolean start(int usbFd);

    /** Let go of the glasses, putting them back in 2D. */
    static native void stop();

    /** Draw into the glasses' Presentation. */
    static native void surface(Surface surface);

    /** The surface is going; returns once nothing draws into it. */
    static native void surfaceGone();

    static native void recenter();

    /** A few lines for the phone's screen. */
    static native String status();
}
