package id.prasetya.spatiand.standin;

import android.graphics.Bitmap;
import android.graphics.PixelFormat;
import android.hardware.display.VirtualDisplayConfig;
import android.media.Image;
import android.media.ImageReader;
import android.os.Binder;
import android.os.IBinder;
import android.os.Looper;

import java.io.File;
import java.io.FileOutputStream;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.nio.ByteBuffer;

/**
 * A display standing in for the glasses on the Beam Pro, for testing the room there with no
 * glasses plugged in: see run.sh. Run by the shell, which may make public displays:
 *
 *   CLASSPATH=standin.dex app_process / id.prasetya.spatiand.standin.StandIn [width height]
 *
 * The app takes a presentation display by this name as the glasses' (MainActivity.glassesDisplay),
 * so the room draws its world into it -- in one eye, as there are no glasses to ask for two --
 * and sends its windows their frames as it would on the glasses. What it shows is kept here:
 * touching /data/local/tmp/standin.shot saves the newest picture as /data/local/tmp/standin.png.
 * The display goes when this program does.
 */
public final class StandIn {
    static final String NAME = "Spatiand stand-in glasses";
    static final File SHOT = new File("/data/local/tmp/standin.shot");
    static final File PNG = new File("/data/local/tmp/standin.png");

    public static void main(String[] args) throws Exception {
        int width = args.length >= 2 ? Integer.parseInt(args[0]) : 1920;
        int height = args.length >= 2 ? Integer.parseInt(args[1]) : 1080;
        Looper.prepareMainLooper();
        ImageReader reader = ImageReader.newInstance(width, height, PixelFormat.RGBA_8888, 3);

        IBinder service = (IBinder) Class.forName("android.os.ServiceManager")
                .getMethod("getService", String.class).invoke(null, "display");
        Object displays = Class.forName("android.hardware.display.IDisplayManager$Stub")
                .getMethod("asInterface", IBinder.class).invoke(null, service);
        // The display lives as long as this token does: the service watches it die.
        Binder token = new Binder();
        Class<?> callback = Class.forName("android.hardware.display.IVirtualDisplayCallback");
        Object told = Proxy.newProxyInstance(callback.getClassLoader(), new Class<?>[] {callback},
                (proxy, method, a) -> {
                    switch (method.getName()) {
                        case "asBinder": return token;
                        case "hashCode": return System.identityHashCode(proxy);
                        case "equals": return proxy == a[0];
                        case "toString": return "stand-in glasses";
                        default: return null;
                    }
                });
        int flags = 1 /* PUBLIC */ | 2 /* PRESENTATION */ | 8 /* OWN_CONTENT_ONLY */
                | (1 << 8) /* DESTROY_CONTENT_ON_REMOVAL */ | (1 << 10) /* TRUSTED */;
        // Unlocked whatever the phone is, as the glasses are while worn: a locked phone otherwise
        // puts "Unlock your device" over the room. Asked for first, and done without where the
        // shell may not (OWN_DISPLAY_GROUP, ALWAYS_UNLOCKED).
        int unlocked = (1 << 11) | (1 << 12);
        Method create = null;
        for (Method m : displays.getClass().getMethods()) {
            if (m.getName().equals("createVirtualDisplay")) create = m;
        }
        int id;
        try {
            id = make(displays, create, callback, told, reader, width, height, flags | unlocked);
        } catch (java.lang.reflect.InvocationTargetException refused) {
            System.out.println("not unlocked: " + refused.getCause());
            id = make(displays, create, callback, told, reader, width, height, flags);
        }
        System.out.println("stand-in glasses: display " + id + ", " + width + "x" + height);

        long since = System.nanoTime();
        int frames = 0;
        while (true) {
            Image image = reader.acquireLatestImage();
            if (image != null) {
                frames++;
                if (SHOT.exists()) {
                    save(image, width, height);
                    SHOT.delete();
                }
                image.close();
            }
            long now = System.nanoTime();
            if (now - since >= 5_000_000_000L) {
                System.out.println(String.format("%d frames in 5 s", frames));
                frames = 0;
                since = now;
            }
            Thread.sleep(4);
        }
    }

    static int make(Object displays, Method create, Class<?> callback, Object told, ImageReader reader,
            int width, int height, int flags) throws Exception {
        VirtualDisplayConfig config = new VirtualDisplayConfig.Builder(NAME, width, height, 320)
                .setSurface(reader.getSurface()).setFlags(flags).build();
        Object[] call = new Object[create.getParameterCount()];
        Class<?>[] types = create.getParameterTypes();
        for (int i = 0; i < call.length; i++) {
            if (types[i] == VirtualDisplayConfig.class) call[i] = config;
            else if (types[i] == callback) call[i] = told;
            else if (types[i] == String.class) call[i] = "com.android.shell";
        }
        return (Integer) create.invoke(displays, call);
    }

    static void save(Image image, int width, int height) throws Exception {
        Image.Plane plane = image.getPlanes()[0];
        ByteBuffer pixels = plane.getBuffer();
        Bitmap row = Bitmap.createBitmap(plane.getRowStride() / plane.getPixelStride(), height,
                Bitmap.Config.ARGB_8888);
        row.copyPixelsFromBuffer(pixels);
        File part = new File(PNG.getPath() + ".part");
        try (FileOutputStream out = new FileOutputStream(part)) {
            Bitmap.createBitmap(row, 0, 0, width, height).compress(Bitmap.CompressFormat.PNG, 100, out);
        }
        part.renameTo(PNG);
        System.out.println("saved " + PNG);
    }
}
