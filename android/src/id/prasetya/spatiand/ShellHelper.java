package id.prasetya.spatiand;

import android.content.AttributionSource;
import android.content.ComponentName;
import android.content.Context;
import android.content.ContextWrapper;
import android.media.AudioFormat;
import android.media.AudioManager;
import android.media.AudioRecord;
import android.os.ParcelFileDescriptor;
import android.content.Intent;
import android.hardware.display.VirtualDisplayConfig;
import android.net.LocalServerSocket;
import android.os.Binder;
import android.os.Bundle;
import android.os.IBinder;
import android.os.Looper;
import android.os.Parcel;
import android.os.RemoteException;
import android.os.SystemClock;
import android.view.InputDevice;
import android.view.InputEvent;
import android.view.KeyCharacterMap;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.Surface;

import java.io.File;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.nio.file.Files;
import java.util.HashMap;
import java.util.Map;

/**
 * Spatiand's hands on the phone as the shell user: what lets the room show Android's own apps.
 *
 * <p>Android gives an ordinary app no way to put another app on a display it made, to touch it
 * there, or to take its sound. The shell user (adb's) may do all three, so this program runs as
 * that user and does them for the app, which reaches it through the Binder this class is. It is
 * started by Spatiand itself through the phone's own Wireless debugging ({@code AdbStarter}), or
 * by hand over adb ({@code tools/beam-shell/run.sh}):
 *
 * <pre>
 *   CLASSPATH=$(pm path id.prasetya.spatiand | cut -d: -f2) setsid app_process /system/bin \
 *       --nice-name=spatiand-shell id.prasetya.spatiand.ShellHelper
 * </pre>
 *
 * <p>It hands itself to the app through the app's {@link ShellProvider}, and again whenever the
 * app's process is new; it answers no one but that app. One runs at a time: a newer one tells
 * the one before it to go. Nothing of it outlives a reboot, which is Android's rule for anything
 * started this way.
 */
public final class ShellHelper extends Binder {
    /** Told to the app on attaching; the app starts a new helper when its own is newer. */
    static final int VERSION = 1;
    static final String TAG = "spatiand-shell";
    static final String PACKAGE = "id.prasetya.spatiand";
    static final String AUTHORITY = PACKAGE + ".shell";
    static final String LOCK = "spatiand-shell";

    // What the app asks; the numbers are the wire, shared with Shell.java.
    static final int CREATE_DISPLAY = 1;
    static final int RESIZE_DISPLAY = 2;
    static final int RELEASE_DISPLAY = 3;
    static final int LAUNCH = 4;
    static final int MOTION = 5;
    static final int KEY = 6;
    static final int TEXT = 7;
    static final int QUIT = 9;
    static final int PING = 10;
    static final int TASKS = 11;
    static final int GRANT_SECURE_SETTINGS = 12;
    static final int SOUND = 14;
    static final int AWAKE = 15;
    static final int MOVE_TASK = 16;
    static final int TOUCH = 17;

    // What this tells the app, on the Binder the app gave when it attached.
    static final int TOLD_EMPTY = 1;
    static final int TOLD_SECURE = 2;

    /**
     * Every app's display: its own picture, its own touch and focus, its apps closed with it, and
     * unlocked whatever the phone is -- which Android allows only a display in a power group of
     * its own (in the phone's, an app showed "Unlock your device to continue" while the phone
     * was locked).
     *
     * <p>A group of its own also has its own power, which nothing outlasts but a screen wake
     * lock on it: one is held for each display while the room draws ({@link #awake}), so an app
     * watched without being touched stays lit, and nothing is lit for glasses not worn.
     *
     * <p>Not SECURE (4): that needs CAPTURE_SECURE_VIDEO_OUTPUT, which only system apps have, so
     * an app that protects its picture shows black here (see docs/beam-pro.md).
     */
    static final int FLAGS = 1 /* PUBLIC */ | 8 /* OWN_CONTENT_ONLY */ | 64 /* SUPPORTS_TOUCH */
            | 256 /* DESTROY_CONTENT_ON_REMOVAL */ | 1024 /* TRUSTED */ | 2048 /* OWN_DISPLAY_GROUP */
            | 4096 /* ALWAYS_UNLOCKED */ | 8192 /* TOUCH_FEEDBACK_DISABLED */ | 16384 /* OWN_FOCUS */;
    /** SCREEN_BRIGHT_WAKE_LOCK | ACQUIRE_CAUSES_WAKEUP: lit, and woken if asleep. */
    static final int SCREEN_LOCK = 0x1000000a;

