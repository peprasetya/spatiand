package id.prasetya.spatiand;

import android.app.Notification;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Bundle;
import android.service.notification.NotificationListenerService;
import android.service.notification.StatusBarNotification;
import android.util.Log;

import org.json.JSONObject;

import java.util.HashMap;
import java.util.Map;

/**
 * The phone's notifications for the room's card, as the Mac shows its banners there
 * (crates/spatiand-android/src/android/notices.rs, mac/.../Notices.swift).
 *
 * <p>Android tells a notification listener of every notification, once the wearer has let the
 * app read them (Settings, "Notification read, reply & control": MainActivity asks). Those that
 * would interrupt on the phone are handed to the room; the rest are left alone:
 *
 * <ul>
 *   <li>Spatiand's own, ongoing ones (music, a call, a download) and group summaries;
 *   <li>those Do Not Disturb holds back (the ranking's {@code matchesInterruptionFilter});
 *   <li>silent ones -- Android's "Silent" section, below {@code IMPORTANCE_DEFAULT}.
 * </ul>
 *
 * <p>Only while the room is running: when it is not, the listener lets Android unbind it
 * ({@code requestUnbind}) so the app's process is not kept for it, and MainActivity asks it back
 * ({@link #rebind}).
 *
 * <p>Pressed on the card, what the notification opens is opened in the room: its content intent,
 * sent by the shell helper onto a display of the room's (Shell.sendNotice), in its app's window.
 */
public final class Notices extends NotificationListenerService {
    static final String TAG = "spatiand";

    private static volatile Notices running;
    /** Each notification on the card or that could be: its intent and its app, by key. */
    private static final Map<String, StatusBarNotification> posted = new HashMap<>();

    static ComponentName component(Context c) {
        return new ComponentName(c, Notices.class);
    }

    /** Whether the wearer has let Spatiand read notifications. */
    static boolean allowed(Context c) {
        return c.getSystemService(NotificationManager.class).isNotificationListenerAccessGranted(component(c));
    }

    /** The room is running: the listener is to be bound again, if the wearer allows it at all. */
    static void rebind(Context c) {
        if (!allowed(c)) return;
        try {
            NotificationListenerService.requestRebind(component(c));
        } catch (Exception e) {
            Log.w(TAG, "notifications: " + e);
        }
    }

    /** The room is going: the listener lets go, so nothing keeps the app's process for it. */
    static void unbind() {
        Notices n = running;
        if (n == null) return;
        try {
            n.requestUnbind();
        } catch (Exception e) {
            Log.w(TAG, "notifications: " + e);
        }
    }

    @Override
    public void onListenerConnected() {
        if (MainActivity.running == null) {
            // Bound by Android with no room to show anything in (after a restart of the phone).
            requestUnbind();
            return;
        }
        running = this;
        Log.i(TAG, "notifications: listening");
    }

    @Override
    public void onListenerDisconnected() {
        if (running == this) running = null;
        synchronized (posted) {
            posted.clear();
        }
    }

    @Override
    public void onNotificationPosted(StatusBarNotification sbn, RankingMap ranking) {
        if (MainActivity.running == null || !interrupts(sbn, ranking)) return;
        Notification n = sbn.getNotification();
        Bundle extras = n.extras;
        CharSequence title = extras == null ? null : extras.getCharSequence(Notification.EXTRA_TITLE);
        CharSequence text = extras == null ? null : extras.getCharSequence(Notification.EXTRA_TEXT);
        if (text == null && extras != null) text = extras.getCharSequence(Notification.EXTRA_BIG_TEXT);
        synchronized (posted) {
            posted.put(sbn.getKey(), sbn);
        }
        Native.notice(sbn.getKey(), appName(sbn.getPackageName()),
                title == null ? "" : title.toString(), text == null ? "" : text.toString());
    }

    @Override
    public void onNotificationRemoved(StatusBarNotification sbn, RankingMap ranking) {
        synchronized (posted) {
            posted.remove(sbn.getKey());
        }
        if (MainActivity.running != null) Native.noticeGone(sbn.getKey());
    }

    /** Whether a notification is one that would interrupt on the phone. */
    private boolean interrupts(StatusBarNotification sbn, RankingMap ranking) {
        if (sbn.getPackageName().equals(getPackageName())) return false;
        Notification n = sbn.getNotification();
        if (sbn.isOngoing() || (n.flags & Notification.FLAG_GROUP_SUMMARY) != 0
                || (n.flags & Notification.FLAG_FOREGROUND_SERVICE) != 0) {
            return false;
        }
        Ranking r = new Ranking();
        if (ranking != null && ranking.getRanking(sbn.getKey(), r)) {
            if (!r.matchesInterruptionFilter()) return false;
            if (r.getImportance() < NotificationManager.IMPORTANCE_DEFAULT) return false;
        }
        return true;
    }

    private String appName(String pkg) {
        PackageManager pm = getPackageManager();
        try {
            return pm.getApplicationLabel(pm.getApplicationInfo(pkg, 0)).toString();
        } catch (PackageManager.NameNotFoundException gone) {
            return pkg;
        }
    }

    /**
     * The card was pressed: what the notification opens is kept for the helper to send (Shell),
     * the notification taken away if it goes when tapped, and the room told what to open, as JSON:
     * "package", "label", "intent" (whether it has its own to open), "launcher" (its app's
     * launcher activity, for one that has not). Null when it has gone.
     */
    static String press(String key) {
        StatusBarNotification sbn;
        synchronized (posted) {
            sbn = posted.get(key);
        }
        Notices n = running;
        if (sbn == null || n == null) return null;
        Notification notification = sbn.getNotification();
        PendingIntent intent = notification.contentIntent;
        Shell.pendingNotice = intent;
        if ((notification.flags & Notification.FLAG_AUTO_CANCEL) != 0) {
            try {
                n.cancelNotification(key);
            } catch (Exception e) {
                Log.w(TAG, "notifications: " + e);
            }
        }
        try {
            String launcher = "";
            Intent main = n.getPackageManager().getLaunchIntentForPackage(sbn.getPackageName());
            if (main != null && main.getComponent() != null) launcher = main.getComponent().flattenToString();
            return new JSONObject()
                    .put("package", sbn.getPackageName())
                    .put("label", n.appName(sbn.getPackageName()))
                    .put("intent", intent != null)
                    .put("launcher", launcher)
                    .toString();
        } catch (Exception e) {
            Log.w(TAG, "notifications: " + e);
            return null;
        }
    }
}
