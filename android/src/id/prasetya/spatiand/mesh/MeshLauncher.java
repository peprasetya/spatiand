package id.prasetya.spatiand.mesh;

import android.app.ActivityManager;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.net.Credentials;
import android.net.LocalServerSocket;
import android.net.LocalSocket;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.Message;
import android.os.Messenger;
import android.os.RemoteException;
import android.os.SystemClock;
import android.util.Log;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.List;

/**
 * Starts SpatiMesh's engines for its window (docs/spatimesh.md). The window is a native program
 * the compositor started, which cannot start an Android service itself; it asks here, in
 * Spatiand's own process, on an abstract socket only this app's user is answered on:
 *
 *   start PROFILE PRIVATE SOCKET      PRIVATE 1 or 0; SOCKET the abstract name it listens on
 *
 * answered with one line: "ok", "full" (every process for kept profiles is in use) or
 * "no WHY". The window keeps the connection open while it wants the engine; when it closes it,
 * or goes itself, the engine is let go of and its process ends.
 *
 * Written on Linux and compiled by CI; not yet run on a Beam Pro.
 */
public final class MeshLauncher {
    static final String TAG = "spatimesh";

    /** The processes for kept profiles, then the private profile's (AndroidManifest.xml). */
    private static final Class<?>[] SERVICES = {MeshEngine.Slot0.class, MeshEngine.Slot1.class, MeshEngine.Slot2.class,
            MeshEngine.Slot3.class, MeshEngine.Private.class};
    private static final String[] PROCESSES = {":mesh0", ":mesh1", ":mesh2", ":mesh3", ":meshprivate"};
    private static final int PRIVATE = 4;

    private static MeshLauncher running;

    private final Context context;
    private final Handler main = new Handler(Looper.getMainLooper());
    /** The connection each process is bound with, null where it is free. */
    private final ServiceConnection[] bound = new ServiceConnection[SERVICES.length];
    /** The profile each process was last given, kept after it is let go of. */
    private final String[] shown = new String[SERVICES.length];

    private MeshLauncher(Context context) {
        this.context = context.getApplicationContext();
    }

    /** Listen for SpatiMesh, once for this process. */
    public static synchronized void start(Context context) {
        if (running != null)
            return;
        running = new MeshLauncher(context);
        Thread listen = new Thread(running::listen, "spatimesh-launcher");
        listen.setDaemon(true);
        listen.start();
    }

    private void listen() {
        String name = "spatimesh-launch-" + android.os.Process.myUid();
        try (LocalServerSocket server = new LocalServerSocket(name)) {
            while (true) {
                LocalSocket client = server.accept();
                Thread serve = new Thread(() -> serve(client), "spatimesh-launch");
                serve.setDaemon(true);
                serve.start();
            }
        } catch (IOException e) {
            Log.e(TAG, "SpatiMesh's engines cannot be started: " + e);
        }
    }

    private static String readLine(InputStream in) throws IOException {
        StringBuilder line = new StringBuilder();
        while (line.length() < 4096) {
            int c = in.read();
            if (c < 0)
                return null;
            if (c == '\n')
                return line.toString();
            line.append((char) c);
        }
        return null;
    }

    private void serve(LocalSocket client) {
        int slot = -1;
        ServiceConnection connection = null;
        try (LocalSocket socket = client) {
            Credentials peer = socket.getPeerCredentials();
            if (peer == null || peer.getUid() != android.os.Process.myUid()) {
                Log.w(TAG, "a launch asked for by another user, refused");
                return;
            }
            InputStream in = socket.getInputStream();
            OutputStream out = socket.getOutputStream();
            String line = readLine(in);
            String[] words = line == null ? new String[0] : line.trim().split(" ");
            if (words.length != 4 || !words[0].equals("start")) {
                out.write("no not understood\n".getBytes(StandardCharsets.UTF_8));
                return;
            }
            String profile = words[1], name = words[3];
            boolean isPrivate = words[2].equals("1");
            slot = take(profile, isPrivate);
            if (slot < 0) {
                out.write((slot == FULL ? "full\n" : "no the profile's last engine is still ending\n").getBytes(StandardCharsets.UTF_8));
                slot = -1;
                return;
            }
            connection = bind(slot, profile, isPrivate, name);
            if (connection == null) {
                free(slot, null);
                slot = -1;
                out.write("no the engine's service could not be bound\n".getBytes(StandardCharsets.UTF_8));
                return;
            }
            out.write("ok\n".getBytes(StandardCharsets.UTF_8));
            // Held open while the window wants the engine: its end is the engine's end.
            while (in.read() >= 0) {}
        } catch (IOException e) {
            Log.i(TAG, "a launch connection ended: " + e);
        } finally {
            if (slot >= 0)
                free(slot, connection);
        }
    }