    static final class Display {
        int id;
        Object callback;
        int width, height, dpi;
        boolean hadTask;
        /** Whether a window on it is FLAG_SECURE, as last told to the app. */
        boolean secure;
        /** The screen wake lock that keeps it lit while the room draws, or null. */
        Binder awake;
        /** Its app's sound, taken off the speakers for the room (see {@link #sound}). */
        Object policy;
        AudioRecord record;
    }

    final Map<Integer, Display> displays = new HashMap<>();
    final Object displayManager;
    final Method createVirtualDisplay, resizeVirtualDisplay, setVirtualDisplaySurface, releaseVirtualDisplay;
    final Class<?> callbackClass;
    final Object activityManager;
    final Object activityTaskManager;
    final Object inputManager;
    final Method injectInputEvent;
    final Method setDisplayId;
    final Object powerManager;
    Method acquireWakeLock, releaseWakeLock;
    /** Whether the room is drawing, and so whether the displays are held lit. */
    boolean roomDrawing;

    final KeyCharacterMap characters = KeyCharacterMap.load(KeyCharacterMap.VIRTUAL_KEYBOARD);

    /** The app's uid, the only caller answered; and its Binder, to tell it things. */
    volatile int appUid = -1;
    volatile IBinder app;

    public static void main(String[] args) throws Exception {
        Looper.prepareMainLooper();
        LocalServerSocket lock = takeLock();
        if (lock == null) {
            log("could not take over from the helper before");
            System.exit(1);
        }
        ShellHelper helper = new ShellHelper();
        log("running as uid " + android.os.Process.myUid() + ", version " + VERSION);
        Thread keeper = new Thread(helper::keepAttached, "attach");
        keeper.setDaemon(true);
        keeper.start();
        Thread watcher = new Thread(helper::watchDisplays, "displays");
        watcher.setDaemon(true);
        watcher.start();
        // Binder calls arrive on the threads app_process starts for them; this one only waits.
        Looper.loop();
    }

    /**
     * The lock is an abstract socket only one process can hold. A helper already holding it is
     * asked to go through the socket itself, so a newer program replaces an older one.
     */
    static LocalServerSocket takeLock() throws Exception {
        for (int attempt = 0; attempt < 20; attempt++) {
            try {
                LocalServerSocket server = new LocalServerSocket(LOCK);
                Thread t = new Thread(() -> {
                    while (true) {
                        try (android.net.LocalSocket peer = server.accept()) {
                            // Only another shell process may say so: the app has its Binder.
                            if (peer.getPeerCredentials().getUid() == android.os.Process.myUid()) {
                                log("a newer helper is starting; going");
                                System.exit(0);
                            }
                        } catch (Exception e) {
                            log("lock: " + e);
                        }
                    }
                }, "lock");
                t.setDaemon(true);
                t.start();
                return server;
            } catch (java.io.IOException taken) {
                try (android.net.LocalSocket s = new android.net.LocalSocket()) {
                    s.connect(new android.net.LocalSocketAddress(LOCK));
                } catch (java.io.IOException gone) {
                    // Gone between the two: try again.
                }
                Thread.sleep(150);
            }
        }
        return null;
    }

    static void log(String line) {
        System.out.println(line);
        android.util.Log.i(TAG, line);
    }

    static Object service(String name, String stub) throws Exception {
        IBinder binder = (IBinder) Class.forName("android.os.ServiceManager")
                .getMethod("getService", String.class).invoke(null, name);
        return Class.forName(stub + "$Stub").getMethod("asInterface", IBinder.class).invoke(null, binder);
    }

    static Method method(Object target, String name) {
        Method found = null;
        for (Method m : target.getClass().getMethods()) {
            if (m.getName().equals(name)) found = m;
        }
        return found;
    }

    ShellHelper() throws Exception {
        displayManager = service("display", "android.hardware.display.IDisplayManager");
        createVirtualDisplay = method(displayManager, "createVirtualDisplay");
        resizeVirtualDisplay = method(displayManager, "resizeVirtualDisplay");
        setVirtualDisplaySurface = method(displayManager, "setVirtualDisplaySurface");
        releaseVirtualDisplay = method(displayManager, "releaseVirtualDisplay");
        callbackClass = Class.forName("android.hardware.display.IVirtualDisplayCallback");
        activityManager = service("activity", "android.app.IActivityManager");
        activityTaskManager = service("activity_task", "android.app.IActivityTaskManager");
        inputManager = service("input", "android.hardware.input.IInputManager");
        injectInputEvent = inputManager.getClass().getMethod("injectInputEvent", InputEvent.class, int.class);
        setDisplayId = InputEvent.class.getMethod("setDisplayId", int.class);
        powerManager = service("power", "android.os.IPowerManager");
        for (Method m : powerManager.getClass().getMethods()) {
            // (lock, flags, tag, package, work source, history tag, display, callback)
            if (m.getName().equals("acquireWakeLock") && m.getParameterCount() == 8) acquireWakeLock = m;
            if (m.getName().equals("releaseWakeLock") && m.getParameterCount() == 2) releaseWakeLock = m;
        }
    }

