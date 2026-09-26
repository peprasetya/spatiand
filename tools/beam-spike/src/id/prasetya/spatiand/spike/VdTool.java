package id.prasetya.spatiand.spike;

import android.content.AttributionSource;
import android.content.Context;
import android.content.ContextWrapper;
import android.hardware.display.DisplayManager;
import android.hardware.display.VirtualDisplay;
import android.media.Image;
import android.media.ImageReader;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Looper;
import android.os.Process;

import java.lang.reflect.Constructor;
import java.lang.reflect.Field;
import java.lang.reflect.Method;

/**
 * Phase 0 spike, check (d): run as the shell user, the way spatiand-bridge would,
 * and create a TRUSTED virtual display that another app can be launched onto.
 *
 *   adb push spike.apk /data/local/tmp/
 *   adb shell CLASSPATH=/data/local/tmp/spike.apk app_process / \
 *       id.prasetya.spatiand.spike.VdTool 1920 1080 240 [seconds]
 *
 * Prints the display id; then, from another shell:
 *   am start --display <id> -n <pkg>/<activity>
 *   input -d <id> tap 960 540
 *   screencap -d <id> -p /data/local/tmp/vd.png
 *
 * Frames the display produces are counted from an ImageReader, which stands in for
 * the SurfaceTexture the real app would sample.
 */
public class VdTool {
    // Nebula's LaunchManager uses 1485; the same set minus SECURE (4), which needs
    // CAPTURE_SECURE_VIDEO_OUTPUT. Shell lacks it, so protected apps will render black.
    static final int FLAGS = DisplayManager.VIRTUAL_DISPLAY_FLAG_PUBLIC            // 1
            | DisplayManager.VIRTUAL_DISPLAY_FLAG_OWN_CONTENT_ONLY                 // 8
            | 64    // SUPPORTS_TOUCH
            | 128   // ROTATES_WITH_CONTENT
            | 256   // DESTROY_CONTENT_ON_REMOVAL
            | 1024; // TRUSTED

    static class ShellContext extends ContextWrapper {
        ShellContext(Context base) { super(base); }
        @Override public String getPackageName() { return "com.android.shell"; }
        @Override public String getOpPackageName() { return "com.android.shell"; }
        @Override public AttributionSource getAttributionSource() {
            return new AttributionSource.Builder(Process.SHELL_UID).setPackageName("com.android.shell").build();
        }
        @Override public Context getApplicationContext() { return this; }
    }

    static Context systemContext() throws Exception {
        Class<?> at = Class.forName("android.app.ActivityThread");
        Constructor<?> c = at.getDeclaredConstructor();
        c.setAccessible(true);
        Object thread = c.newInstance();
        Field cur = at.getDeclaredField("sCurrentActivityThread");
        cur.setAccessible(true);
        cur.set(null, thread);
        Method sys = at.getDeclaredMethod("getSystemContext");
        sys.setAccessible(true);
        return (Context) sys.invoke(thread);
    }

    public static void main(String[] args) throws Exception {
        int w = Integer.parseInt(args[0]), h = Integer.parseInt(args[1]), dpi = Integer.parseInt(args[2]);
        int seconds = args.length > 3 ? Integer.parseInt(args[3]) : 120;
        Looper.prepareMainLooper();
        Context ctx = new ShellContext(systemContext());
        Constructor<DisplayManager> dmc = DisplayManager.class.getDeclaredConstructor(Context.class);
        dmc.setAccessible(true);
        DisplayManager dm = dmc.newInstance(ctx);

        HandlerThread ht = new HandlerThread("frames");
        ht.start();
        ImageReader reader = ImageReader.newInstance(w, h, android.graphics.PixelFormat.RGBA_8888, 3);
        final int[] frames = {0};
        reader.setOnImageAvailableListener(r -> {
            Image img = r.acquireLatestImage();
            if (img != null) { frames[0]++; img.close(); }
        }, new Handler(ht.getLooper()));

        VirtualDisplay vd = dm.createVirtualDisplay("spatiand-spike", w, h, dpi, reader.getSurface(), FLAGS);
        int id = vd.getDisplay().getDisplayId();
        System.out.println("display " + id + " flags=0x" + Integer.toHexString(vd.getDisplay().getFlags()));
        System.out.flush();
        for (int s = 0; s < seconds; s += 2) {
            Thread.sleep(2000);
            int f; synchronized (frames) { f = frames[0]; frames[0] = 0; }
            System.out.println("fps " + (f / 2.0));
            System.out.flush();
        }
        vd.release();
        System.out.println("released");
        System.exit(0);
    }
}
