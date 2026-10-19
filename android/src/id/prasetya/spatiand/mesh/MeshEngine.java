package id.prasetya.spatiand.mesh;

import android.app.Presentation;
import android.app.Service;
import android.content.Intent;
import android.content.pm.PackageInfo;
import android.os.Bundle;
import android.graphics.Bitmap;
import android.graphics.PixelFormat;
import android.hardware.display.DisplayManager;
import android.hardware.display.VirtualDisplay;
import android.media.Image;
import android.media.ImageReader;
import android.net.LocalSocket;
import android.net.LocalSocketAddress;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.IBinder;
import android.os.Looper;
import android.os.Message;
import android.os.Messenger;
import android.os.SystemClock;
import android.system.ErrnoException;
import android.system.Os;
import android.util.Log;
import android.view.InputDevice;
import android.view.KeyCharacterMap;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.webkit.CookieManager;
import android.webkit.JavascriptInterface;
import android.webkit.JsPromptResult;
import android.webkit.JsResult;
import android.webkit.PermissionRequest;
import android.webkit.RenderProcessGoneDetail;
import android.webkit.ValueCallback;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebSettings;
import android.webkit.WebStorage;
import android.webkit.WebView;
import android.webkit.WebViewClient;

import java.io.ByteArrayInputStream;
import java.io.File;
import java.io.FileDescriptor;
import java.io.IOException;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.io.InterruptedIOException;
import java.io.OutputStream;
import java.lang.reflect.InvocationHandler;
import java.nio.Buffer;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;
import org.json.JSONTokener;

/**
 * SpatiMesh's engine on the Beam Pro: one profile's pages, in an Android process of its own
 * (docs/spatimesh.md). WebView must live in an app's process, so this is a service of the
 * Spatiand APK, bound by {@link MeshLauncher} for the window process (`spatimesh`, a native
 * program), which it then speaks to over the same wire as every other engine host. It lives
 * while it is bound: unbound, its process ends.
 *
 * Each tab's WebView is shown on a private virtual display that draws into an ImageReader --
 * the way Flutter shows Android views in its own pictures -- and each picture is copied into
 * one of the tab's two shared buffers (memfd), which the window maps.
 *
 * What is and is not isolated (Android allows no more): every tab of this profile shares one
 * WebView renderer process, so a page that crashes it takes all of them, each told and reloaded
 * when asked; this process is the profile's alone, so another profile's tabs and the window are
 * unharmed. WebView takes one data directory per process, which is why a profile is a process.
 *
 * Written on Linux and compiled by CI; not yet run on a Beam Pro.
 */
public class MeshEngine extends Service {
    static final String TAG = "spatimesh";

    // The fixed processes a profile can have (AndroidManifest.xml): four kept, one private.
    public static class Slot0 extends MeshEngine {}
    public static class Slot1 extends MeshEngine {}
    public static class Slot2 extends MeshEngine {}
    public static class Slot3 extends MeshEngine {}
    public static class Private extends MeshEngine {}

    /** A <select>'s list, asked of SpatiMesh instead of shown in a dialog of WebView's own. */
    static final String SELECT_SCRIPT = "(function(){if(window.__spatimesh)return;window.__spatimesh=1;"
            + "document.addEventListener('mousedown',function(ev){var s=ev.target.closest&&ev.target.closest('select');"
            + "if(!s||s.multiple||s.size>1||s.disabled)return;ev.preventDefault();s.focus();window.__spatimeshSelect=s;"
            + "var r=s.getBoundingClientRect(),o=[];for(var i=0;i<s.options.length;i++){var x=s.options[i];"
            + "o.push({label:x.label||x.text,enabled:!x.disabled,selected:x.selected});}"
            + "spatimeshSelect.open(JSON.stringify({options:o,x:r.left,y:r.top,w:r.width,h:r.height}));},true);"
            + "window.__spatimeshChoose=function(i){var s=window.__spatimeshSelect;window.__spatimeshSelect=null;"
            + "if(!s||i<0||i>=s.options.length)return;if(s.selectedIndex!==i){s.selectedIndex=i;"
            + "s.dispatchEvent(new Event('input',{bubbles:true}));s.dispatchEvent(new Event('change',{bubbles:true}));}};})();";

    private Handler main;
    private Handler pictures;
    private LocalSocket socket;
    private OutputStream out;
    private final Object sending = new Object();
    private final Map<Long, Page> pages = new HashMap<>();
    private boolean started;
    private boolean isPrivate;
    /** The profile's extensions, as the window last gave them (crates/spatimesh/src/extensions/
     *  set.rs, with its `page_start` for this engine): read by WebView's own threads too, when a
     *  page asks for one of their files. */
    private volatile JSONObject extensions;
    /** Their declarativeNetRequest rules, read from it: null for none. */
    private volatile Dnr rules;
    /** The WebView's support-library interfaces, once looked for: null, extensions do not run. */
    private Boundary boundary;
    private boolean boundaryLooked;

    /** One tab. */
    final class Page {
        final long tab;
        int width, height;
        float zoom;
        VirtualDisplay display;
        ImageReader reader;
        Presentation presentation;
        WebView view;
        boolean dead;
        /** The top frame's address, for the WebView's own threads: the page a request with no
         *  Referer or Origin is taken to come from. */
        volatile String url;
        final FileDescriptor[] buffers = new FileDescriptor[2];
        final boolean[] held = new boolean[2];
        int bufferWidth, bufferHeight, newest = -1;
        /** A picture that came while the window had both buffers: kept until one comes back. */
        Image waiting;
        long downTime;
        int buttons;
        JsResult result;
        JsPromptResult prompt;
        /** What the WebView's support library calls this view, the extensions' scripts on it, and
         *  whether their listener is. */
        InvocationHandler provider;
        final List<Object> starts = new ArrayList<>();
        boolean listening;
        /** Frames other than the top one that the extensions' contexts speak from: by world ("" an
         *  extension's own page), then api.js's number for the frame: the extension's id and the
         *  frame's reply proxy. */
        final Map<String, Map<Long, Object[]>> frames = new HashMap<>();

        Page(long tab) {
            this.tab = tab;
        }
    }

    @Override
    public IBinder onBind(Intent intent) {
        // Told whose pages these are, and where the window is, by the launcher's one message.
        Handler told = new Handler(Looper.getMainLooper(), message -> {
            Bundle what = message.getData();
            begin(what.getString("profile", ""), what.getBoolean("private"), what.getString("socket", ""));
            return true;
        });
        return new Messenger(told).getBinder();
    }

    @Override
    public boolean onUnbind(Intent intent) {
        // The window let the profile's pages go, or went itself.
        end();
        return false;
    }

    private void begin(String profile, boolean isPrivate, String name) {
        if (started || name.isEmpty())
            return;
        started = true;
        this.isPrivate = isPrivate;
        if (isPrivate)
            forgetPrivate();
        // A profile, a process, a data directory: WebView's rule, and before any WebView.
        try {
            WebView.setDataDirectorySuffix(isPrivate ? "spatimesh-private" : "spatimesh-" + profile);
        } catch (IllegalStateException e) {
            Log.w(TAG, "the data directory was already chosen: " + e);
        }
        main = new Handler(Looper.getMainLooper());
        HandlerThread thread = new HandlerThread("spatimesh-pictures");
        thread.start();
        pictures = new Handler(thread.getLooper());
        new Thread(() -> serve(name), "spatimesh-wire").start();
    }