    // --- being found by the app ---

    /** Hand this Binder to the app whenever its process is new, for as long as this runs. */
    void keepAttached() {
        int seenPid = -1;
        while (true) {
            int pid = appPid();
            IBinder current = app;
            if (pid > 0 && (pid != seenPid || current == null || !current.isBinderAlive())) {
                if (attach()) seenPid = pid;
            }
            SystemClock.sleep(1500);
        }
    }

    /** The app's main process, from /proc, which the shell may read all of; -1 for none. */
    static int appPid() {
        File[] all = new File("/proc").listFiles();
        if (all == null) return -1;
        for (File f : all) {
            String name = f.getName();
            if (name.isEmpty() || !Character.isDigit(name.charAt(0))) continue;
            try {
                byte[] cmd = Files.readAllBytes(new File(f, "cmdline").toPath());
                int end = 0;
                while (end < cmd.length && cmd[end] != 0) end++;
                if (new String(cmd, 0, end).equals(PACKAGE)) return Integer.parseInt(name);
            } catch (Exception ignored) {
                // Gone, or not readable.
            }
        }
        return -1;
    }

    /** Call the app's ShellProvider with this Binder; it answers with its own and its uid. */
    boolean attach() {
        Binder token = new Binder();
        try {
            Method get = activityManager.getClass().getMethod("getContentProviderExternal",
                    String.class, int.class, IBinder.class, String.class);
            Object holder = get.invoke(activityManager, AUTHORITY, 0, token, TAG);
            if (holder == null) {
                log("no " + AUTHORITY + ": is Spatiand installed?");
                return false;
            }
            Object provider = holder.getClass().getField("provider").get(holder);
            Bundle extras = new Bundle();
            extras.putBinder("helper", this);
            extras.putInt("version", VERSION);
            // Which APK this runs from: the app replaces a helper left from before an update.
            extras.putString("apk", System.getenv("CLASSPATH"));
            AttributionSource source = new AttributionSource.Builder(android.os.Process.myUid())
                    .setPackageName("com.android.shell").build();
            Method call = provider.getClass().getMethod("call", AttributionSource.class, String.class,
                    String.class, String.class, Bundle.class);
            Bundle reply = (Bundle) call.invoke(provider, source, AUTHORITY, "attach", null, extras);
            activityManager.getClass().getMethod("removeContentProviderExternalAsUser",
                    String.class, IBinder.class, int.class).invoke(activityManager, AUTHORITY, token, 0);
            if (reply == null) return false;
            if (reply.getBoolean("go", false)) {
                log("Spatiand has been updated since this started: going");
                System.exit(0);
            }
            appUid = reply.getInt("uid", -1);
            app = reply.getBinder("app");
            log("attached to the app, uid " + appUid);
            return true;
        } catch (Exception e) {
            Throwable cause = e instanceof java.lang.reflect.InvocationTargetException ? e.getCause() : e;
            log("could not reach the app: " + cause);
            return false;
        }
    }

    /** Tell the app something, on its Binder: one-way, so a busy app never holds this up. */
    void tell(int what, int display, int... more) {
        IBinder to = app;
        if (to == null) return;
        Parcel data = Parcel.obtain();
        try {
            data.writeInt(display);
            for (int m : more) data.writeInt(m);
            to.transact(what, data, null, IBinder.FLAG_ONEWAY);
        } catch (RemoteException e) {
            log("could not tell the app: " + e);
        } finally {
            data.recycle();
        }
    }

