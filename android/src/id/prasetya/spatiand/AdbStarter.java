package id.prasetya.spatiand;

import android.app.Activity;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.RemoteInput;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.graphics.drawable.Icon;
import android.net.nsd.NsdManager;
import android.net.nsd.NsdServiceInfo;
import android.os.Bundle;
import android.os.SystemClock;
import android.provider.Settings;
import android.util.Log;

import java.io.File;
import java.io.FileOutputStream;
import java.net.InetAddress;
import java.net.NetworkInterface;
import java.security.KeyPairGenerator;
import java.security.spec.RSAKeyGenParameterSpec;
import java.util.Collections;
import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

/**
 * Spatiand starting its shell helper ({@link ShellHelper}) by itself, through the phone's own
 * Wireless debugging -- what Shizuku does for itself, done here so no other app is needed.
 *
 * <p>Once: the wearer opens Wireless debugging, taps "Pair device with pairing code" and types
 * the six digits into Spatiand's notification (Settings closes its dialog the moment it is left,
 * so the code cannot be typed into Spatiand's own screen). Spatiand pairs its own key with them.
 *
 * <p>Then, whenever the helper is not there and Wireless debugging is on, Spatiand connects with
 * that key and starts it. The adb protocol itself is crates/spatiand-adb, in libspatiand.so;
 * what is Android's -- finding adbd's ports, the key, the notification -- is here.
 *
 * <p>The helper lives as long as adbd does: Android stops everything adbd started when adbd
 * stops, which it does at a reboot, and when Wireless debugging goes off with USB debugging off.
 * Spatiand starts it again when it can.
 */
final class AdbStarter {
    static final String TAG = "spatiand";
    static final String CHANNEL = "shell-helper";
    static final int NOTIFICATION = 70;
    static final String ACTION_CODE = "id.prasetya.spatiand.PAIRING_CODE";
    static final String ACTION_AUTO = "id.prasetya.spatiand.HELPER_AUTO";
    static final String KEY_CODE = "code";
    static final String PAIRING = "_adb-tls-pairing._tcp";
    static final String CONNECT = "_adb-tls-connect._tcp";
    /** How Spatiand is listed under Wireless debugging's paired devices. */
    static final String NAME = "Spatiand";
    static final int NOTIFICATIONS_REQUEST = 41;

    private static volatile boolean starting;

    private AdbStarter() {}

    static SharedPreferences prefs(Context c) {
        return c.getSharedPreferences("adb", Context.MODE_PRIVATE);
    }

    static boolean paired(Context c) {
        return prefs(c).getBoolean("paired", false);
    }

    // --- Spatiand's key ---

    /** Spatiand's own adb key, made the first time: RSA 2048, PKCS#8, in the app's own files. */
    static synchronized String keyPath(Context c) throws Exception {
        File key = new File(c.getFilesDir(), "adbkey.der");
        if (!key.exists()) {
            KeyPairGenerator make = KeyPairGenerator.getInstance("RSA");
            make.initialize(new RSAKeyGenParameterSpec(2048, RSAKeyGenParameterSpec.F4));
            byte[] der = make.generateKeyPair().getPrivate().getEncoded();
            File part = new File(key.getPath() + ".part");
            try (FileOutputStream out = new FileOutputStream(part)) {
                out.write(der);
            }
            if (!part.renameTo(key)) throw new java.io.IOException("could not keep the key");
            Log.i(TAG, "made Spatiand's adb key");
        }
        return key.getPath();
    }

    // --- this phone's adbd ---

    /** This phone's own addresses, to tell its adbd from another phone's on the same network. */
    static Set<String> ownAddresses() {
        Set<String> mine = new HashSet<>();
        try {
            for (NetworkInterface i : Collections.list(NetworkInterface.getNetworkInterfaces())) {
                for (InetAddress a : Collections.list(i.getInetAddresses())) mine.add(plain(a));
            }
        } catch (Exception e) {
            Log.w(TAG, "no addresses: " + e);
        }
        return mine;
    }

