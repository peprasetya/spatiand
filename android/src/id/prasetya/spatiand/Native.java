package id.prasetya.spatiand;

import android.view.Surface;

/** libspatiand.so: crates/spatiand-android. Every call returns quickly. */
final class Native {
    static {
        System.loadLibrary("spatiand");
    }

    private Native() {}

    /** Where to keep what is learned about the glasses between runs. Call first. */
    static native void configDir(String path);

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

    /** The host: its link, a pairing under way, the windows it has open. */
    static native String remoteStatus();

    /** What the host offers, one "id\tname" per line. */
    static native String apps();

    /** Ask the host to start one of its applications. */
    static native void launch(String id);

    /** Close every window the host has open here. */
    static native void closeAll();

    /** Start pairing with a host that is running spatiand-host --pair. */
    static native void pair(String address);

    /** The codes on the phone and on the host match. */
    static native void confirmPair();
}