    /** What the private profile kept, from a run that ended without cleaning up after itself. */
    private void forgetPrivate() {
        for (File dir : new File[] {getDataDir(), getCacheDir()}) {
            File[] all = dir == null ? null : dir.listFiles();
            if (all == null)
                continue;
            for (File f : all)
                if (f.getName().contains("spatimesh-private"))
                    delete(f);
        }
        File webview = new File(getCacheDir(), "WebView");
        File[] all = webview.listFiles();
        if (all != null)
            for (File f : all)
                if (f.getName().contains("spatimesh-private"))
                    delete(f);
    }

    private static void delete(File f) {
        File[] inside = f.listFiles();
        if (inside != null)
            for (File c : inside)
                delete(c);
        f.delete();
    }

    // ---- the wire ----

    private void send(byte[] message) {
        synchronized (sending) {
            try {
                if (out != null)
                    out.write(message);
            } catch (IOException e) {
                Log.w(TAG, "the window has gone: " + e);
            }
        }
    }

    private void sendWithFds(byte[] message, FileDescriptor[] fds) {
        synchronized (sending) {
            try {
                socket.setFileDescriptorsForSend(fds);
                out.write(message);
                socket.setFileDescriptorsForSend(null);
            } catch (IOException e) {
                Log.w(TAG, "the window has gone: " + e);
            }
        }
    }

    private static void readFully(InputStream in, byte[] into) throws IOException {
        int done = 0;
        while (done < into.length) {
            int n = in.read(into, done, into.length - done);
            if (n < 0)
                throw new IOException("closed");
            done += n;
        }
    }

    private void serve(String name) {
        try {
            socket = new LocalSocket();
            socket.connect(new LocalSocketAddress(name, LocalSocketAddress.Namespace.ABSTRACT));
            out = socket.getOutputStream();
            PackageInfo webview = WebView.getCurrentWebViewPackage();
            send(Wire.hello("Android System WebView " + (webview != null ? webview.versionName : "")));
            InputStream in = socket.getInputStream();
            byte[] head = new byte[5];
            while (true) {
                readFully(in, head);
                int length = (head[1] & 0xff) | (head[2] & 0xff) << 8 | (head[3] & 0xff) << 16 | (head[4] & 0xff) << 24;
                byte[] body = new byte[length];
                readFully(in, body);
                Wire.ToHost message = Wire.decode(head[0] & 0xff, body);
                if (message == null)
                    continue;
                if (message.tag == Wire.QUIT)
                    break;
                main.post(() -> handle(message));
            }
        } catch (IOException e) {
            Log.i(TAG, "the window has gone: " + e);
        }
        // Nothing to draw for: this process, and for the private profile all it kept, go.
        main.post(this::end);
    }

    private void end() {
        for (Page page : pages.values())
            close(page);
        pages.clear();
        if (isPrivate && started) {
            CookieManager.getInstance().removeAllCookies(null);
            WebStorage.getInstance().deleteAllData();
            forgetPrivate();
        }
        android.os.Process.killProcess(android.os.Process.myPid());
    }

    /** SpatiMesh's own script for every frame's every document (its media), or none. */
    private String pageScript;
    /** Every page the profile visited, on any machine (Chrome.getVisitedHistory). */
    private final java.util.Set<String> visited = new java.util.HashSet<>();

    /** Cookies and visited pages, the same on every machine (docs/spatimesh.md, "Logins and
     *  history on every machine"). WebView cannot list its cookies: it is asked what it would
     *  send to each address, and the window makes the cookies out of that. */
    private void kept(Wire.ToHost m) {
        android.webkit.CookieManager jar = android.webkit.CookieManager.getInstance();
        try {
            if (m.tag == Wire.ASK_COOKIES) {
                JSONArray addresses = new JSONArray(m.json), sent = new JSONArray();
                for (int i = 0; i < addresses.length(); i++) {
                    String url = addresses.getString(i);
                    String header = jar.getCookie(url);
                    sent.put(new JSONArray().put(url).put(header == null ? "" : header));
                }
                send(Wire.cookiesSent(sent.toString()));
            } else if (m.tag == Wire.SET_COOKIES) {
                JSONObject both = new JSONObject(m.json);
                JSONArray set = both.optJSONArray("set"), remove = both.optJSONArray("remove");
                for (int i = 0; set != null && i < set.length(); i++)
                    setCookie(jar, set.getJSONObject(i), false);
                for (int i = 0; remove != null && i < remove.length(); i++)
                    setCookie(jar, remove.getJSONObject(i), true);
                jar.flush();
            } else {
                JSONArray urls = new JSONArray(m.json);
                synchronized (visited) {
                    for (int i = 0; i < urls.length(); i++)
                        visited.add(urls.getString(i));
                }
            }
        } catch (JSONException e) {
            Log.w(TAG, "cookies or visited pages not understood: " + e);
        }
    }

    /** A cookie made, replaced, or (`gone`) removed, as a Set-Cookie header from its own address. */
    private static void setCookie(android.webkit.CookieManager jar, JSONObject c, boolean gone) throws JSONException {
        String domain = c.getString("domain"), path = c.optString("path", "/");
        boolean secure = c.optBoolean("secure");
        StringBuilder header = new StringBuilder(c.getString("name")).append('=').append(gone ? "" : c.optString("value"));
        if (domain.startsWith("."))
            header.append("; Domain=").append(domain);
        header.append("; Path=").append(path);
        if (gone) {
            header.append("; Max-Age=0");
        } else if (c.has("expires")) {
            long seconds = (long) c.getDouble("expires") - System.currentTimeMillis() / 1000;
            header.append("; Max-Age=").append(Math.max(seconds, 0));
        }
        if (secure)
            header.append("; Secure");
        if (c.optBoolean("http_only"))
            header.append("; HttpOnly");
        String same = c.optString("same_site", "");
        if (!same.isEmpty())
            header.append("; SameSite=").append(same);
        String host = domain.startsWith(".") ? domain.substring(1) : domain;
        jar.setCookie((secure ? "https://" : "http://") + host + path, header.toString());
    }

    private void handle(Wire.ToHost m) {
        if (m.tag == Wire.OPEN) {
            open(m.tab, m.url, m.width, m.height, m.zoom);
            return;
        }
        if (m.tag == Wire.EXTENSIONS) {
            extensions(m.set);
            return;
        }
        if (m.tag == Wire.RUN_SCRIPT) {
            runScript(pages.get(m.tab), m);
            return;
        }
        if (m.tag == Wire.ASK_COOKIES || m.tag == Wire.SET_COOKIES || m.tag == Wire.VISITED) {
            kept(m);
            return;
        }
        if (m.tag == Wire.PAGE_SCRIPT) {
            // SpatiMesh's media script, for every frame's every document from now on.
            pageScript = m.script == null || m.script.isEmpty() ? null : m.script;
            for (Page each : pages.values())
                applyExtensions(each);
            return;
        }
        Page page = pages.get(m.tab);
        if (page == null)
            return;
        switch (m.tag) {
            case Wire.CLOSE: close(page); pages.remove(m.tab); break;
            case Wire.LOAD: if (live(page)) page.view.loadUrl(m.url); break;
            case Wire.BACK: if (live(page)) page.view.goBack(); break;
            case Wire.FORWARD: if (live(page)) page.view.goForward(); break;
            case Wire.RELOAD: reload(page); break;
            case Wire.STOP: if (live(page)) page.view.stopLoading(); break;
            case Wire.RESIZE: resize(page, m.width, m.height, m.zoom); break;
            case Wire.POINTER: pointer(page, m.x, m.y, m.button, m.pressed, m.modifiers); break;
            case Wire.POINTER_LEFT: pointer(page, -1, -1, 0, false, 0); break;
            case Wire.SCROLL: scroll(page, m.x, m.y, m.dx, m.dy, m.modifiers); break;
            case Wire.KEY: key(page, m); break;
            case Wire.FOCUS: if (live(page) && m.focused) page.view.requestFocus(); break;
            case Wire.RELEASE: release(page, m.buffer); break;
            case Wire.CHOOSE:
                if (live(page))
                    page.view.evaluateJavascript("window.__spatimeshChoose && window.__spatimeshChoose(" + m.index + ")", null);
                break;
            case Wire.ANSWER: answer(page, m.accepted, m.hasText ? m.text : null); break;
            default: break;
        }
    }

