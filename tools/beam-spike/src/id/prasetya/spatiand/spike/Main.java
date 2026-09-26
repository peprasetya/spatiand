package id.prasetya.spatiand.spike;

import android.app.Activity;
import android.app.PendingIntent;
import android.app.Presentation;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.hardware.display.DisplayManager;
import android.hardware.usb.UsbConstants;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbEndpoint;
import android.hardware.usb.UsbInterface;
import android.hardware.usb.UsbManager;
import android.opengl.GLES30;
import android.opengl.GLSurfaceView;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.view.Display;
import android.widget.ScrollView;
import android.widget.TextView;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.zip.CRC32;

import javax.microedition.khronos.egl.EGLConfig;
import javax.microedition.khronos.opengles.GL10;

/**
 * Phase 0 spike. Answers, on the real device, whether an ordinary app can:
 * (a) put its own pixels on the glasses, (b) switch them to side-by-side 3D,
 * (c) read their IMU. Driven from adb:
 *
 *   am start -n id.prasetya.spatiand.spike/.Main --es cmd show|imu|zero|displays|stop
 *   am start -n id.prasetya.spatiand.spike/.Main --es cmd mode --ei mode 3
 *
 * Everything is logged under the tag "spike".
 */
public class Main extends Activity {
    static final String TAG = "spike";
    static final int VID = 0x3318, PID = 0x0424;
    static final String ACTION_USB = "id.prasetya.spatiand.spike.USB";

    TextView logView;
    final Handler ui = new Handler(Looper.getMainLooper());
    Presentation presentation;
    volatile boolean imuRunning;
    Thread imuThread;
    UsbDeviceConnection conn;
    // Integrated gyro angles in degrees, drawn as bars so head motion is visible.
    static volatile float angX, angY, angZ;
    // Gyro bias, learned over two still seconds after "zero".
    static volatile float bx, by, bz;
    static volatile long zeroUntil;
    static double sx, sy, sz;
    static int sn;
    int pendingMode = -1;
    boolean pendingImu;

    @Override
    protected void onCreate(Bundle b) {
        super.onCreate(b);
        logView = new TextView(this);
        logView.setTextSize(11);
        ScrollView sv = new ScrollView(this);
        sv.addView(logView);
        setContentView(sv);
        registerReceiver(usbReceiver, new IntentFilter(ACTION_USB), Context.RECEIVER_NOT_EXPORTED);
        handle(getIntent());
    }

    @Override
    protected void onNewIntent(Intent i) {
        super.onNewIntent(i);
        handle(i);
    }

    void handle(Intent i) {
        String cmd = i.getStringExtra("cmd");
        if (cmd == null) cmd = UsbManager.ACTION_USB_DEVICE_ATTACHED.equals(i.getAction()) ? "imu" : "displays";
        log("cmd=" + cmd);
        switch (cmd) {
            case "displays": displays(); break;
            case "show": displays(); show(); break;
            case "imu": pendingImu = true; withUsb(); break;
            case "mode": pendingMode = i.getIntExtra("mode", 3); withUsb(); break;
            case "stop": stopAll(); break;
            case "zero":
                sx = sy = sz = 0; sn = 0;
                zeroUntil = System.nanoTime() + 2_000_000_000L;
                log("zero: hold still for 2 s");
                break;
        }
    }

    void log(String s) {
        Log.i(TAG, s);
        ui.post(() -> logView.append(s + "\n"));
    }

    // ---------------------------------------------------------------- display

    Display glasses() {
        DisplayManager dm = getSystemService(DisplayManager.class);
        for (Display d : dm.getDisplays(DisplayManager.DISPLAY_CATEGORY_PRESENTATION)) {
            if (d.getDeviceProductInfo() != null
                    && String.valueOf(d.getDeviceProductInfo().getManufacturerPnpId()).contains("MRG")) {
                return d;
            }
        }
        return null;
    }

    void displays() {
        DisplayManager dm = getSystemService(DisplayManager.class);
        for (Display d : dm.getDisplays()) {
            Display.Mode m = d.getMode();
            log("display " + d.getDisplayId() + " '" + d.getName() + "' " + m.getPhysicalWidth() + "x"
                    + m.getPhysicalHeight() + "@" + m.getRefreshRate() + " flags=0x" + Integer.toHexString(d.getFlags())
                    + " pnp=" + (d.getDeviceProductInfo() == null ? "-" : d.getDeviceProductInfo().getManufacturerPnpId()));
        }
    }

    void show() {
        Display d = glasses();
        if (d == null) { log("no MRG display: are the glasses plugged in?"); return; }
        if (presentation != null) presentation.dismiss();
        presentation = new Presentation(this, d);
        GLSurfaceView gl = new GLSurfaceView(presentation.getContext());
        gl.setEGLContextClientVersion(3);
        gl.setRenderer(new Pattern());
        presentation.setContentView(gl);
        presentation.show();
        log("presentation shown on display " + d.getDisplayId());
    }

