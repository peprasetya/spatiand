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
 * provider is the one way a process outside the app can give it one. Nothing else is here, and
 * nobody but the shell user (or root) is answered.
 */
public final class ShellProvider extends ContentProvider {
    @Override
    public Bundle call(String method, String arg, Bundle extras) {
        int caller = Binder.getCallingUid();
        if (caller != Process.SHELL_UID && caller != Process.ROOT_UID) {
            throw new SecurityException("only Spatiand's shell helper may call this");
        }
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