    private static boolean live(Page page) {
        return page.view != null && !page.dead;
    }

    // ---- tabs ----

    private void open(long tab, String url, int width, int height, float zoom) {
        if (pages.containsKey(tab))
            return;
        Page page = new Page(tab);
        pages.put(tab, page);
        page.width = Math.max(width, 1);
        page.height = Math.max(height, 1);
        page.zoom = zoom > 0.1f ? zoom : 1f;
        show(page);
        page.view.loadUrl(url == null || url.isEmpty() ? "about:blank" : url);
    }

    /** A display of the tab's own, its WebView on it, its pictures into the ImageReader. */
    private void show(Page page) {
        DisplayManager displays = getSystemService(DisplayManager.class);
        page.reader = ImageReader.newInstance(page.width, page.height, PixelFormat.RGBA_8888, 3);
        page.reader.setOnImageAvailableListener(reader -> picture(page, reader), pictures);
        // The page's zoom as the display's density: 160 dots an inch is one CSS pixel a pixel.
        int dpi = Math.round(160 * page.zoom);
        page.display = displays.createVirtualDisplay("spatimesh-" + page.tab, page.width, page.height, dpi,
                page.reader.getSurface(), DisplayManager.VIRTUAL_DISPLAY_FLAG_OWN_CONTENT_ONLY);
        page.presentation = new Presentation(this, page.display.getDisplay());
        WebView view = new WebView(page.presentation.getContext());
        WebSettings settings = view.getSettings();
        settings.setJavaScriptEnabled(true);
        settings.setDomStorageEnabled(true);
        settings.setSupportMultipleWindows(true);
        settings.setJavaScriptCanOpenWindowsAutomatically(false);
        settings.setMediaPlaybackRequiresUserGesture(true);
        settings.setBuiltInZoomControls(false);
        settings.setUserAgentString(desktopAgent(settings.getUserAgentString()));
        if (isPrivate)
            settings.setCacheMode(WebSettings.LOAD_NO_CACHE);
        view.setWebViewClient(new Client(page));
        view.setWebChromeClient(new Chrome(page));
        view.addJavascriptInterface(new SelectBridge(page), "spatimeshSelect");
        view.addJavascriptInterface(new MediaBridge(page), "spatimeshMedia");
        page.presentation.setContentView(view);
        page.presentation.show();
        page.view = view;
        page.dead = false;
        // A new view: the extensions' scripts and listener on it, from its first page on.
        page.provider = null;
        page.starts.clear();
        page.listening = false;
        page.frames.clear();
        applyExtensions(page);
    }

    private void close(Page page) {
        if (page.view != null)
            page.view.destroy();
        if (page.presentation != null)
            page.presentation.dismiss();
        if (page.display != null)
            page.display.release();
        if (page.reader != null)
            page.reader.close();
        synchronized (page) {
            if (page.waiting != null)
                page.waiting.close();
            page.waiting = null;
        }
        page.view = null;
    }

    private void reload(Page page) {
        if (page.dead) {
            // The renderer went; the WebView with it. A new one, at the same address.
            String url = page.view != null ? page.view.getUrl() : null;
            close(page);
            show(page);
            page.view.loadUrl(url != null ? url : "about:blank");
        } else if (page.view != null) {
            page.view.reload();
        }
    }

    private void resize(Page page, int width, int height, float zoom) {
        page.width = Math.max(width, 1);
        page.height = Math.max(height, 1);
        page.zoom = zoom > 0.1f ? zoom : page.zoom;
        ImageReader old = page.reader;
        page.reader = ImageReader.newInstance(page.width, page.height, PixelFormat.RGBA_8888, 3);
        page.reader.setOnImageAvailableListener(reader -> picture(page, reader), pictures);
        page.display.resize(page.width, page.height, Math.round(160 * page.zoom));
        page.display.setSurface(page.reader.getSurface());
        if (old != null)
            old.close();
    }

    // ---- pictures ----

    /** A new picture on the tab's display: copied into a buffer the window is not reading. */
    private void picture(Page page, ImageReader reader) {
        Image image;
        try {
            image = reader.acquireLatestImage();
        } catch (IllegalStateException e) {
            return;
        }
        if (image == null)
            return;
        synchronized (page) {
            if (page.waiting != null)
                page.waiting.close();
            page.waiting = image;
        }
        deliver(page);
    }

    private void deliver(Page page) {
        Image image;
        int buffer;
        synchronized (page) {
            image = page.waiting;
            if (image == null)
                return;
            int width = image.getWidth(), height = image.getHeight();
            if (page.bufferWidth != width || page.bufferHeight != height || page.buffers[0] == null) {
                try {
                    for (int i = 0; i < 2; i++) {
                        FileDescriptor fd = Os.memfd_create("spatimesh-picture", 0);
                        Os.ftruncate(fd, (long) width * height * 4);
                        page.buffers[i] = fd;
                        page.held[i] = false;
                    }
                } catch (ErrnoException e) {
                    Log.w(TAG, "no memory for a picture: " + e);
                    return;
                }
                page.bufferWidth = width;
                page.bufferHeight = height;
                page.newest = -1;
                sendWithFds(Wire.buffers(page.tab, width, height, true, 2), page.buffers);
            }
            buffer = page.newest == 0 ? 1 : 0;
            if (page.held[buffer])
                buffer = 1 - buffer;
            if (page.held[buffer])
                return; // Both with the window: the picture waits for one to come back.
            page.held[buffer] = true;
            page.waiting = null;
        }
        try {
            Image.Plane plane = image.getPlanes()[0];
            ByteBuffer pixels = plane.getBuffer();
            int stride = plane.getRowStride(), width = image.getWidth(), height = image.getHeight(), row = width * 4;
            FileDescriptor fd = page.buffers[buffer];
            // Through Buffer: ByteBuffer's own position(int) and limit(int) are newer than Android.
            Buffer view = pixels;
            if (stride == row) {
                view.limit(row * height);
                view.position(0);
                long at = 0;
                while (pixels.hasRemaining())
                    at += Os.pwrite(fd, pixels, at);
            } else {
                for (int y = 0; y < height; y++) {
                    view.limit(y * stride + row);
                    view.position(y * stride);
                    long at = (long) y * row;
                    while (pixels.hasRemaining())
                        at += Os.pwrite(fd, pixels, at);
                }
            }
            synchronized (page) {
                page.newest = buffer;
            }
            send(Wire.picture(page.tab, buffer, width, height));
        } catch (ErrnoException | InterruptedIOException | IllegalStateException e) {
            Log.w(TAG, "a picture could not be copied: " + e);
            synchronized (page) {
                page.held[buffer] = false;
            }
        } finally {
            image.close();
        }
    }

