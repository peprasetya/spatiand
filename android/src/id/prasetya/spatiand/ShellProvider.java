package id.prasetya.spatiand;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.net.Uri;
import android.os.Binder;
import android.os.Bundle;
import android.os.IBinder;
import android.os.Process;

/**
 * Where {@link ShellHelper}, running as the shell user, hands the app its Binder: a call on this
 * provider is the one way a process outside the app can give it one. And adb's way to try the
 * phone's apps in the room with no glasses' pointer, since the caller of a provider is known
 * and the sender of an activity's intent is not:
 *
 * <pre>
 *   content call --uri content://id.prasetya.spatiand.shell --method open --arg pkg/activity
 *   content call --uri content://id.prasetya.spatiand.shell --method tap --arg x,y
 * </pre>
 *
 * the first opens an app in the room as its launcher does, the second clicks at x,y (its own
 * pixels) in the newest app's window through the room's own input path. Both only while
 * Spatiand is open. Nobody but the shell user (or root) is answered.
 */
public final class ShellProvider extends ContentProvider {
    @Override
    public Bundle call(String method, String arg, Bundle extras) {
        int caller = Binder.getCallingUid();
        if (caller != Process.SHELL_UID && caller != Process.ROOT_UID) {
            throw new SecurityException("only Spatiand's shell helper may call this");
        }
        if ("open".equals(method) || "tap".equals(method)) return asked(method, arg);
        if (!"attach".equals(method) || extras == null) return null;
        IBinder helper = extras.getBinder("helper");
        if (helper == null) return null;
        boolean kept = Shell.attached(helper, extras.getInt("version", 0), extras.getString("apk"));
        Bundle reply = new Bundle();
        // A helper from before an update is told to go: a new one is on its way.
        reply.putBoolean("go", !kept);
        reply.putBinder("app", Shell.APP);
        reply.putInt("uid", Process.myUid());
        return reply;
    }

    /** adb trying the phone's apps in the room: see the class's note. */
    static Bundle asked(String method, String arg) {
        Bundle reply = new Bundle();
        MainActivity main = MainActivity.running;
        boolean done = false;
        if (main == null) {
            reply.putString("why", "Spatiand is not open");
        } else if ("open".equals(method)) {
            done = arg != null && android.content.ComponentName.unflattenFromString(arg) != null;
            if (done) main.runOnUiThread(() -> main.openAsked(arg));
            else reply.putString("why", "not package/activity");
        } else {
            done = arg != null && arg.matches("\\d{1,6},\\d{1,6}");
            if (done) {
                String[] at = arg.split(",");
                Native.appsTap(Integer.parseInt(at[0]), Integer.parseInt(at[1]));
            } else {
                reply.putString("why", "not x,y");
            }
        }
        reply.putBoolean("done", done);
        return reply;
    }

    @Override
    public boolean onCreate() {
        return true;
    }

    @Override
    public Cursor query(Uri uri, String[] projection, String selection, String[] args, String order) {
        return null;
    }

    @Override
    public String getType(Uri uri) {
        return null;
    }

    @Override
    public Uri insert(Uri uri, ContentValues values) {
        return null;
    }

    @Override
    public int delete(Uri uri, String selection, String[] args) {
        return 0;
    }

    @Override
    public int update(Uri uri, ContentValues values, String selection, String[] args) {
        return 0;
    }
}