    /** Left eye red, right eye blue; three white bars track the integrated gyro axes. */
    class Pattern implements GLSurfaceView.Renderer {
        int w, h, frames;
        long since = System.nanoTime();

        public void onSurfaceCreated(GL10 u, EGLConfig c) {}

        public void onSurfaceChanged(GL10 u, int w, int h) {
            this.w = w; this.h = h;
            log("glasses surface " + w + "x" + h);
        }

        public void onDrawFrame(GL10 u) {
            GLES30.glEnable(GLES30.GL_SCISSOR_TEST);
            int half = w / 2;
            rect(0, 0, half, h, 0.6f, 0.1f, 0.1f);
            rect(half, 0, w - half, h, 0.1f, 0.1f, 0.6f);
            float[] a = {angX, angY, angZ};
            for (int eye = 0; eye < 2; eye++) {
                for (int k = 0; k < 3; k++) {
                    int x = eye * half + half / 2 + (int) (a[k] * half / 180f);
                    x = Math.max(eye * half, Math.min(eye * half + half - 8, x));
                    rect(x, h / 4 * (k + 1) - 20, 8, 40, 1, 1, 1);
                }
            }
            frames++;
            long now = System.nanoTime();
            if (now - since > 2_000_000_000L) {
                log(String.format("glasses fps %.1f", frames * 1e9 / (now - since)));
                frames = 0; since = now;
            }
        }

        void rect(int x, int y, int rw, int rh, float r, float g, float b) {
            GLES30.glScissor(x, y, rw, rh);
            GLES30.glClearColor(r, g, b, 1);
            GLES30.glClear(GLES30.GL_COLOR_BUFFER_BIT);
        }
    }

    // ---------------------------------------------------------------- usb

    UsbDevice device() {
        UsbManager um = getSystemService(UsbManager.class);
        for (UsbDevice d : um.getDeviceList().values()) {
            if (d.getVendorId() == VID && d.getProductId() == PID) return d;
        }
        return null;
    }

    void withUsb() {
        UsbDevice d = device();
        if (d == null) { log("glasses not on USB"); return; }
        UsbManager um = getSystemService(UsbManager.class);
        if (!um.hasPermission(d)) {
            log("asking for USB permission");
            Intent pi = new Intent(ACTION_USB).setPackage(getPackageName());
            um.requestPermission(d, PendingIntent.getBroadcast(this, 0, pi, PendingIntent.FLAG_MUTABLE));
            return;
        }
        usbReady(d);
    }