    private void release(Page page, int buffer) {
        synchronized (page) {
            if (buffer >= 0 && buffer < 2)
                page.held[buffer] = false;
        }
        pictures.post(() -> deliver(page));
    }

    // ---- input ----

    private static int meta(int modifiers) {
        int meta = 0;
        if ((modifiers & 1) != 0) meta |= KeyEvent.META_SHIFT_ON | KeyEvent.META_SHIFT_LEFT_ON;
        if ((modifiers & 2) != 0) meta |= KeyEvent.META_CTRL_ON | KeyEvent.META_CTRL_LEFT_ON;
        if ((modifiers & 4) != 0) meta |= KeyEvent.META_ALT_ON | KeyEvent.META_ALT_LEFT_ON;
        if ((modifiers & 8) != 0) meta |= KeyEvent.META_META_ON | KeyEvent.META_META_LEFT_ON;
        return meta;
    }

    private static MotionEvent mouse(long down, int action, float x, float y, int buttons, int meta) {
        MotionEvent.PointerProperties[] properties = {new MotionEvent.PointerProperties()};
        properties[0].id = 0;
        properties[0].toolType = MotionEvent.TOOL_TYPE_MOUSE;
        MotionEvent.PointerCoords[] coords = {new MotionEvent.PointerCoords()};
        coords[0].x = x;
        coords[0].y = y;
        coords[0].pressure = buttons != 0 ? 1f : 0f;
        coords[0].size = 1f;
        return MotionEvent.obtain(down, SystemClock.uptimeMillis(), action, 1, properties, coords, meta, buttons, 1f, 1f, 0, 0,
                InputDevice.SOURCE_MOUSE, 0);
    }

    private void pointer(Page page, float x, float y, int button, boolean pressed, int modifiers) {
        if (!live(page))
            return;
        int meta = meta(modifiers);
        if (button == 4 || button == 5) {
            if (pressed) {
                if (button == 4) page.view.goBack();
                else page.view.goForward();
            }
            return;
        }
        if (button == 0) {
            if (x < 0) {
                MotionEvent exit = mouse(SystemClock.uptimeMillis(), MotionEvent.ACTION_HOVER_EXIT, 0, 0, 0, meta);
                page.view.dispatchGenericMotionEvent(exit);
                exit.recycle();
                return;
            }
            if (page.buttons != 0) {
                MotionEvent move = mouse(page.downTime, MotionEvent.ACTION_MOVE, x, y, page.buttons, meta);
                page.view.dispatchTouchEvent(move);
                move.recycle();
            } else {
                MotionEvent hover = mouse(SystemClock.uptimeMillis(), MotionEvent.ACTION_HOVER_MOVE, x, y, 0, meta);
                page.view.dispatchGenericMotionEvent(hover);
                hover.recycle();
            }
            return;
        }
        int bit = button == 3 ? MotionEvent.BUTTON_SECONDARY : button == 2 ? MotionEvent.BUTTON_TERTIARY : MotionEvent.BUTTON_PRIMARY;
        if (pressed) {
            boolean first = page.buttons == 0;
            page.buttons |= bit;
            if (first) {
                page.downTime = SystemClock.uptimeMillis();
                MotionEvent down = mouse(page.downTime, MotionEvent.ACTION_DOWN, x, y, page.buttons, meta);
                page.view.dispatchTouchEvent(down);
                down.recycle();
            }
        } else {
            page.buttons &= ~bit;
            if (page.buttons == 0) {
                MotionEvent up = mouse(page.downTime, MotionEvent.ACTION_UP, x, y, 0, meta);
                page.view.dispatchTouchEvent(up);
                up.recycle();
            }
        }
    }

    private void scroll(Page page, float x, float y, float dx, float dy, int modifiers) {
        if (!live(page))
            return;
        MotionEvent.PointerProperties[] properties = {new MotionEvent.PointerProperties()};
        properties[0].id = 0;
        properties[0].toolType = MotionEvent.TOOL_TYPE_MOUSE;
        MotionEvent.PointerCoords[] coords = {new MotionEvent.PointerCoords()};
        coords[0].x = x;
        coords[0].y = y;
        // A wheel's notch is about sixty-four pixels to WebView; up is positive.
        coords[0].setAxisValue(MotionEvent.AXIS_VSCROLL, -dy / 64f);
        coords[0].setAxisValue(MotionEvent.AXIS_HSCROLL, dx / 64f);
        long now = SystemClock.uptimeMillis();
        MotionEvent ev = MotionEvent.obtain(now, now, MotionEvent.ACTION_SCROLL, 1, properties, coords, meta(modifiers), 0, 1f, 1f, 0, 0,
                InputDevice.SOURCE_MOUSE, 0);
        page.view.dispatchGenericMotionEvent(ev);
        ev.recycle();
    }