    private static final int FULL = -1, BUSY = -2;

    /**
     * A process for `profile`, taken: FULL if all four for kept profiles are in use, BUSY if
     * the profile's last process has not ended in three seconds. Two processes never show one
     * profile at once: WebView allows one process a data directory.
     */
    private int take(String profile, boolean isPrivate) {
        long until = SystemClock.uptimeMillis() + 3000;
        while (true) {
            List<String> alive = runningProcesses();
            synchronized (this) {
                if (isPrivate) {
                    if (bound[PRIVATE] == null && !alive.contains(PROCESSES[PRIVATE])) {
                        claim(PRIVATE, profile);
                        return PRIVATE;
                    }
                } else {
                    boolean ending = false, free = false;
                    for (int i = 0; i < PRIVATE; i++) {
                        if (profile.equals(shown[i]) && (bound[i] != null || alive.contains(PROCESSES[i])))
                            ending = true;
                        if (bound[i] == null)
                            free = true;
                    }
                    if (!ending) {
                        for (int i = 0; i < PRIVATE; i++) {
                            if (bound[i] == null && !alive.contains(PROCESSES[i])) {
                                claim(i, profile);
                                return i;
                            }
                        }
                        if (!free)
                            return FULL;
                    }
                }
            }
            if (SystemClock.uptimeMillis() > until)
                return BUSY;
            SystemClock.sleep(100);
        }
    }

    /** Held until bound, so that no one else takes it meanwhile. */
    private static final ServiceConnection CLAIMED = new ServiceConnection() {
        @Override
        public void onServiceConnected(ComponentName name, IBinder service) {}

        @Override
        public void onServiceDisconnected(ComponentName name) {}
    };

    private void claim(int slot, String profile) {
        bound[slot] = CLAIMED;
        shown[slot] = profile;
    }

    /** This app's processes that are running, by the name after the package's. */
    private List<String> runningProcesses() {
        List<String> names = new java.util.ArrayList<>();
        ActivityManager manager = context.getSystemService(ActivityManager.class);
        List<ActivityManager.RunningAppProcessInfo> all = manager == null ? null : manager.getRunningAppProcesses();
        if (all != null) {
            String prefix = context.getPackageName();
            for (ActivityManager.RunningAppProcessInfo p : all)
                if (p.processName.startsWith(prefix))
                    names.add(p.processName.substring(prefix.length()));
        }
        return names;
    }

    private ServiceConnection bind(int slot, String profile, boolean isPrivate, String socket) {
        ServiceConnection connection = new ServiceConnection() {
            @Override
            public void onServiceConnected(ComponentName component, IBinder service) {
                Bundle what = new Bundle();
                what.putString("profile", profile);
                what.putBoolean("private", isPrivate);
                what.putString("socket", socket);
                Message message = Message.obtain();
                message.setData(what);
                try {
                    new Messenger(service).send(message);
                } catch (RemoteException e) {
                    Log.w(TAG, "the engine went before it was told whose it is: " + e);
                }
            }

            @Override
            public void onServiceDisconnected(ComponentName component) {
                // Its process ended (as it does when its window lets it go, or a crash): not
                // started again for nobody.
                free(slot, this);
            }
        };
        synchronized (this) {
            bound[slot] = connection;
        }
        Intent intent = new Intent(context, SERVICES[slot]);
        boolean ok;
        try {
            ok = context.bindService(intent, connection, Context.BIND_AUTO_CREATE | Context.BIND_IMPORTANT);
        } catch (SecurityException e) {
            Log.w(TAG, "the engine's service could not be bound: " + e);
            ok = false;
        }
        if (!ok) {
            synchronized (this) {
                bound[slot] = CLAIMED;
            }
            return null;
        }
        return connection;
    }

    /** A process let go of: unbound, which ends it, if it is still `connection`'s. */
    private void free(int slot, ServiceConnection connection) {
        synchronized (this) {
            ServiceConnection now = bound[slot];
            if (connection != null && now != connection)
                return;
            bound[slot] = null;
        }
        if (connection != null)
            main.post(() -> {
                try {
                    context.unbindService(connection);
                } catch (IllegalArgumentException e) {
                    // Already unbound.
                }
            });
    }
}
