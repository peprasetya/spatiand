package id.prasetya.spatiand;

import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.drawable.Drawable;
import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Process;
import android.os.RemoteException;
import android.util.Log;
import android.view.Surface;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.File;
import java.io.FileOutputStream;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * The app's side of {@link ShellHelper}: the Binder it handed over, the calls the room makes on
 * it (from the compositor's thread, through JNI: crates/spatiand-android/src/android/apps.rs),
 * and the list of the phone's apps for the room's launcher.
 *
 * <p>Every call here is safe with no helper: it answers that nothing was done.
 */
final class Shell {
    static final String TAG = "spatiand";
    /** Each app's display: as dense as a small tablet's, so a phone app lays out roomily. */
    static final int DPI = 213;

    static volatile IBinder helper;
    static volatile int helperVersion;
    /** The app's own context, for starting the helper when the room asks. */
    static volatile Context context;

    private Shell() {}

    static void init(Context c) {
        context = c.getApplicationContext();
    }

    /** The room wants an app and there is no helper: start one (AdbStarter), from any thread. */
    static void startHelper() {
        Context c = context;
        if (c != null) AdbStarter.start(c, false);
    }

    /**
     * Let Spatiand turn Wireless debugging on itself, when its helper is needed after a restart:
     * the helper, as the shell user, grants it WRITE_SECURE_SETTINGS. Only on the wearer's word
     * (the notification's "Do that by itself").
     */
    static void grantSecureSettings() {
        call(ShellHelper.GRANT_SECURE_SETTINGS, data -> {}, reply -> 0, 0);
    }

    /** What the helper tells the app, one way: a display has no app left on it. */
    static final Binder APP = new Binder() {
        @Override
        protected boolean onTransact(int code, Parcel data, Parcel reply, int flags) throws RemoteException {
            if (code == INTERFACE_TRANSACTION) return super.onTransact(code, data, reply, flags);
            if (Binder.getCallingUid() != Process.SHELL_UID && Binder.getCallingUid() != Process.ROOT_UID) {
                return false;
            }
            if (code == ShellHelper.TOLD_EMPTY) {
                Native.appGone(data.readInt());
                return true;
            }
            if (code == ShellHelper.TOLD_SECURE) {
                int display = data.readInt();
                Native.appSecure(display, data.readInt() != 0);
                return true;
            }
            return false;
        }
    };

    /**
     * The helper has handed itself over (ShellProvider). One left running from before Spatiand
     * was updated runs the old APK's code, which may not speak this one's calls: it is told to
     * go, and a new one started.
     */
    static boolean attached(IBinder binder, int version, String apk) {
        Context c = context;
        if (apk != null && c != null && !apk.equals(c.getApplicationInfo().sourceDir)) {
            Log.i(TAG, "a shell helper from before Spatiand was updated: replacing it");
            new android.os.Handler(android.os.Looper.getMainLooper())
                    .postDelayed(() -> AdbStarter.start(c, true), 1500);
            return false;
        }
        helper = binder;
        helperVersion = version;
        Log.i(TAG, "shell helper " + version + " attached");
        try {
            binder.linkToDeath(() -> {
                if (helper == binder) {
                    helper = null;
                    Log.i(TAG, "shell helper gone");
                    Native.shellReady(false);
                }
            }, 0);
        } catch (RemoteException gone) {
            helper = null;
            return true;
        }
        Native.shellReady(true);
        return true;
    }

    static boolean ready() {
        IBinder h = helper;
        return h != null && h.isBinderAlive();
    }

    /** One call on the helper; the reply is read by {@code read}, or null for a one-way call. */
    private interface Reader<T> {
        T read(Parcel reply);
    }

    private interface Writer {
        void write(Parcel data);
    }

    private static <T> T call(int code, Writer write, Reader<T> read, T none) {
        IBinder h = helper;
        if (h == null) return none;
        Parcel data = Parcel.obtain();
        Parcel reply = read != null ? Parcel.obtain() : null;
        try {
            write.write(data);
            h.transact(code, data, reply, read != null ? 0 : IBinder.FLAG_ONEWAY);
            if (read == null) return none;
            reply.readException();
            return read.read(reply);
        } catch (Exception e) {
            Log.w(TAG, "shell helper call " + code + ": " + e);
            return none;
        } finally {
            data.recycle();
            if (reply != null) reply.recycle();
        }
    }

    // --- what the room asks, by JNI from the compositor's thread ---

    /**
     * Put an app ("package/activity") on a new display drawing into {@code surface}. Returns the
     * display's id, or -1 if there is no helper or it could not be done.
     */
    static int openApp(String component, Surface surface, int width, int height) {
        return openApp(component, surface, width, height, null, null);
    }

    /**
     * As {@link #openApp(String, Surface, int, int)}, the app handed a file to open: {@code uri}
     * (one of {@link OpenFiles}, granted to it by {@link #askSystem}) of type {@code type}, or
     * none.
     */
    static int openApp(String component, Surface surface, int width, int height, String uri, String type) {
        Integer display = call(ShellHelper.CREATE_DISPLAY, data -> {
            data.writeString("Spatiand: " + component);
            data.writeInt(width);
            data.writeInt(height);
            data.writeInt(DPI);
            data.writeInt(1);
            surface.writeToParcel(data, 0);
        }, Parcel::readInt, -1);
        if (display == null || display < 0) return -1;
        int id = display;
        Integer started = launchOn(id, component, uri, type);
        Log.i(TAG, "opened " + component + " on display " + id + ": " + started);
        if (started == null || started < 0) {
            closeApp(id);
            return -1;
        }
        return id;
    }

    /** Start an app on a display it is already on, with a file to open (or none). */
    static int relaunch(int display, String component, String uri, String type) {
        Integer started = launchOn(display, component, uri, type);
        return started == null ? -100 : started;
    }

    private static Integer launchOn(int display, String component, String uri, String type) {
        return call(ShellHelper.LAUNCH, data -> {
            data.writeInt(display);
            data.writeString(component);
            // Since the helper's version 2: a file for it to open.
            if (uri != null) {
                data.writeString(uri);
                data.writeString(type);
            }
        }, Parcel::readInt, -100);
    }

    // --- "Open with", for SpatiShelf (spatiand_xr_v1.ask_system) ---

    private static String error(String why) {
        try {
            return new JSONObject().put("error", why).toString();
        } catch (Exception e) {
            return "{\"error\":\"?\"}";
        }
    }

    private static String component(ResolveInfo r) {
        return r.activityInfo.packageName + "/" + r.activityInfo.name;
    }

    /**
     * What a native program in the app asks of the system through the room, as JSON (the
     * protocol's description of {@code ask_system} says what): which of the phone's apps open a
     * kind of file, and handing one of them a file, granted to it to read. Off the compositor's
     * thread.
     */
    static String askSystem(String json) {
        Context c = context;
        if (c == null) return error("Spatiand is not ready.");
        try {
            JSONObject q = new JSONObject(json);
            PackageManager pm = c.getPackageManager();
            String type = q.optString("type", "*/*");
            switch (q.optString("do")) {
                case "openers": {
                    Intent view = new Intent(Intent.ACTION_VIEW).setDataAndType(OpenFiles.uriFor(q.getString("path")), type);
                    List<ResolveInfo> found = pm.queryIntentActivities(view, PackageManager.ResolveInfoFlags.of(PackageManager.MATCH_DEFAULT_ONLY));
                    ResolveInfo best = pm.resolveActivity(view, PackageManager.ResolveInfoFlags.of(PackageManager.MATCH_DEFAULT_ONLY));
                    // No default chosen, Android answers with its own chooser: not an app.
                    String preferred = best != null && best.activityInfo != null && !"android".equals(best.activityInfo.packageName)
                            && !best.activityInfo.name.endsWith("ResolverActivity") ? component(best) : null;
                    return openersJson(c, pm, found, preferred);
                }
                case "all_openers": {
                    Intent view = new Intent(Intent.ACTION_VIEW).setDataAndType(OpenFiles.uriFor("/storage/emulated/0/any"), "*/*");
                    List<ResolveInfo> found = pm.queryIntentActivities(view, PackageManager.ResolveInfoFlags.of(PackageManager.MATCH_DEFAULT_ONLY));
                    return openersJson(c, pm, found, null);
                }
                case "open_with": {
                    String id = q.getString("id");
                    android.content.ComponentName name = android.content.ComponentName.unflattenFromString(id);
                    if (name == null) return error("Not an app: " + id);
                    android.net.Uri uri = OpenFiles.uriFor(q.getString("path"));
                    c.grantUriPermission(name.getPackageName(), uri, Intent.FLAG_GRANT_READ_URI_PERMISSION);
                    String label = pm.getActivityInfo(name, PackageManager.ComponentInfoFlags.of(0)).loadLabel(pm).toString();
                    return new JSONObject().put("ok", true).put("component", id).put("uri", uri.toString()).put("type", type).put("label", label).toString();
                }
                default:
                    return error("Not something that can be asked: " + q.optString("do"));
            }
        } catch (Exception e) {
            Log.w(TAG, "ask_system: " + e);
            return error(e.toString());
        }
    }

    private static String openersJson(Context c, PackageManager pm, List<ResolveInfo> found, String preferred) throws Exception {
        File dir = new File(c.getCacheDir(), "open-icons");
        dir.mkdirs();
        JSONArray apps = new JSONArray();
        Set<String> seen = new HashSet<>();
        for (ResolveInfo r : found) {
            if (r.activityInfo == null || r.activityInfo.packageName.equals(c.getPackageName())) continue;
            String id = component(r);
            if (!seen.add(id)) continue;
            File icon = new File(dir, id.replace('/', '_') + ".png");
            if (!icon.exists()) {
                try {
                    draw(r.loadIcon(pm), icon);
                } catch (Exception e) {
                    Log.w(TAG, "no icon for " + id + ": " + e);
                }
            }
            JSONObject app = new JSONObject().put("id", id).put("name", r.loadLabel(pm).toString()).put("default", id.equals(preferred));
            if (icon.exists()) app.put("icon", icon.getPath());
            if (id.equals(preferred)) {
                // The system's own choice first.
                JSONArray first = new JSONArray().put(app);
                for (int i = 0; i < apps.length(); i++) first.put(apps.get(i));
                apps = first;
            } else {
                apps.put(app);
            }
        }
        return new JSONObject().put("apps", apps).toString();
    }

    /** The app's window has a new size, and a new surface of that size to draw into. */
    static void resizeApp(int display, Surface surface, int width, int height) {
        call(ShellHelper.RESIZE_DISPLAY, data -> {
            data.writeInt(display);
            data.writeInt(width);
            data.writeInt(height);
            data.writeInt(DPI);
            data.writeInt(1);
            surface.writeToParcel(data, 0);
        }, reply -> 0, 0);
    }

    /**
     * The sound of the app on a display, taken off the speakers for the room: a descriptor read
     * as 16-bit stereo at 48 kHz, or -1. The caller owns it.
     */
    static int appSound(int display, String packageName) {
        Context c = context;
        if (c == null) return -1;
        int uid;
        try {
            uid = c.getPackageManager().getApplicationInfo(packageName, 0).uid;
        } catch (PackageManager.NameNotFoundException gone) {
            return -1;
        }
        // The system's own apps share their uid with the system: taking theirs would take every
        // sound the system makes.
        if (uid < Process.FIRST_APPLICATION_UID) return -1;
        Integer fd = call(ShellHelper.SOUND, data -> {
            data.writeInt(display);
            data.writeInt(uid);
        }, reply -> {
            if (reply.readInt() == 0) return -1;
            android.os.ParcelFileDescriptor pipe = reply.readFileDescriptor();
            return pipe == null ? -1 : pipe.detachFd();
        }, -1);
        return fd == null ? -1 : fd;
    }

    /**
     * The room's word to lend the glasses to the app on a display, flat (its note's button):
     * MainActivity does it, on its own thread.
     */
    static void showFlat(int display, String label) {
        MainActivity main = MainActivity.running;
        if (main != null) main.runOnUiThread(() -> main.showFlat(display, label));
    }

    /** Move the app on one display to another: onto the glasses, flat, and back. */
    static void moveApp(int from, int to) {
        call(ShellHelper.MOVE_TASK, data -> {
            data.writeInt(from);
            data.writeInt(to);
        }, reply -> 0, 0);
    }

    /** A finger on a display, as a touchscreen's: action as MotionEvent's, in its pixels. */
    static void touch(int display, int action, float x, float y, long downTime) {
        call(ShellHelper.TOUCH, data -> {
            data.writeInt(display);
            data.writeInt(action);
            data.writeFloat(x);
            data.writeFloat(y);
            data.writeLong(downTime);
        }, null, 0);
    }

    /** The window is closed: the display goes, and the app's activities on it with it. */
    static void closeApp(int display) {
        call(ShellHelper.RELEASE_DISPLAY, data -> data.writeInt(display), reply -> 0, 0);
    }

    /** A mouse event, as MotionEvent has them (SOURCE_MOUSE), on a display. */
    static void appMotion(int display, int action, int buttons, int actionButton, float x, float y,
            float vscroll, float hscroll, long downTime) {
        call(ShellHelper.MOTION, data -> {
            data.writeInt(display);
            data.writeInt(action);
            data.writeInt(buttons);
            data.writeInt(actionButton);
            data.writeFloat(x);
            data.writeFloat(y);
            data.writeFloat(vscroll);
            data.writeFloat(hscroll);
            data.writeLong(downTime);
        }, null, 0);
    }

    /** A key, by KeyEvent code, with its meta state, on a display. */
    static void appKey(int display, int action, int keyCode, int meta, long downTime) {
        call(ShellHelper.KEY, data -> {
            data.writeInt(display);
            data.writeInt(action);
            data.writeInt(keyCode);
            data.writeInt(meta);
            data.writeLong(downTime);
        }, null, 0);
    }

    /** Text typed into a display's focused window, as the keys that make it. */
    static void appText(int display, String text) {
        call(ShellHelper.TEXT, data -> {
            data.writeInt(display);
            data.writeString(text);
        }, null, 0);
    }

    // --- the launcher's list ---

    /**
     * The phone's apps as the launcher lists them -- every activity its home screen shows,
     * Spatiand's own aside -- with their icons drawn to files, handed to the room as JSON. Off
     * the caller's thread: drawing a hundred icons takes a moment.
     */
    static void listApps(Context context) {
        Context app = context.getApplicationContext();
        new Thread(() -> {
            try {
                Native.apps(appsJson(app));
            } catch (Exception e) {
                Log.w(TAG, "could not list the apps: " + e);
            }
        }, "apps").start();
    }

    static String appsJson(Context context) throws Exception {
        PackageManager pm = context.getPackageManager();
        Intent main = new Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER);
        List<ResolveInfo> found = pm.queryIntentActivities(main, PackageManager.ResolveInfoFlags.of(0));
        File dir = new File(context.getCacheDir(), "app-icons");
        dir.mkdirs();
        Set<String> kept = new HashSet<>();
        JSONArray list = new JSONArray();
        for (ResolveInfo r : found) {
            String pkg = r.activityInfo.packageName;
            if (pkg.equals(context.getPackageName())) continue;
            String component = pkg + "/" + r.activityInfo.name;
            long updated = 0;
            try {
                updated = pm.getPackageInfo(pkg, 0).lastUpdateTime;
            } catch (PackageManager.NameNotFoundException gone) {
                continue;
            }
            // Named for when the app was last updated, so a new icon is a new file: the room
            // caches icons by their path.
            File icon = new File(dir, component.replace('/', '_') + "-" + updated + ".png");
            if (!icon.exists()) {
                try {
                    draw(r.loadIcon(pm), icon);
                } catch (Exception e) {
                    Log.w(TAG, "no icon for " + component + ": " + e);
                }
            }
            kept.add(icon.getName());
            JSONObject entry = new JSONObject();
            entry.put("name", r.loadLabel(pm).toString());
            entry.put("exec", component);
            if (icon.exists()) entry.put("icon", icon.getPath());
            list.put(entry);
        }
        File[] old = dir.listFiles();
        if (old != null) {
            for (File f : old) {
                if (!kept.contains(f.getName())) f.delete();
            }
        }
        Log.i(TAG, list.length() + " apps for the launcher");
        return list.toString();
    }

    static void draw(Drawable drawable, File to) throws Exception {
        int size = 192;
        Bitmap bitmap = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888);
        Canvas canvas = new Canvas(bitmap);
        drawable.setBounds(0, 0, size, size);
        drawable.draw(canvas);
        File part = new File(to.getPath() + ".part");
        try (FileOutputStream out = new FileOutputStream(part)) {
            bitmap.compress(Bitmap.CompressFormat.PNG, 100, out);
        }
        bitmap.recycle();
        if (!part.renameTo(to)) throw new java.io.IOException("could not save " + to);
    }
}