    /** Linux's key numbers to Android's, for the keys a page cares about. */
    private static int keycode(int code) {
        switch (code) {
            case 1: return KeyEvent.KEYCODE_ESCAPE;
            case 2: return KeyEvent.KEYCODE_1; case 3: return KeyEvent.KEYCODE_2; case 4: return KeyEvent.KEYCODE_3;
            case 5: return KeyEvent.KEYCODE_4; case 6: return KeyEvent.KEYCODE_5; case 7: return KeyEvent.KEYCODE_6;
            case 8: return KeyEvent.KEYCODE_7; case 9: return KeyEvent.KEYCODE_8; case 10: return KeyEvent.KEYCODE_9;
            case 11: return KeyEvent.KEYCODE_0; case 12: return KeyEvent.KEYCODE_MINUS; case 13: return KeyEvent.KEYCODE_EQUALS;
            case 14: return KeyEvent.KEYCODE_DEL; case 15: return KeyEvent.KEYCODE_TAB;
            case 16: return KeyEvent.KEYCODE_Q; case 17: return KeyEvent.KEYCODE_W; case 18: return KeyEvent.KEYCODE_E;
            case 19: return KeyEvent.KEYCODE_R; case 20: return KeyEvent.KEYCODE_T; case 21: return KeyEvent.KEYCODE_Y;
            case 22: return KeyEvent.KEYCODE_U; case 23: return KeyEvent.KEYCODE_I; case 24: return KeyEvent.KEYCODE_O;
            case 25: return KeyEvent.KEYCODE_P; case 26: return KeyEvent.KEYCODE_LEFT_BRACKET; case 27: return KeyEvent.KEYCODE_RIGHT_BRACKET;
            case 28: return KeyEvent.KEYCODE_ENTER; case 29: return KeyEvent.KEYCODE_CTRL_LEFT;
            case 30: return KeyEvent.KEYCODE_A; case 31: return KeyEvent.KEYCODE_S; case 32: return KeyEvent.KEYCODE_D;
            case 33: return KeyEvent.KEYCODE_F; case 34: return KeyEvent.KEYCODE_G; case 35: return KeyEvent.KEYCODE_H;
            case 36: return KeyEvent.KEYCODE_J; case 37: return KeyEvent.KEYCODE_K; case 38: return KeyEvent.KEYCODE_L;
            case 39: return KeyEvent.KEYCODE_SEMICOLON; case 40: return KeyEvent.KEYCODE_APOSTROPHE; case 41: return KeyEvent.KEYCODE_GRAVE;
            case 42: return KeyEvent.KEYCODE_SHIFT_LEFT; case 43: return KeyEvent.KEYCODE_BACKSLASH;
            case 44: return KeyEvent.KEYCODE_Z; case 45: return KeyEvent.KEYCODE_X; case 46: return KeyEvent.KEYCODE_C;
            case 47: return KeyEvent.KEYCODE_V; case 48: return KeyEvent.KEYCODE_B; case 49: return KeyEvent.KEYCODE_N;
            case 50: return KeyEvent.KEYCODE_M; case 51: return KeyEvent.KEYCODE_COMMA; case 52: return KeyEvent.KEYCODE_PERIOD;
            case 53: return KeyEvent.KEYCODE_SLASH; case 54: return KeyEvent.KEYCODE_SHIFT_RIGHT; case 56: return KeyEvent.KEYCODE_ALT_LEFT;
            case 57: return KeyEvent.KEYCODE_SPACE; case 58: return KeyEvent.KEYCODE_CAPS_LOCK;
            case 59: return KeyEvent.KEYCODE_F1; case 60: return KeyEvent.KEYCODE_F2; case 61: return KeyEvent.KEYCODE_F3;
            case 62: return KeyEvent.KEYCODE_F4; case 63: return KeyEvent.KEYCODE_F5; case 64: return KeyEvent.KEYCODE_F6;
            case 65: return KeyEvent.KEYCODE_F7; case 66: return KeyEvent.KEYCODE_F8; case 67: return KeyEvent.KEYCODE_F9;
            case 68: return KeyEvent.KEYCODE_F10; case 87: return KeyEvent.KEYCODE_F11; case 88: return KeyEvent.KEYCODE_F12;
            case 97: return KeyEvent.KEYCODE_CTRL_RIGHT; case 100: return KeyEvent.KEYCODE_ALT_RIGHT;
            case 102: return KeyEvent.KEYCODE_MOVE_HOME; case 103: return KeyEvent.KEYCODE_DPAD_UP; case 104: return KeyEvent.KEYCODE_PAGE_UP;
            case 105: return KeyEvent.KEYCODE_DPAD_LEFT; case 106: return KeyEvent.KEYCODE_DPAD_RIGHT; case 107: return KeyEvent.KEYCODE_MOVE_END;
            case 108: return KeyEvent.KEYCODE_DPAD_DOWN; case 109: return KeyEvent.KEYCODE_PAGE_DOWN; case 110: return KeyEvent.KEYCODE_INSERT;
            case 111: return KeyEvent.KEYCODE_FORWARD_DEL; case 125: return KeyEvent.KEYCODE_META_LEFT; case 126: return KeyEvent.KEYCODE_META_RIGHT;
            default: return KeyEvent.KEYCODE_UNKNOWN;
        }
    }

    private void key(Page page, Wire.ToHost m) {
        if (!live(page))
            return;
        long now = SystemClock.uptimeMillis();
        int code = keycode(m.code);
        if (code == KeyEvent.KEYCODE_UNKNOWN) {
            // A key Android has no number for: what it typed, as text.
            if (m.pressed && m.hasText && !m.text.isEmpty()) {
                KeyEvent typed = new KeyEvent(now, m.text, KeyCharacterMap.VIRTUAL_KEYBOARD, 0);
                page.view.dispatchKeyEvent(typed);
            }
            return;
        }
        KeyEvent ev = new KeyEvent(now, now, m.pressed ? KeyEvent.ACTION_DOWN : KeyEvent.ACTION_UP, code, m.repeat ? 1 : 0,
                meta(m.modifiers), KeyCharacterMap.VIRTUAL_KEYBOARD, m.code, KeyEvent.FLAG_FROM_SYSTEM, InputDevice.SOURCE_KEYBOARD);
        page.view.dispatchKeyEvent(ev);
    }

    private void answer(Page page, boolean accepted, String text) {
        if (page.prompt != null) {
            if (accepted) page.prompt.confirm(text != null ? text : "");
            else page.prompt.cancel();
            page.prompt = null;
        }
        if (page.result != null) {
            if (accepted) page.result.confirm();
            else page.result.cancel();
            page.result = null;
        }
    }

    // ---- extensions (docs/spatimesh.md, "Extensions") ----
    //
    // The window's runtime answers every extension API; here is only the bridge. Android's
    // WebView gives a page no world but its own, and no hooks but what its support library has:
    // each extension's script (made by the window: crates/spatimesh/src/extensions/android.js)
    // at the start of every frame's every page, a listener (`spatimeshExt`) in every frame that
    // its contexts speak to, the top frame reached by evaluateJavascript through the extension's
    // way in, other frames by their listener's reply. Each extension is at
    // https://<id>.spatimesh-extension.invalid, answered here and never by the network.

    /** The profile's extensions, as the window gives them: on every tab's view from its next page on. */
    private void extensions(String json) {
        try {
            JSONObject set = new JSONObject(json);
            extensions = set;
            rules = rulesOf(set.optJSONObject("page_start"));
        } catch (JSONException e) {
            Log.w(TAG, "the window's extensions could not be read", e);
            return;
        }
        for (Page page : pages.values())
            applyExtensions(page);
    }

    private JSONObject pageStart() {
        JSONObject set = extensions;
        return set == null ? null : set.optJSONObject("page_start");
    }

    /** The extensions' scripts on a tab's view in place of what was there, and the listener
     *  their contexts speak to. */
    private void applyExtensions(Page page) {
        if (!live(page))
            return;
        if (!boundaryLooked) {
            boundaryLooked = true;
            boundary = Boundary.find();
        }
        if (boundary == null)
            return;
        try {
            if (page.provider == null)
                page.provider = boundary.provider(page.view);
            for (Object script : page.starts)
                boundary.remove(script);
            page.starts.clear();
            // SpatiMesh's media script first, in every frame; the top one given its way out, kept
            // before the page could replace the bridge's name.
            if (pageScript != null)
                page.starts.add(boundary.addDocumentStart(page.provider,
                        "if(window.top===window){(function(){var b=window.spatimeshMedia;"
                        + "if(b){window.__spatimeshMediaPost=function(s){b.post(s);};}})();}\n" + pageScript));
            JSONObject start = pageStart();
            JSONArray scripts = start == null ? null : start.optJSONArray("scripts");
            if (scripts == null || scripts.length() == 0)
                return;
            // The listener first: each script takes it as it starts.
            if (!page.listening) {
                boundary.addListener(page.provider, "spatimeshExt", boundary.listener((text, top, reply) -> said(page, text, top, reply)));
                page.listening = true;
            }
            for (int i = 0; i < scripts.length(); i++) {
                JSONArray one = scripts.optJSONArray(i);
                if (one != null)
                    page.starts.add(boundary.addDocumentStart(page.provider, one.optString(1)));
            }
        } catch (Throwable e) {
            Log.w(TAG, "the extensions could not be put on a tab", e);
        }
    }