    // --- what the app asks ---

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags) throws RemoteException {
        if (code == INTERFACE_TRANSACTION || code == DUMP_TRANSACTION) return super.onTransact(code, data, reply, flags);
        if (Binder.getCallingUid() != appUid) {
            log("refused a call from uid " + Binder.getCallingUid());
            return false;
        }
        try {
            switch (code) {
                case PING:
                    answer(reply, VERSION);
                    return true;
                case CREATE_DISPLAY: {
                    String name = data.readString();
                    int width = data.readInt(), height = data.readInt(), dpi = data.readInt();
                    Surface surface = data.readInt() != 0 ? Surface.CREATOR.createFromParcel(data) : null;
                    int id = createDisplay(name, width, height, dpi, surface);
                    answer(reply, id);
                    return true;
                }
                case RESIZE_DISPLAY: {
                    int id = data.readInt();
                    int width = data.readInt(), height = data.readInt(), dpi = data.readInt();
                    Surface surface = data.readInt() != 0 ? Surface.CREATOR.createFromParcel(data) : null;
                    resizeDisplay(id, width, height, dpi, surface);
                    answer(reply);
                    return true;
                }
                case RELEASE_DISPLAY:
                    releaseDisplay(data.readInt());
                    answer(reply);
                    return true;
                case LAUNCH: {
                    int id = data.readInt();
                    String component = data.readString();
                    int result = launch(id, component);
                    answer(reply, result);
                    return true;
                }
                case MOTION: {
                    int id = data.readInt();
                    int action = data.readInt(), buttons = data.readInt(), actionButton = data.readInt();
                    float x = data.readFloat(), y = data.readFloat();
                    float vscroll = data.readFloat(), hscroll = data.readFloat();
                    long downTime = data.readLong();
                    motion(id, action, buttons, actionButton, x, y, vscroll, hscroll, downTime);
                    return true;
                }
                case KEY: {
                    int id = data.readInt();
                    int action = data.readInt(), keyCode = data.readInt(), meta = data.readInt();
                    long downTime = data.readLong();
                    key(id, action, keyCode, meta, downTime);
                    return true;
                }
                case TEXT: {
                    int id = data.readInt();
                    text(id, data.readString());
                    return true;
                }
                case TASKS: {
                    int id = data.readInt();
                    answer(reply, tasksOn(id));
                    return true;
                }
                case GRANT_SECURE_SETTINGS: {
                    // What lets the app turn Wireless debugging on after a restart; asked only
                    // when the wearer said so (AdbStarter).
                    Process grant = new ProcessBuilder("pm", "grant", PACKAGE,
                            "android.permission.WRITE_SECURE_SETTINGS").redirectErrorStream(true).start();
                    String said = new String(grant.getInputStream().readAllBytes()).trim();
                    int exit = grant.waitFor();
                    log("granted the app WRITE_SECURE_SETTINGS: " + exit + (said.isEmpty() ? "" : " " + said));
                    answer(reply, exit);
                    return true;
                }
                case AWAKE:
                    awake(data.readInt() != 0);
                    return true;
                case MOVE_TASK: {
                    int from = data.readInt(), to = data.readInt();
                    moveTask(from, to);
                    answer(reply);
                    return true;
                }
                case TOUCH: {
                    int id = data.readInt(), action = data.readInt();
                    float x = data.readFloat(), y = data.readFloat();
                    long downTime = data.readLong();
                    touch(id, action, x, y, downTime);
                    return true;
                }
                case SOUND: {
                    int id = data.readInt();
                    int uid = data.readInt();
                    // On the main thread: an AudioPolicy wants a Looper, which Binder's have not.
                    java.util.concurrent.FutureTask<ParcelFileDescriptor> made =
                            new java.util.concurrent.FutureTask<>(() -> sound(id, uid));
                    new android.os.Handler(Looper.getMainLooper()).post(made);
                    ParcelFileDescriptor pipe = made.get(5, java.util.concurrent.TimeUnit.SECONDS);
                    if (reply != null) {
                        reply.writeNoException();
                        reply.writeInt(pipe != null ? 1 : 0);
                        if (pipe != null) {
                            reply.writeFileDescriptor(pipe.getFileDescriptor());
                            pipe.close();
                        }
                    }
                    return true;
                }
                case QUIT:
                    answer(reply);
                    log("asked to go");
                    for (Display d : displays.values().toArray(new Display[0])) releaseDisplay(d.id);
                    new Thread(() -> {
                        SystemClock.sleep(100);
                        System.exit(0);
                    }).start();
                    return true;
                default:
                    return false;
            }
        } catch (Exception e) {
            Throwable cause = e instanceof java.lang.reflect.InvocationTargetException ? e.getCause() : e;
            log("call " + code + " failed: " + cause);
            if (reply != null && (flags & FLAG_ONEWAY) == 0) {
                reply.writeException(cause instanceof RuntimeException ? (RuntimeException) cause
                        : new IllegalStateException(String.valueOf(cause)));
                return true;
            }
            return false;
        }
    }

    /** Reply that the call went well, with its result; nothing for a one-way call. */
    static void answer(Parcel reply, int result) {
        if (reply == null) return;
        reply.writeNoException();
        reply.writeInt(result);
    }

    static void answer(Parcel reply) {
        if (reply != null) reply.writeNoException();
    }

    // --- displays ---

    synchronized int createDisplay(String name, int width, int height, int dpi, Surface surface) throws Exception {
        Binder token = new Binder();
        Object callback = Proxy.newProxyInstance(callbackClass.getClassLoader(), new Class<?>[] {callbackClass},
                (proxy, m, a) -> {
                    switch (m.getName()) {
                        case "asBinder": return token;
                        case "hashCode": return System.identityHashCode(proxy);
                        case "equals": return proxy == a[0];
                        case "toString": return name;
                        default: return null;
                    }
                });
        int id;
        try {
            id = makeDisplay(name, width, height, dpi, surface, callback, FLAGS);
        } catch (java.lang.reflect.InvocationTargetException refused) {
            // Where this Android will not have them: the display is then locked, and lit, with
            // the phone.
            log("display not always unlocked: " + refused.getCause());
            id = makeDisplay(name, width, height, dpi, surface, callback, FLAGS & ~(2048 | 4096));
        }
        Display d = new Display();
        d.id = id;
        d.callback = callback;
        d.width = width;
        d.height = height;
        d.dpi = dpi;
        displays.put(id, d);
        log("display " + id + ": " + name + ", " + width + "x" + height + " at " + dpi + " dpi");
        if (roomDrawing) hold(d);
        return id;
    }

    int makeDisplay(String name, int width, int height, int dpi, Surface surface, Object callback, int flags)
            throws Exception {
        VirtualDisplayConfig.Builder config = new VirtualDisplayConfig.Builder(name, width, height, dpi)
                .setFlags(flags)
                .setRequestedRefreshRate(60);
        if (surface != null) config.setSurface(surface);
        VirtualDisplayConfig built = config.build();
        Object[] call = new Object[createVirtualDisplay.getParameterCount()];
        Class<?>[] types = createVirtualDisplay.getParameterTypes();
        for (int i = 0; i < call.length; i++) {
            if (types[i] == VirtualDisplayConfig.class) call[i] = built;
            else if (types[i] == callbackClass) call[i] = callback;
            else if (types[i] == String.class) call[i] = "com.android.shell";
        }
        return (Integer) createVirtualDisplay.invoke(displayManager, call);
    }

    synchronized void resizeDisplay(int id, int width, int height, int dpi, Surface surface) throws Exception {
        Display d = displays.get(id);
        if (d == null) return;
        // The new surface first: resizing alone would draw the new size into the old buffers.
        if (surface != null) setVirtualDisplaySurface.invoke(displayManager, d.callback, surface);
        resizeVirtualDisplay.invoke(displayManager, d.callback, width, height, dpi);
        d.width = width;
        d.height = height;
        d.dpi = dpi;
    }

    synchronized void releaseDisplay(int id) throws Exception {
        Display d = displays.remove(id);
        if (d == null) return;
        letGo(d);
        stopSound(d);
        releaseVirtualDisplay.invoke(displayManager, d.callback);
        log("display " + id + " released");
    }

    // --- staying lit ---

    /** The room has begun or stopped drawing: each display is held lit, or let go of. */
    synchronized void awake(boolean drawing) {
        if (drawing == roomDrawing) return;
        roomDrawing = drawing;
        log(drawing ? "the room is drawing: its displays are held lit" : "the room has stopped drawing");
        for (Display d : displays.values()) {
            if (drawing) hold(d);
            else letGo(d);
        }
    }

    void hold(Display d) {
        if (d.awake != null || acquireWakeLock == null) return;
        Binder lock = new Binder();
        try {
            acquireWakeLock.invoke(powerManager, lock, SCREEN_LOCK, "spatiand display " + d.id, "com.android.shell",
                    null, null, d.id, null);
            d.awake = lock;
        } catch (Exception e) {
            log("display " + d.id + ": not held lit: " + e);
        }
    }

    void letGo(Display d) {
        if (d.awake == null) return;
        try {
            releaseWakeLock.invoke(powerManager, d.awake, 0);
        } catch (Exception e) {
            log("display " + d.id + ": " + e);
        }
        d.awake = null;
    }

    // --- sound ---

    Context context;
    AudioManager audio;

    /** The system's context, as the shell's: what an AudioManager and an AudioPolicy need. */
    Context context() throws Exception {
        if (context != null) return context;
        Class<?> threads = Class.forName("android.app.ActivityThread");
        java.lang.reflect.Constructor<?> make = threads.getDeclaredConstructor();
        make.setAccessible(true);
        Object thread = make.newInstance();
        java.lang.reflect.Field current = threads.getDeclaredField("sCurrentActivityThread");
        current.setAccessible(true);
        current.set(null, thread);
        Method system = threads.getDeclaredMethod("getSystemContext");
        system.setAccessible(true);
        Context base = (Context) system.invoke(thread);
        context = new ContextWrapper(base) {
            @Override
            public String getPackageName() {
                return "com.android.shell";
            }

            @Override
            public String getOpPackageName() {
                return "com.android.shell";
            }

            @Override
            public AttributionSource getAttributionSource() {
                return new AttributionSource.Builder(android.os.Process.myUid()).setPackageName("com.android.shell").build();
            }

            @Override
            public Context getApplicationContext() {
                return this;
            }
        };
        audio = context.getSystemService(AudioManager.class);
        return context;
    }

    /**
     * Take the sound of the app with this uid off the speakers and hand it to the app, for the
     * room to place at its window: an AudioPolicy (the shell may route audio) whose one mix is
     * every player of that uid, looped back -- not also played -- into a recorder read here, and
     * written down a pipe as 16-bit stereo at 48 kHz. Returns the pipe's reading end, or null.
     *
     * <p>An app that forbids capture of its sound (ALLOW_CAPTURE_BY_NONE), and calls, are not
     * taken: they play as Android plays them.
     */
    synchronized ParcelFileDescriptor sound(int id, int uid) throws Exception {
        Display d = displays.get(id);
        if (d == null) return null;
        stopSound(d);
        context();
        Class<?> rules = Class.forName("android.media.audiopolicy.AudioMixingRule$Builder");
        Object rule = rules.getConstructor().newInstance();
        rules.getMethod("addMixRule", int.class, Object.class).invoke(rule, 4 /* RULE_MATCH_UID */, uid);
        try {
            // Sound the app lets only the system capture, too: the shell has CAPTURE_MEDIA_OUTPUT.
            rules.getMethod("allowPrivilegedPlaybackCapture", boolean.class).invoke(rule, true);
        } catch (NoSuchMethodException old) {
            // Not on this Android.
        }
        Object built = rules.getMethod("build").invoke(rule);
        AudioFormat format = new AudioFormat.Builder()
                .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setSampleRate(48000)
                .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                .build();
        Class<?> mixes = Class.forName("android.media.audiopolicy.AudioMix$Builder");
        Object mix = mixes.getConstructor(built.getClass()).newInstance(built);
        mixes.getMethod("setFormat", AudioFormat.class).invoke(mix, format);
        mixes.getMethod("setRouteFlags", int.class).invoke(mix, 2 /* ROUTE_FLAG_LOOP_BACK */);
        Object audioMix = mixes.getMethod("build").invoke(mix);
        Class<?> policies = Class.forName("android.media.audiopolicy.AudioPolicy$Builder");
        Object policy = policies.getConstructor(Context.class).newInstance(context);
        policies.getMethod("addMix", audioMix.getClass()).invoke(policy, audioMix);
        Object audioPolicy = policies.getMethod("build").invoke(policy);
        int registered = (Integer) AudioManager.class.getMethod("registerAudioPolicy", audioPolicy.getClass())
                .invoke(audio, audioPolicy);
        if (registered != 0) {
            log("display " + id + ": no sound policy (" + registered + ")");
            return null;
        }
        AudioRecord record = (AudioRecord) audioPolicy.getClass().getMethod("createAudioRecordSink", audioMix.getClass())
                .invoke(audioPolicy, audioMix);
        record.startRecording();
        d.policy = audioPolicy;
        d.record = record;
        ParcelFileDescriptor[] pipe = ParcelFileDescriptor.createPipe();
        ParcelFileDescriptor write = pipe[1];
        Thread pump = new Thread(() -> {
            byte[] chunk = new byte[48000 / 100 * 4 * 2]; // 20 ms
            try (java.io.OutputStream out = new ParcelFileDescriptor.AutoCloseOutputStream(write)) {
                while (true) {
                    int n = record.read(chunk, 0, chunk.length);
                    if (n < 0) break;
                    if (n > 0) out.write(chunk, 0, n);
                }
            } catch (Exception gone) {
                // The app closed its end, or the recorder was stopped.
            }
        }, "sound " + id);
        pump.setDaemon(true);
        pump.start();
        log("display " + id + ": the sound of uid " + uid + " goes to the room");
        return pipe[0];
    }

    void stopSound(Display d) {
        if (d.record != null) {
            try {
                d.record.stop();
            } catch (Exception ignored) {
                // Already.
            }
            d.record.release();
            d.record = null;
        }
        if (d.policy != null) {
            try {
                AudioManager.class.getMethod("unregisterAudioPolicy", d.policy.getClass()).invoke(audio, d.policy);
            } catch (Exception e) {
                log("display " + d.id + ": " + e);
            }
            d.policy = null;
        }
    }

    /** Start an app's activity ("package/class") on a display, as its launcher icon does. */
    int launch(int id, String component) throws Exception {
        ComponentName name = ComponentName.unflattenFromString(component);
        if (name == null) throw new IllegalArgumentException("not a component: " + component);
        Intent intent = new Intent(Intent.ACTION_MAIN)
                .addCategory(Intent.CATEGORY_LAUNCHER)
                .setComponent(name)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_RESET_TASK_IF_NEEDED);
        Bundle options = android.app.ActivityOptions.makeBasic().setLaunchDisplayId(id).toBundle();
        Method start = null;
        for (Method m : activityManager.getClass().getMethods()) {
            if (m.getName().equals("startActivityAsUser") && m.getParameterCount() == 11) start = m;
        }
        if (start == null) throw new NoSuchMethodException("startActivityAsUser");
        int result = (Integer) start.invoke(activityManager, null, "com.android.shell", intent, null, null, null,
                0, 0, null, options, /* the current user */ -2);
        log("launched " + component + " on display " + id + ": " + result);
        return result;
    }

    /** How many tasks are on a display; -1 if that could not be found out. */
    int tasksOn(int id) {
        try {
            Method all = activityTaskManager.getClass().getMethod("getAllRootTaskInfosOnDisplay", int.class);
            Object list = all.invoke(activityTaskManager, id);
            return ((java.util.List<?>) list).size();
        } catch (Exception e) {
            return -1;
        }
    }

    /**
     * Tell the app when a display it shows has no app left on it (closed from inside), and when
     * an app there begins or stops hiding its picture.
     */
    void watchDisplays() {
        int round = 0;
        while (true) {
            SystemClock.sleep(1000);
            Display[] now;
            synchronized (this) {
                now = displays.values().toArray(new Display[0]);
            }
            if (now.length > 0 && round++ % 2 == 0) {
                java.util.Set<Integer> secure = secureDisplays();
                for (Display d : now) {
                    boolean is = secure.contains(d.id);
                    if (is != d.secure) {
                        d.secure = is;
                        log("display " + d.id + (is ? " hides its picture" : " shows its picture again"));
                        tell(TOLD_SECURE, d.id, is ? 1 : 0);
                    }
                }
            }
            for (Display d : now) {
                int tasks = tasksOn(d.id);
                if (tasks > 0) {
                    d.hadTask = true;
                } else if (tasks == 0 && d.hadTask) {
                    d.hadTask = false;
                    log("display " + d.id + " has no app left on it");
                    tell(TOLD_EMPTY, d.id);
                }
            }
        }
    }

    /**
     * The displays a visible FLAG_SECURE window is on: Android draws such a window black in a
     * picture of a display that is not SECURE, which ours cannot be. From dumpsys, which reads
     * every window's display and flags; no call an app may make says this.
     */
    java.util.Set<Integer> secureDisplays() {
        java.util.Set<Integer> found = new java.util.HashSet<>();
        try {
            Process dump = new ProcessBuilder("dumpsys", "window", "windows").redirectErrorStream(true).start();
            java.io.BufferedReader lines = new java.io.BufferedReader(new java.io.InputStreamReader(dump.getInputStream()));
            int display = -1;
            boolean secure = false, visible = false;
            for (String line; (line = lines.readLine()) != null; ) {
                if (line.startsWith("  Window #")) {
                    if (secure && visible && display >= 0) found.add(display);
                    display = -1;
                    secure = visible = false;
                    continue;
                }
                String t = line.trim();
                int at = t.indexOf("mDisplayId=");
                if (at >= 0) {
                    int end = at + 11;
                    while (end < t.length() && Character.isDigit(t.charAt(end))) end++;
                    try {
                        display = Integer.parseInt(t.substring(at + 11, end));
                    } catch (NumberFormatException ignored) {
                        // Not this line's.
                    }
                }
                if (t.startsWith("fl=") && (" " + t.substring(3) + " ").contains(" SECURE ")) secure = true;
                if (t.startsWith("isVisible=true") || t.contains(" isVisible=true")) visible = true;
            }
            if (secure && visible && display >= 0) found.add(display);
            dump.waitFor();
        } catch (Exception e) {
            log("could not read the windows: " + e);
        }
        return found;
    }

    /**
     * The app on one display moved to another, task and all: onto the glasses' display for an
     * app shown flat, and back. On the glasses, XREAL's placeholder -- put up by its framework
     * whenever their display is added, as it is on each change of mode -- is taken down, or it
     * would cover the app (`xreal_preview_update`, docs/beam-pro.md); the room's own picture
     * covers it otherwise.
     */
    void moveTask(int from, int to) throws Exception {
        Method all = activityTaskManager.getClass().getMethod("getAllRootTaskInfosOnDisplay", int.class);
        java.util.List<?> tasks = (java.util.List<?>) all.invoke(activityTaskManager, from);
        Method move = activityTaskManager.getClass().getMethod("moveRootTaskToDisplay", int.class, int.class);
        for (Object task : tasks) {
            int id = task.getClass().getField("taskId").getInt(task);
            move.invoke(activityTaskManager, id, to);
            log("task " + id + " from display " + from + " to " + to);
        }
        if (!displays.containsKey(to)) {
            Process placeholder = new ProcessBuilder("settings", "put", "system", "xreal_preview_update", "stop")
                    .redirectErrorStream(true).start();
            placeholder.waitFor();
        }
    }

    // --- input ---

    /** A finger, as a touchscreen's, on a display: for an app shown flat on the glasses. */
    void touch(int display, int action, float x, float y, long downTime) throws Exception {
        long now = SystemClock.uptimeMillis();
        MotionEvent event = MotionEvent.obtain(downTime > 0 ? downTime : now, now, action, x, y, 0);
        event.setSource(InputDevice.SOURCE_TOUCHSCREEN);
        inject(event, display);
        event.recycle();
    }

    void inject(InputEvent event, int display) throws Exception {
        setDisplayId.invoke(event, display);
        injectInputEvent.invoke(inputManager, event, 0 /* ASYNC */);
    }

    void motion(int display, int action, int buttons, int actionButton, float x, float y, float vscroll,
            float hscroll, long downTime) throws Exception {
        MotionEvent.PointerProperties[] properties = {new MotionEvent.PointerProperties()};
        properties[0].id = 0;
        properties[0].toolType = MotionEvent.TOOL_TYPE_MOUSE;
        MotionEvent.PointerCoords[] coords = {new MotionEvent.PointerCoords()};
        coords[0].x = x;
        coords[0].y = y;
        coords[0].pressure = buttons != 0 ? 1f : 0f;
        coords[0].size = 1f;
        if (action == MotionEvent.ACTION_SCROLL) {
            coords[0].setAxisValue(MotionEvent.AXIS_VSCROLL, vscroll);
            coords[0].setAxisValue(MotionEvent.AXIS_HSCROLL, hscroll);
        }
        long now = SystemClock.uptimeMillis();
        MotionEvent event = MotionEvent.obtain(downTime > 0 ? downTime : now, now, action, 1, properties, coords,
                0, buttons, 1f, 1f, 0, 0, InputDevice.SOURCE_MOUSE, 0);
        if (actionButton != 0) {
            MotionEvent.class.getMethod("setActionButton", int.class).invoke(event, actionButton);
        }
        inject(event, display);
        event.recycle();
    }

    void key(int display, int action, int keyCode, int meta, long downTime) throws Exception {
        long now = SystemClock.uptimeMillis();
        KeyEvent event = new KeyEvent(downTime > 0 ? downTime : now, now, action, keyCode, 0, meta,
                KeyCharacterMap.VIRTUAL_KEYBOARD, 0, KeyEvent.FLAG_FROM_SYSTEM, InputDevice.SOURCE_KEYBOARD);
        inject(event, display);
    }

    void text(int display, String text) throws Exception {
        KeyEvent[] events = characters.getEvents(text.toCharArray());
        if (events == null) {
            log("no keys type \"" + text + "\"");
            return;
        }
        for (KeyEvent e : events) inject(e, display);
    }
}