    static String plain(InetAddress a) {
        String s = a.getHostAddress();
        int scope = s.indexOf('%');
        return scope < 0 ? s : s.substring(0, scope);
    }

    /** The port of this phone's own service of {@code type}, or -1 if none was found in time. */
    static int findPort(Context c, String type, long timeoutMs) {
        NsdManager nsd = c.getSystemService(NsdManager.class);
        Set<String> mine = ownAddresses();
        CompletableFuture<Integer> found = new CompletableFuture<>();
        ExecutorService on = Executors.newSingleThreadExecutor();
        NsdManager.DiscoveryListener listener = new NsdManager.DiscoveryListener() {
            @Override
            public void onServiceFound(NsdServiceInfo service) {
                nsd.registerServiceInfoCallback(service, on, new NsdManager.ServiceInfoCallback() {
                    @Override
                    public void onServiceUpdated(NsdServiceInfo resolved) {
                        for (InetAddress a : resolved.getHostAddresses()) {
                            if (mine.contains(plain(a))) found.complete(resolved.getPort());
                        }
                        try {
                            nsd.unregisterServiceInfoCallback(this);
                        } catch (IllegalArgumentException gone) {
                            // Already.
                        }
                    }

                    @Override
                    public void onServiceInfoCallbackRegistrationFailed(int error) {}

                    @Override
                    public void onServiceLost() {}

                    @Override
                    public void onServiceInfoCallbackUnregistered() {}
                });
            }

            @Override
            public void onDiscoveryStarted(String serviceType) {}

            @Override
            public void onDiscoveryStopped(String serviceType) {}

            @Override
            public void onServiceLost(NsdServiceInfo service) {}

            @Override
            public void onStartDiscoveryFailed(String serviceType, int error) {
                found.complete(-1);
            }

            @Override
            public void onStopDiscoveryFailed(String serviceType, int error) {}
        };
        nsd.discoverServices(type, NsdManager.PROTOCOL_DNS_SD, listener);
        try {
            return found.get(timeoutMs, TimeUnit.MILLISECONDS);
        } catch (Exception none) {
            return -1;
        } finally {
            try {
                nsd.stopServiceDiscovery(listener);
            } catch (IllegalArgumentException already) {
                // Stopped by its failure.
            }
            on.shutdown();
        }
    }

    // --- pairing, once ---