    /** An extension's context speaking (api.js's JSON): to the window; a frame other than the
     *  top one kept, to be answered. */
    private void said(Page page, String text, boolean top, Object reply) {
        if (text == null)
            return;
        if (!top && reply != null) {
            try {
                JSONObject call = new JSONObject(text);
                String ext = call.optString("e");
                String world = "content".equals(call.optString("k")) ? "spatimesh-" + ext : "";
                Map<Long, Object[]> of = page.frames.get(world);
                if (of == null) {
                    of = new HashMap<>();
                    page.frames.put(world, of);
                }
                long frame = call.optLong("f");
                if ("gone".equals(call.optString("c")))
                    of.remove(frame);
                else
                    of.put(frame, new Object[] {ext, reply});
            } catch (JSONException e) {
                return;
            }
        }
        send(Wire.extension(page.tab, text));
    }

    /** Code from the window: in the top frame, in the page's own world or through the
     *  extension's way in (its API made first if asked); and, where it is a delivery, to the
     *  other frames the extension's contexts speak from, as their message. Code itself reaches
     *  no frame but the top one here. */
    private void runScript(Page page, Wire.ToHost m) {
        if (page == null || !live(page)) {
            if (m.request != 0)
                send(Wire.scriptResult(m.tab, m.request, false, "No such tab"));
            return;
        }
        JSONObject start = pageStart();
        String run = m.script;
        if (!m.world.isEmpty()) {
            if (start == null) {
                if (m.request != 0)
                    send(Wire.scriptResult(m.tab, m.request, false, "No extensions here"));
                return;
            }
            String ext = m.world.startsWith("spatimesh-") ? m.world.substring("spatimesh-".length()) : m.world;
            run = "(function(w){return typeof w==='function'?w(" + JSONObject.quote(start.optString("secret")) + ",function(globalThis){" + m.script
                    + "\n}," + m.boot + "):null;})(window[" + JSONObject.quote(start.optString("entry") + ext) + "])";
        }
        long tab = page.tab, request = m.request;
        page.view.evaluateJavascript(run, request == 0 ? null : value -> send(Wire.scriptResult(tab, request, true, value == null ? "null" : value)));
        if (m.allFrames && start != null)
            deliverToFrames(page, m.world, m.script, start.optString("deliver_prefix"));
    }

    private void deliverToFrames(Page page, String world, String code, String prefix) {
        Map<Long, Object[]> of = page.frames.get(world);
        if (boundary == null || of == null || of.isEmpty() || prefix.isEmpty() || !code.startsWith(prefix) || !code.endsWith(")"))
            return;
        Object payload;
        try {
            payload = new JSONTokener(code.substring(prefix.length(), code.length() - 1)).nextValue();
        } catch (JSONException e) {
            return;
        }
        if (!(payload instanceof String))
            return;
        for (Object[] frame : of.values()) {
            try {
                boundary.reply(frame[1], new JSONObject().put("e", frame[0]).put("d", payload).toString());
            } catch (JSONException e) {
                // A string and a string: never.
            }
        }
    }

    private static final String DOMAIN = ".spatimesh-extension.invalid";
    private static final Pattern MESSAGE = Pattern.compile("__MSG_([A-Za-z0-9_@]+)__");

    /** A file of an extension's, from its folder, for the page that asked (its Referer or
     *  Origin): only what it made web accessible, to a page not its own. Called on WebView's own
     *  threads; null for every address but the extensions'. */
    WebResourceResponse serveExtension(WebResourceRequest request) {
        return serveExtension(request.getUrl(), request.getRequestHeaders());
    }

    WebResourceResponse serveExtension(android.net.Uri url, Map<String, String> headers) {
        String host = url.getHost();
        if (!"https".equals(url.getScheme()) || host == null || !host.toLowerCase(Locale.ROOT).endsWith(DOMAIN))
            return null;
        JSONObject set = extensions;
        JSONObject start = set == null ? null : set.optJSONObject("page_start");
        String id = host.substring(0, host.length() - DOMAIN.length()).toLowerCase(Locale.ROOT);
        JSONArray all = set == null ? null : set.optJSONArray("extensions");
        JSONObject spec = null;
        for (int i = 0; all != null && i < all.length(); i++) {
            JSONObject one = all.optJSONObject(i);
            if (one != null && id.equals(one.optString("id")))
                spec = one;
        }
        if (spec == null || start == null)
            return refused(404, "Not Found");
        String path = url.getPath() == null ? "" : url.getPath();
        while (path.startsWith("/"))
            path = path.substring(1);
        if (path.isEmpty())
            path = "index.html";
        String initiator = headers == null ? null : headers.get("Referer");
        if ((initiator == null || initiator.isEmpty()) && headers != null)
            initiator = headers.get("Origin");
        if (initiator != null && !initiator.isEmpty() && !initiator.startsWith(spec.optString("origin")) && !initiator.startsWith("about:")
                && !initiator.equals("null")) {
            // An Origin header is the page's origin alone: its patterns are for a path.
            int scheme = initiator.indexOf("//");
            if (scheme >= 0 && initiator.indexOf('/', scheme + 2) < 0)
                initiator += "/";
            if (!webAccessible(start, id, path, initiator))
                return refused(403, "Forbidden");
        }
        String mime = mimeOf(path);
        byte[] bytes;
        if (path.equals(start.optString("background_page")) && !spec.isNull("background_html")) {
            bytes = spec.optString("background_html").getBytes(StandardCharsets.UTF_8);
        } else if (path.equals(start.optString("worker_path"))) {
            bytes = start.optString("worker").getBytes(StandardCharsets.UTF_8);
        } else {
            try {
                File root = new File(spec.optString("root")).getCanonicalFile();
                File file = new File(root, path).getCanonicalFile();
                if (!file.getPath().startsWith(root.getPath() + File.separator) || !file.isFile())
                    return refused(404, "Not Found");
                bytes = java.nio.file.Files.readAllBytes(file.toPath());
            } catch (IOException e) {
                return refused(404, "Not Found");
            }
            // Style sheets are written with __MSG_name__, as in Chrome.
            if (mime.equals("text/css"))
                bytes = localized(spec, new String(bytes, StandardCharsets.UTF_8)).getBytes(StandardCharsets.UTF_8);
        }
        Map<String, String> out = new HashMap<>();
        out.put("Access-Control-Allow-Origin", "*");
        out.put("Cache-Control", "no-cache");
        boolean text = mime.startsWith("text/") || mime.equals("application/json") || mime.equals("application/xml") || mime.equals("image/svg+xml");
        return new WebResourceResponse(mime, text ? "utf-8" : null, 200, "OK", out, new ByteArrayInputStream(bytes));
    }

    private static WebResourceResponse refused(int status, String why) {
        return new WebResourceResponse("text/plain", "utf-8", status, why, new HashMap<>(), new ByteArrayInputStream(new byte[0]));
    }

    /** Whether `page` may have `path` of extension `id`: its web-accessible resources, as the
     *  window gives them (regular expressions of its paths, and of the pages they are for). */
    private static boolean webAccessible(JSONObject start, String id, String path, String page) {
        JSONArray all = start.optJSONArray("web_accessible");
        for (int i = 0; all != null && i < all.length(); i++) {
            JSONArray one = all.optJSONArray(i);
            if (one == null || !id.equals(one.optString(0)))
                continue;
            JSONArray entries = one.optJSONArray(1);
            for (int j = 0; entries != null && j < entries.length(); j++) {
                JSONArray entry = entries.optJSONArray(j);
                if (entry == null)
                    continue;
                String paths = entry.optString(0), pages = entry.optString(1);
                if (!paths.isEmpty() && Pattern.compile(paths).matcher(path).find() && (pages.isEmpty() || Pattern.compile(pages).matcher(page).find()))
                    return true;
            }
        }
        return false;
    }