    final BroadcastReceiver usbReceiver = new BroadcastReceiver() {
        public void onReceive(Context c, Intent i) {
            boolean ok = i.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false);
            log("USB permission " + (ok ? "granted" : "denied"));
            UsbDevice d = device();
            if (ok && d != null) usbReady(d);
        }
    };

    void usbReady(UsbDevice d) {
        if (conn == null) {
            conn = getSystemService(UsbManager.class).openDevice(d);
            log("opened " + d.getDeviceName() + " conn=" + conn);
            if (conn == null) return;
        }
        if (pendingMode >= 0) { setMode(d, pendingMode); pendingMode = -1; }
        if (pendingImu) { pendingImu = false; startImu(d); }
    }

    static UsbInterface iface(UsbDevice d, int n) {
        for (int i = 0; i < d.getInterfaceCount(); i++) {
            UsbInterface f = d.getInterface(i);
            if (f.getId() == n && f.getAlternateSetting() == 0) return f;
        }
        return null;
    }

    static UsbEndpoint ep(UsbInterface f, int dir) {
        for (int i = 0; i < f.getEndpointCount(); i++) {
            if (f.getEndpoint(i).getDirection() == dir) return f.getEndpoint(i);
        }
        return null;
    }

    static int crc(byte[] b, int off, int len) {
        CRC32 c = new CRC32();
        c.update(b, off, len);
        return (int) c.getValue();
    }

    void setMode(UsbDevice d, int mode) {
        UsbInterface f = iface(d, 4);
        if (!conn.claimInterface(f, true)) { log("claim MCU failed"); return; }
        ByteBuffer p = ByteBuffer.allocate(23).order(ByteOrder.LITTLE_ENDIAN);
        p.put((byte) 0xFD).putInt(0).putShort((short) 18).putLong(System.currentTimeMillis())
                .putShort((short) 0x0008).put(new byte[5]).put((byte) mode);
        byte[] b = p.array();
        ByteBuffer.wrap(b, 1, 4).order(ByteOrder.LITTLE_ENDIAN).putInt(crc(b, 5, 18));
        int n = conn.bulkTransfer(ep(f, UsbConstants.USB_DIR_OUT), b, b.length, 500);
        byte[] r = new byte[64];
        int m = conn.bulkTransfer(ep(f, UsbConstants.USB_DIR_IN), r, r.length, 500);
        log("mode " + mode + ": wrote " + n + ", reply " + m + " msgid@15=0x" + (m > 16 ? Integer.toHexString(r[15] & 0xff) : "-"));
    }

    void startImu(UsbDevice d) {
        if (imuRunning) { log("imu already running"); return; }
        UsbInterface f = iface(d, 3);
        if (!conn.claimInterface(f, true)) { log("claim IMU failed"); return; }
        UsbEndpoint in = ep(f, UsbConstants.USB_DIR_IN), out = ep(f, UsbConstants.USB_DIR_OUT);
        byte[] start = {(byte) 0xAA, 0, 0, 0, 0, 0x04, 0x00, 0x19, 0x01};
        ByteBuffer.wrap(start, 1, 4).order(ByteOrder.LITTLE_ENDIAN).putInt(crc(start, 5, 4));
        log("imu start wrote " + conn.bulkTransfer(out, start, start.length, 500));
        imuRunning = true;
        imuThread = new Thread(() -> imuLoop(in), "imu");
        imuThread.start();
    }

    static int i24(byte[] r, int o) { return ((r[o] & 0xff) | (r[o + 1] & 0xff) << 8 | r[o + 2] << 16); }
    static int i16le(byte[] r, int o) { return (short) ((r[o] & 0xff) | (r[o + 1] & 0xff) << 8); }
    static int i32le(byte[] r, int o) { return ByteBuffer.wrap(r, o, 4).order(ByteOrder.LITTLE_ENDIAN).getInt(); }

    void imuLoop(UsbEndpoint in) {
        byte[] r = new byte[64];
        int count = 0;
        long since = System.nanoTime(), last = 0;
        float px = 0, py = 0, pz = 0;
        while (imuRunning) {
            int n = conn.bulkTransfer(in, r, r.length, 200);
            if (n < 54 || r[0] != 0x01 || r[1] != 0x02) continue;
            float gm = i16le(r, 12), gd = i32le(r, 14);
            float gx = i24(r, 18) * gm / gd, gy = i24(r, 21) * gm / gd, gz = i24(r, 24) * gm / gd;
            float am = i16le(r, 27), ad = i32le(r, 29);
            float ax = i24(r, 33) * am / ad, ay = i24(r, 36) * am / ad, az = i24(r, 39) * am / ad;
            float mm = (short) ((r[42] & 0xff) << 8 | (r[43] & 0xff));
            float md = ByteBuffer.wrap(r, 44, 4).order(ByteOrder.BIG_ENDIAN).getInt();
            float mx = (short) (i16le(r, 48) ^ 0x8000) * mm / md;
            float my = (short) (i16le(r, 50) ^ 0x8000) * mm / md;
            float mz = (short) (i16le(r, 52) ^ 0x8000) * mm / md;
            long now = System.nanoTime();
            if (now < zeroUntil) {
                sx += gx; sy += gy; sz += gz; sn++;
                angX = angY = angZ = 0; last = now;
                continue;
            } else if (sn > 0) {
                bx = (float) (sx / sn); by = (float) (sy / sn); bz = (float) (sz / sn);
                log(String.format("zeroed: bias (%.3f %.3f %.3f) deg/s from %d samples", bx, by, bz, sn));
                sn = 0;
            }
            gx -= bx; gy -= by; gz -= bz;
            px = Math.max(px, Math.abs(gx)); py = Math.max(py, Math.abs(gy)); pz = Math.max(pz, Math.abs(gz));
            if (last != 0) {
                float dt = (now - last) / 1e9f;
                angX += gx * dt; angY += gy * dt; angZ += gz * dt;
            }
            last = now;
            count++;
            if (now - since > 1_000_000_000L) {
                log(String.format("imu %d Hz |a|=%.3f |m|=%.3f peak=(%.0f %.0f %.0f) ang=(%.0f %.0f %.0f)",
                        count, Math.sqrt(ax * ax + ay * ay + az * az), Math.sqrt(mx * mx + my * my + mz * mz),
                        px, py, pz, angX, angY, angZ));
                count = 0; since = now; px = py = pz = 0;
            }
        }
    }

    void stopAll() {
        imuRunning = false;
        if (presentation != null) { presentation.dismiss(); presentation = null; }
        if (conn != null) { conn.close(); conn = null; }
        log("stopped");
    }

    @Override
    protected void onDestroy() {
        stopAll();
        unregisterReceiver(usbReceiver);
        super.onDestroy();
    }
}
