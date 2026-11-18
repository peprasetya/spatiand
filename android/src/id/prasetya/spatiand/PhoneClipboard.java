package id.prasetya.spatiand;

import android.content.ClipData;
import android.content.ClipDescription;
import android.content.ClipboardManager;
import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.net.Uri;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.PersistableBundle;
import android.util.Log;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;

/**
 * The phone's clipboard, joined to the room's (crates/spatiand-android/src/android/clipboard.rs):
 * copy in one of the phone's apps and paste in SpatiMesh, a host's window or one of Spatiand's
 * apps, and the other way round -- as the Mac's pasteboard is joined (mac/.../Clipboard.swift).
 *
 * <p>This end only reads and writes Android's clipboard; what to offer and what to put is the
 * room's. Twice a second it looks whether the clipboard has changed (its description's
 * timestamp), and if so says what it holds: text, or a picture as PNG.
 *
 * <p>Android lets an app read the clipboard only while it has the focus (Android 10 and later).
 * Spatiand has it while its touch area is on the phone's screen, which is while the room is in
 * use; when it has not, Spatiand's shell helper reads it -- the shell may read it in the
 * background -- text only, since a picture's address is granted to whoever reads the clipboard,
 * which would then be the shell. Writing is any app's to do.
 */
final class PhoneClipboard {
    static final String TAG = "spatiand";
    /** How often the clipboard is looked at: the Mac's 0.4 s. */
    static final long EVERY_MS = 400;
    /** The largest picture taken from the clipboard: the room's eager limit (local_board.rs). */
    static final int LARGEST_PICTURE = 8 << 20;

    private static PhoneClipboard running;

    private final Context context;
    private final ClipboardManager board;
    private final Handler handler;
    /** The timestamp of the clip last read or written, so each is said once. */
    private long seen = -1;
    private int written;

    static synchronized void start(Context c) {
        if (running == null) running = new PhoneClipboard(c.getApplicationContext());
    }

    private PhoneClipboard(Context context) {
        this.context = context;
        board = context.getSystemService(ClipboardManager.class);
        HandlerThread thread = new HandlerThread("clipboard");
        thread.start();
        handler = new Handler(thread.getLooper());
        handler.post(this::look);
    }

    /** Whether this app may read the clipboard itself now: its activity has the focus. */
    private static boolean mayRead() {
        MainActivity main = MainActivity.running;
        return main != null && main.hasWindowFocus();
    }

    private void look() {
        try {
            long stamp = stamp();
            if (stamp > 0 && stamp != seen) {
                seen = stamp;
                read();
            }
        } catch (Exception e) {
            Log.w(TAG, "clipboard: " + e);
        }
        handler.postDelayed(this::look, EVERY_MS);
    }

    /** When the clip on the clipboard was put there; 0 for none, -1 when that cannot be found out. */
    private long stamp() {
        if (mayRead()) {
            ClipDescription d = board.getPrimaryClipDescription();
            return d == null ? 0 : d.getTimestamp();
        }
        if (Shell.ready()) return Shell.clipStamp();
        return -1;
    }

    private void read() {
        if (!mayRead()) {
            String text = Shell.clipText();
            if (text != null && !text.isEmpty()) Native.clipboardText(text);
            return;
        }
        ClipData clip = board.getPrimaryClip();
        if (clip == null || clip.getItemCount() == 0) return;
        // A password copied in a password manager is not offered to the room and its hosts.
        if (sensitive(clip.getDescription())) return;
        ClipData.Item item = clip.getItemAt(0);
        Uri uri = item.getUri();
        String type = uri == null ? null : context.getContentResolver().getType(uri);
        if (uri != null && type != null && type.startsWith("image/")) {
            byte[] png = picture(uri, type);
            if (png != null) {
                Native.clipboardImage(png);
                return;
            }
        }
        CharSequence text = item.getText();
        if (text == null && uri == null) text = item.coerceToText(context);
        if (text != null && text.length() > 0) Native.clipboardText(text.toString());
    }

    /** A copied picture as PNG: as it is if it is one, drawn again as one if not. */
    private byte[] picture(Uri uri, String type) {
        try (InputStream in = context.getContentResolver().openInputStream(uri)) {
            if (in == null) return null;
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            byte[] buffer = new byte[65536];
            for (int n; (n = in.read(buffer)) > 0; ) {
                bytes.write(buffer, 0, n);
                if (bytes.size() > LARGEST_PICTURE * 4) return null;
            }
            byte[] read = bytes.toByteArray();
            if (type.equals("image/png") && read.length <= LARGEST_PICTURE) return read;
            Bitmap bitmap = BitmapFactory.decodeByteArray(read, 0, read.length);
            if (bitmap == null) return null;
            ByteArrayOutputStream png = new ByteArrayOutputStream();
            bitmap.compress(Bitmap.CompressFormat.PNG, 100, png);
            bitmap.recycle();
            return png.size() <= LARGEST_PICTURE ? png.toByteArray() : null;
        } catch (Exception e) {
            Log.w(TAG, "clipboard: the copied picture could not be read: " + e);
            return null;
        }
    }

    // --- what the room copied, put on the phone's clipboard ---

    /**
     * Text copied in the room; `secret` when the room's copy said it is one (a password manager's):
     * marked sensitive, so Android does not show it in its clipboard preview.
     */
    static void putText(String text, boolean secret) {
        PhoneClipboard c = running;
        if (c == null) return;
        c.handler.post(() -> {
            ClipData clip = ClipData.newPlainText("Spatiand", text);
            if (secret) {
                PersistableBundle extras = new PersistableBundle();
                extras.putBoolean(IS_SENSITIVE, true);
                clip.getDescription().setExtras(extras);
            }
            c.put(clip);
        });
    }

    /** ClipDescription.EXTRA_IS_SENSITIVE (Android 13), read and written by any version. */
    static final String IS_SENSITIVE = "android.content.extra.IS_SENSITIVE";

    /** Whether the clip's owner marked it sensitive (a password): such a clip stays on the phone. */
    static boolean sensitive(ClipDescription d) {
        PersistableBundle extras = d == null ? null : d.getExtras();
        return extras != null && extras.getBoolean(IS_SENSITIVE, false);
    }

    /**
     * A picture copied in the room: a file of the app's own, handed out by its read-only provider
     * (OpenFiles), whose address Android grants to whichever app pastes it.
     */
    static void putImage(byte[] png) {
        PhoneClipboard c = running;
        if (c == null) return;
        c.handler.post(() -> {
            File dir = new File(c.context.getCacheDir(), "clipboard");
            dir.mkdirs();
            File[] old = dir.listFiles();
            if (old != null) for (File f : old) f.delete();
            File file = new File(dir, "copied-" + (++c.written) + ".png");
            try (FileOutputStream out = new FileOutputStream(file)) {
                out.write(png);
            } catch (Exception e) {
                Log.w(TAG, "clipboard: could not keep the copied picture: " + e);
                return;
            }
            c.put(ClipData.newUri(c.context.getContentResolver(), "Spatiand", OpenFiles.uriFor(file.getPath())));
        });
    }

    private void put(ClipData clip) {
        try {
            board.setPrimaryClip(clip);
            // Said already: the room knows what it put there.
            if (mayRead()) {
                ClipDescription d = board.getPrimaryClipDescription();
                if (d != null) seen = d.getTimestamp();
            } else if (Shell.ready()) {
                seen = Shell.clipStamp();
            }
        } catch (Exception e) {
            Log.w(TAG, "clipboard: could not put the room's copy on the phone's: " + e);
        }
    }
}