    /** `__MSG_name__` in a style sheet as the extension's message. */
    private static String localized(JSONObject spec, String css) {
        JSONObject messages = spec.optJSONObject("messages");
        if (messages == null || messages.length() == 0)
            return css;
        Matcher m = MESSAGE.matcher(css);
        StringBuffer out = new StringBuffer();
        while (m.find())
            m.appendReplacement(out, Matcher.quoteReplacement(messages.optString(m.group(1).toLowerCase(Locale.ROOT), "")));
        m.appendTail(out);
        return out.toString();
    }

    /** As crates/spatimesh/src/extensions/set.rs's mime_of. */
    private static String mimeOf(String path) {
        int dot = path.lastIndexOf('.');
        switch (dot < 0 ? "" : path.substring(dot + 1).toLowerCase(Locale.ROOT)) {
            case "html": case "htm": return "text/html";
            case "js": case "mjs": return "text/javascript";
            case "css": return "text/css";
            case "json": return "application/json";
            case "png": return "image/png";
            case "jpg": case "jpeg": return "image/jpeg";
            case "gif": return "image/gif";
            case "svg": return "image/svg+xml";
            case "webp": return "image/webp";
            case "ico": return "image/x-icon";
            case "woff": return "font/woff";
            case "woff2": return "font/woff2";
            case "ttf": return "font/ttf";
            case "otf": return "font/otf";
            case "wasm": return "application/wasm";
            case "txt": return "text/plain";
            case "xml": return "application/xml";
            case "mp3": return "audio/mpeg";
            case "ogg": case "oga": return "audio/ogg";
            case "wav": return "audio/wav";
            case "mp4": return "video/mp4";
            case "webm": return "video/webm";
            default: return "application/octet-stream";
        }
    }

    // ---- declarativeNetRequest ----
    //
    // The WebView has no rules of its own to be given: each request it lets shouldInterceptRequest
    // see is matched here (Dnr.java) against the rules the window gives with the set. Blocking is
    // answering with a failure; a redirect of a subresource is answering with what the address
    // it goes to answers (the extension's own file, or a GET made here), as the WebView takes no
    // redirect from shouldInterceptRequest; a redirect of the top frame is a new load.

    private static Dnr rulesOf(JSONObject start) {
        JSONArray all = start == null ? null : start.optJSONArray("rules");
        if (all == null || all.length() == 0)
            return null;
        Dnr dnr = new Dnr();
        for (int i = 0; i < all.length(); i++) {
            JSONObject ext = all.optJSONObject(i);
            JSONArray list = ext == null ? null : ext.optJSONArray("rules");
            if (list == null)
                continue;
            List<Dnr.Fields> fields = new ArrayList<>();
            for (int j = 0; j < list.length(); j++) {
                JSONObject rule = list.optJSONObject(j);
                if (rule != null)
                    fields.add(fieldsOf(rule));
            }
            dnr.add(ext.optString("id"), fields);
        }
        return dnr.isEmpty() ? null : dnr;
    }

    private static Dnr.Fields fieldsOf(JSONObject o) {
        return new Dnr.Fields() {
            @Override
            public String text(String key) {
                return o.has(key) && !o.isNull(key) ? String.valueOf(o.opt(key)) : null;
            }

            @Override
            public long number(String key, long otherwise) {
                return o.optLong(key, otherwise);
            }

            @Override
            public boolean flag(String key) {
                return o.optBoolean(key, false);
            }

            @Override
            public List<String> list(String key) {
                JSONArray a = o.optJSONArray(key);
                if (a == null)
                    return null;
                List<String> out = new ArrayList<>();
                for (int i = 0; i < a.length(); i++)
                    out.add(a.optString(i));
                return out;
            }
        };
    }

    private static String header(Map<String, String> headers, String name) {
        if (headers == null)
            return null;
        for (Map.Entry<String, String> h : headers.entrySet())
            if (name.equalsIgnoreCase(h.getKey()) && h.getValue() != null && !h.getValue().isEmpty())
                return h.getValue();
        return null;
    }

    /** What the extensions' rules make of a request a page of `page`'s makes: null to let it
     *  go to the network as it is. Called on WebView's own threads. */
    WebResourceResponse ruled(Page page, WebResourceRequest request) {
        Dnr dnr = rules;
        String url = request.getUrl().toString();
        if (dnr == null || !(url.startsWith("http:") || url.startsWith("https:")))
            return null;
        Map<String, String> headers = request.getRequestHeaders();
        boolean top = request.isForMainFrame();
        String initiator = null;
        if (!top) {
            initiator = header(headers, "Referer");
            if (initiator == null)
                initiator = header(headers, "Origin");
            if (initiator == null || initiator.equals("null"))
                initiator = page.url;
        }
        String method = request.getMethod() == null ? "get" : request.getMethod().toLowerCase(Locale.ROOT);
        Dnr.Decision d = dnr.decide(url, initiator, Dnr.typeOf(top, url, headers), method);
        if (d == null)
            return null;
        if (d.block)
            return blocked();
        String to = d.to;
        if (top) {
            main.post(() -> {
                if (live(page))
                    page.view.loadUrl(to);
            });
            return refused(204, "No Content");
        }
        android.net.Uri target = android.net.Uri.parse(to);
        WebResourceResponse own = serveExtension(target, headers);
        if (own != null)
            return own;
        return method.equals("get") ? fetched(to, headers) : null;
    }

    /** A failure for a blocked request: nothing can be read from it, so the WebView fails the
     *  request as a network error does (a page's fetch rejects). */
    private static WebResourceResponse blocked() {
        InputStream failing = new InputStream() {
            @Override
            public int read() throws IOException {
                throw new IOException("blocked by an extension's rules");
            }

            @Override
            public int available() throws IOException {
                throw new IOException("blocked by an extension's rules");
            }
        };
        return new WebResourceResponse("text/plain", "utf-8", 200, "OK", new HashMap<>(), failing);
    }

    /** What `to` answers a GET with the page's headers and cookies: a subresource's redirect. */
    private static WebResourceResponse fetched(String to, Map<String, String> headers) {
        try {
            HttpURLConnection c = (HttpURLConnection) new URL(to).openConnection();
            c.setInstanceFollowRedirects(true);
            if (headers != null)
                for (Map.Entry<String, String> h : headers.entrySet())
                    if (h.getKey() != null && !h.getKey().equalsIgnoreCase("Host") && !h.getKey().equalsIgnoreCase("Accept-Encoding"))
                        c.setRequestProperty(h.getKey(), h.getValue());
            String cookies = CookieManager.getInstance().getCookie(to);
            if (cookies != null)
                c.setRequestProperty("Cookie", cookies);
            int status = c.getResponseCode();
            InputStream body = status >= 400 ? c.getErrorStream() : c.getInputStream();
            String type = c.getContentType() == null ? "application/octet-stream" : c.getContentType();
            String mime = type, charset = null;
            int semi = type.indexOf(';');
            if (semi >= 0) {
                mime = type.substring(0, semi).trim();
                Matcher m = Pattern.compile("charset=\"?([^\";]+)").matcher(type);
                if (m.find())
                    charset = m.group(1);
            }
            Map<String, String> out = new HashMap<>();
            for (Map.Entry<String, List<String>> h : c.getHeaderFields().entrySet())
                if (h.getKey() != null && !h.getValue().isEmpty())
                    out.put(h.getKey(), String.join(", ", h.getValue()));
            String reason = c.getResponseMessage() == null || c.getResponseMessage().isEmpty() ? "OK" : c.getResponseMessage();
            if (status < 200 || (status >= 300 && status < 400)) {
                status = 502;
                reason = "Bad Gateway";
            }
            return new WebResourceResponse(mime, charset, status, reason, out, body == null ? new ByteArrayInputStream(new byte[0]) : body);
        } catch (IOException | RuntimeException e) {
            Log.w(TAG, "a redirect by an extension's rules could not be followed: " + to, e);
            return blocked();
        }
    }