    /**
     * The wearer asked to pair (MainActivity's button): Spatiand's notification to type the code
     * into, then Developer options, at Wireless debugging. Notifications are asked for first.
     */
    static void beginPairing(Activity a) {
        if (a.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS)
                != PackageManager.PERMISSION_GRANTED) {
            a.requestPermissions(new String[] {android.Manifest.permission.POST_NOTIFICATIONS}, NOTIFICATIONS_REQUEST);
            return;
        }
        askForCode(a, null);
        openWirelessDebugging(a);
    }

    /** The answer to the notifications question: on with pairing if allowed. */
    static void notificationsAnswered(Activity a) {
        if (a.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS)
                == PackageManager.PERMISSION_GRANTED) {
            beginPairing(a);
        } else {
            new android.app.AlertDialog.Builder(a)
                    .setTitle("Pairing needs Spatiand's notification")
                    .setMessage("Settings closes the pairing code as soon as it is left, so the code is typed "
                            + "into Spatiand's notification. Allow Spatiand's notifications, then try again.")
                    .setPositiveButton("OK", null)
                    .show();
        }
    }

    /**
     * Developer options, at Wireless debugging. Settings has no page of its own for it that an app
     * may open; ":settings:fragment_args_key" is how Settings' own search finds a row, and
     * where it is not understood the page opens at the top.
     */
    static void openWirelessDebugging(Context c) {
        Intent intent = new Intent(Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        Bundle args = new Bundle();
        args.putString(":settings:fragment_args_key", "toggle_adb_wireless");
        intent.putExtra(":settings:fragment_args_key", "toggle_adb_wireless");
        intent.putExtra(":settings:show_fragment_args", args);
        try {
            c.startActivity(intent);
        } catch (android.content.ActivityNotFoundException none) {
            Log.w(TAG, "no Developer options");
        }
    }

    static NotificationManager notifications(Context c) {
        NotificationManager nm = c.getSystemService(NotificationManager.class);
        nm.createNotificationChannel(new NotificationChannel(CHANNEL, "Spatiand's helper",
                NotificationManager.IMPORTANCE_HIGH));
        return nm;
    }

    static Notification.Builder note(Context c, String title, String text) {
        return new Notification.Builder(c, CHANNEL)
                .setSmallIcon(Icon.createWithResource(c, c.getApplicationInfo().icon))
                .setContentTitle(title)
                .setContentText(text)
                .setStyle(new Notification.BigTextStyle().bigText(text));
    }

    /** The notification the pairing code is typed into; {@code problem} says why it is asked again. */
    static void askForCode(Context c, String problem) {
        RemoteInput input = new RemoteInput.Builder(KEY_CODE).setLabel("Pairing code").build();
        PendingIntent typed = PendingIntent.getBroadcast(c, 0,
                new Intent(ACTION_CODE).setClass(c, Typed.class),
                PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_MUTABLE);
        Notification.Action enter = new Notification.Action.Builder(
                Icon.createWithResource(c, c.getApplicationInfo().icon), "Enter the code", typed)
                .addRemoteInput(input)
                .build();
        String text = (problem != null ? problem + "\n\n" : "")
                + "In Wireless debugging, tap \"Pair device with pairing code\", then type its six digits here.";
        notifications(c).notify(NOTIFICATION, note(c, "Pair Spatiand with Wireless debugging", text)
                .addAction(enter)
                .setOngoing(true)
                .build());
    }

    static void tell(Context c, String title, String text, Notification.Action action) {
        Notification.Builder n = note(c, title, text).setAutoCancel(true);
        if (action != null) n.addAction(action);
        notifications(c).notify(NOTIFICATION, n.build());
    }

    /** The code typed into the notification, or its "start by itself" answer. */
    public static final class Typed extends BroadcastReceiver {
        @Override
        public void onReceive(Context context, Intent intent) {
            Context c = context.getApplicationContext();
            if (ACTION_AUTO.equals(intent.getAction())) {
                prefs(c).edit().putBoolean("auto", true).apply();
                Shell.grantSecureSettings();
                tell(c, "Spatiand's helper", "After a restart, Spatiand turns Wireless debugging on by itself when it "
                        + "needs its helper, on a Wi-Fi network Wireless debugging has been allowed on.", null);
                return;
            }
            Bundle results = RemoteInput.getResultsFromIntent(intent);
            CharSequence code = results == null ? null : results.getCharSequence(KEY_CODE);
            if (code == null) return;
            tell(c, "Pairing Spatiand", "Pairing with Wireless debugging…", null);
            PendingResult pending = goAsync();
            new Thread(() -> {
                try {
                    pair(c, code.toString());
                } finally {
                    pending.finish();
                }
            }, "pairing").start();
        }
    }

    static void pair(Context c, String code) {
        String digits = code.replaceAll("[^0-9]", "");
        int port = findPort(c, PAIRING, 6000);
        if (port < 0) {
            askForCode(c, "Spatiand could not find the pairing on this phone. Is \"Pair device with pairing code\" still "
                    + "open in Settings?");
            return;
        }
        String said;
        try {
            said = Native.adbPair("127.0.0.1", port, digits, keyPath(c), NAME);
        } catch (Exception e) {
            said = String.valueOf(e);
        }
        if (!said.isEmpty()) {
            askForCode(c, "That did not pair: " + said);
            return;
        }
        prefs(c).edit().putBoolean("paired", true).apply();
        Log.i(TAG, "paired with Wireless debugging");
        tell(c, "Spatiand is paired", "Starting Spatiand's helper…", null);
        startNow(c, false);
    }

    // --- starting the helper ---

    /** Start the helper if it is not there; {@code quiet}: no notification if it cannot be. */
    static void start(Context c, boolean quiet) {
        Context app = c.getApplicationContext();
        if (Shell.ready() || starting) return;
        starting = true;
        new Thread(() -> {
            try {
                startNow(app, quiet);
            } finally {
                starting = false;
            }
        }, "shell-helper").start();
    }

    /** What could not be done, in the room and, unless quiet, as a notification. */
    static void cannot(Context c, boolean quiet, String text) {
        Log.w(TAG, "shell helper: " + text);
        Native.appsSay("Android apps\n" + text);
        if (!quiet) tell(c, "Spatiand's helper", text, null);
    }

    static boolean mayTurnOn(Context c) {
        return prefs(c).getBoolean("auto", false)
                && c.checkSelfPermission(android.Manifest.permission.WRITE_SECURE_SETTINGS)
                        == PackageManager.PERMISSION_GRANTED;
    }

    static void startNow(Context c, boolean quiet) {
        if (Shell.ready()) return;
        if (!paired(c)) {
            cannot(c, quiet, "Spatiand's helper has to be paired with Wireless debugging once: open Spatiand on "
                    + "the phone and tap \"Let Spatiand open the phone's apps\".");
            return;
        }
        int port = findPort(c, CONNECT, 3000);
        if (port < 0 && mayTurnOn(c)) {
            Log.i(TAG, "turning Wireless debugging on");
            Settings.Global.putInt(c.getContentResolver(), "adb_wifi_enabled", 1);
            port = findPort(c, CONNECT, 8000);
        }
        if (port < 0) {
            cannot(c, quiet, "Wireless debugging is off, so Spatiand's helper cannot start. Turn it on in "
                    + "Developer options, or with its quick settings tile.");
            return;
        }
        // Its own session, so it outlives the command that starts it; its log where the shell
        // may write.
        String command = "CLASSPATH=" + c.getApplicationInfo().sourceDir
                + " setsid app_process /system/bin --nice-name=spatiand-shell " + ShellHelper.class.getName()
                + " </dev/null >/data/local/tmp/spatiand-shell.log 2>&1 &";
        String out;
        try {
            out = Native.adbRun("127.0.0.1", port, keyPath(c), command);
        } catch (Exception e) {
            out = "error: " + e;
        }
        if (out.startsWith("error: ")) {
            cannot(c, quiet, "Spatiand's helper could not start: " + out.substring(7)
                    + ". If Spatiand is no longer under Wireless debugging's paired devices, pair it again.");
            return;
        }
        for (int i = 0; i < 50 && !Shell.ready(); i++) SystemClock.sleep(150);
        if (!Shell.ready()) {
            cannot(c, quiet, "Spatiand's helper was started but has not answered.");
            return;
        }
        Log.i(TAG, "shell helper started");
        if (!quiet && !prefs(c).getBoolean("auto", false)) {
            PendingIntent yes = PendingIntent.getBroadcast(c, 1, new Intent(ACTION_AUTO).setClass(c, Typed.class),
                    PendingIntent.FLAG_IMMUTABLE);
            tell(c, "Android apps are ready in the room",
                    "Android takes Spatiand's helper away at every restart, and whenever Wireless debugging goes off "
                            + "while USB debugging is off. Spatiand can turn Wireless debugging on by itself when it "
                            + "needs the helper again.",
                    new Notification.Action.Builder(Icon.createWithResource(c, c.getApplicationInfo().icon),
                            "Do that by itself", yes).build());
        } else if (!quiet) {
            notifications(c).cancel(NOTIFICATION);
        }
    }
}