    // ---- what pages do ----

    final class Client extends WebViewClient {
        final Page page;

        Client(Page page) {
            this.page = page;
        }

        @Override
        public void onPageStarted(WebView view, String url, Bitmap favicon) {
            // A new page in the top frame: the frames of the one before are gone.
            page.frames.clear();
            page.url = url;
            send(Wire.url(page.tab, url));
            send(Wire.progress(page.tab, 0.1f, true));
        }

        @Override
        public WebResourceResponse shouldInterceptRequest(WebView view, WebResourceRequest request) {
            WebResourceResponse own = serveExtension(request);
            return own != null ? own : ruled(page, request);
        }

        @Override
        public void onPageFinished(WebView view, String url) {
            view.evaluateJavascript(SELECT_SCRIPT, null);
            send(Wire.progress(page.tab, 1f, false));
            send(Wire.history(page.tab, view.canGoBack(), view.canGoForward()));
        }

        @Override
        public void doUpdateVisitedHistory(WebView view, String url, boolean reload) {
            send(Wire.url(page.tab, url));
            send(Wire.history(page.tab, view.canGoBack(), view.canGoForward()));
        }

        @Override
        public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
            String scheme = request.getUrl().getScheme();
            // Only what a page can show: an intent:, market:, tel: link is not for a page.
            return scheme == null || !(scheme.equals("http") || scheme.equals("https") || scheme.equals("about")
                    || scheme.equals("data") || scheme.equals("file") || scheme.equals("blob") || scheme.equals("javascript"));
        }

        @Override
        public void onReceivedError(WebView view, WebResourceRequest request, WebResourceError error) {
            if (request.isForMainFrame())
                send(Wire.failed(page.tab, request.getUrl().toString(), String.valueOf(error.getDescription())));
        }

        @Override
        public boolean onRenderProcessGone(WebView view, RenderProcessGoneDetail detail) {
            // Every page of this profile shares the renderer: each is told here, in turn.
            page.dead = true;
            send(Wire.crashed(page.tab, detail.didCrash() ? "The pages' process crashed" : "The pages' process was ended to free memory"));
            return true;
        }
    }

    final class Chrome extends WebChromeClient {
        final Page page;

        Chrome(Page page) {
            this.page = page;
        }

        /** Pages visited here and on the profile's other machines: their links look visited. */
        @Override
        public void getVisitedHistory(android.webkit.ValueCallback<String[]> callback) {
            synchronized (visited) {
                callback.onReceiveValue(visited.toArray(new String[0]));
            }
        }

        @Override
        public void onReceivedTitle(WebView view, String title) {
            send(Wire.title(page.tab, title));
        }

        @Override
        public void onProgressChanged(WebView view, int progress) {
            send(Wire.progress(page.tab, progress / 100f, progress < 100));
        }

        @Override
        public boolean onCreateWindow(WebView view, boolean dialog, boolean gesture, Message result) {
            // The address is not told; a WebView that never shows learns it from its first load.
            WebView catcher = new WebView(view.getContext());
            catcher.setWebViewClient(new WebViewClient() {
                @Override
                public boolean shouldOverrideUrlLoading(WebView v, WebResourceRequest request) {
                    send(Wire.openTab(page.tab, request.getUrl().toString(), false));
                    main.post(v::destroy);
                    return true;
                }
            });
            ((WebView.WebViewTransport) result.obj).setWebView(catcher);
            result.sendToTarget();
            return true;
        }

        @Override
        public void onCloseWindow(WebView window) {
            send(Wire.closeTab(page.tab));
        }

        @Override
        public boolean onJsAlert(WebView view, String url, String message, JsResult result) {
            answer(page, false, null);
            page.result = result;
            send(Wire.dialog(page.tab, Wire.ALERT, message, ""));
            return true;
        }

        @Override
        public boolean onJsConfirm(WebView view, String url, String message, JsResult result) {
            answer(page, false, null);
            page.result = result;
            send(Wire.dialog(page.tab, Wire.CONFIRM, message, ""));
            return true;
        }

        @Override
        public boolean onJsPrompt(WebView view, String url, String message, String defaultValue, JsPromptResult result) {
            answer(page, false, null);
            page.prompt = result;
            send(Wire.dialog(page.tab, Wire.PROMPT, message, defaultValue != null ? defaultValue : ""));
            return true;
        }

        @Override
        public boolean onJsBeforeUnload(WebView view, String url, String message, JsResult result) {
            answer(page, false, null);
            page.result = result;
            send(Wire.dialog(page.tab, Wire.LEAVE, message, ""));
            return true;
        }

        @Override
        public boolean onShowFileChooser(WebView view, ValueCallback<android.net.Uri[]> callback, FileChooserParams params) {
            callback.onReceiveValue(null);
            return true;
        }

        @Override
        public void onPermissionRequest(PermissionRequest request) {
            request.deny();
        }
    }

    /** The page's <select>, told by the script in it. */
    /** The way out of SpatiMesh's media script in a tab's top frame (`media/page.js`). */
    final class MediaBridge {
        final Page page;

        MediaBridge(Page page) {
            this.page = page;
        }

        @JavascriptInterface
        public void post(String json) {
            if (json != null)
                main.post(() -> send(Wire.media(page.tab, json)));
        }
    }

    final class SelectBridge {
        final Page page;

        SelectBridge(Page page) {
            this.page = page;
        }

        @JavascriptInterface
        public void open(String json) {
            try {
                JSONObject body = new JSONObject(json);
                JSONArray options = body.getJSONArray("options");
                int n = options.length();
                String[] labels = new String[n];
                boolean[] enabled = new boolean[n], selected = new boolean[n];
                for (int i = 0; i < n; i++) {
                    JSONObject o = options.getJSONObject(i);
                    labels[i] = o.optString("label");
                    enabled[i] = o.optBoolean("enabled", true);
                    selected[i] = o.optBoolean("selected", false);
                }
                // CSS pixels to the picture's: the display's density is the page's zoom.
                float scale = page.zoom;
                send(Wire.choose(page.tab, Math.round((float) body.optDouble("x") * scale), Math.round((float) body.optDouble("y") * scale),
                        Math.round((float) body.optDouble("w") * scale), Math.round((float) body.optDouble("h") * scale), labels, enabled, selected));
            } catch (org.json.JSONException e) {
                Log.w(TAG, "a list the page described badly: " + e);
            }
        }
    }

    /** A window in the room is a desktop browser's: say what Chrome's "Desktop site" says, Linux
     *  on a PC, with this WebView's own Chrome version, so sites give the desktop pages. */
    static String desktopAgent(String webview) {
        java.util.regex.Matcher chrome = java.util.regex.Pattern.compile("Chrome/([0-9.]+)").matcher(webview);
        String version = chrome.find() ? chrome.group(1) : "120.0.0.0";
        return "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/" + version + " Safari/537.36";
    }
}
