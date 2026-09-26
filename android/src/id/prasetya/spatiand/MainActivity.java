package id.prasetya.spatiand;

import android.app.Activity;
import android.app.PendingIntent;
import android.app.Presentation;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.graphics.Color;
import android.graphics.PixelFormat;
import android.hardware.Sensor;
import android.hardware.SensorEvent;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;
import android.hardware.display.DisplayManager;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbManager;
import android.media.AudioDeviceInfo;
import android.media.AudioManager;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.provider.Settings;
import android.util.Log;
import android.view.Display;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import android.view.WindowManager;
import android.widget.AdapterView;
import android.widget.ArrayAdapter;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.Spinner;
import android.widget.TextView;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.List;

/**
 * The phone: the Beam Pro's controller for the session on the glasses, and the owner of what
 * Android only gives an activity -- the glasses' display and their USB.
 *
 * The session is the Deck's own compositor (crates/spatiand-android); this is its controller:
 *
 *   top      the sound's way out and way in, small, side by side
 *   middle   the touch area: the Deck's left pad, with the machine's monitors under the thumb
 *   bottom   ⋯ (the launcher), B (back), the keyboard, and recentre
 *
 * and, without a place on the screen, the orange key as STEAM and the phone itself, pointed,
 * as the right pad. The glasses' display comes and goes -- switching them to 3D removes it and
 * adds a double-width one -- so their window is put back whenever an XREAL display appears.
 */
public class MainActivity extends Activity implements SensorEventListener {
    static final String TAG = "spatiand";
    static final int VENDOR = 0x3318, PRODUCT = 0x0424;
    static final String ACTION_USB = "id.prasetya.spatiand.USB";
    /** The Beam Pro's orange key, as the framework rebroadcasts it while glasses are on. */
    static final String ORANGE_DOWN = "XREAL.switchMode.down";
    static final String ORANGE_UP = "XREAL.switchMode.up";
    /** Bumped when the bundled keyboard layouts change, so they are unpacked again. */
    static final String XKB_VERSION = "xkb-data 2.42-1";

    final Handler ui = new Handler(Looper.getMainLooper());
    TextView status;
    TouchArea touch;
    Spinner output, input;
    Presentation presentation;
    /** The glasses' window when it is an overlay, above XREAL's placeholder. */
    SurfaceView overlay;
    WindowManager overlayManager;
    int presentationDisplay = -1;
    UsbDeviceConnection connection;
    SensorManager sensors;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        Native.configDir(getFilesDir().getPath(), getCacheDir().getPath());
        Native.xkbDir(unpackLayouts());
        unpackAsset("default.sofa", new File(getFilesDir(), "default.sofa"));
        Native.begin();

        LinearLayout column = new LinearLayout(this);
        column.setOrientation(LinearLayout.VERTICAL);
        column.setBackgroundColor(Color.rgb(9, 10, 14));
        column.setPadding(24, 72, 24, 24);

        // The sound's way out and way in: small, as the rest of the screen is the pad.
        LinearLayout sound = new LinearLayout(this);
        output = new Spinner(this);
        input = new Spinner(this);
        sound.addView(output, new LinearLayout.LayoutParams(0, -2, 1));
        sound.addView(input, new LinearLayout.LayoutParams(0, -2, 1));
        column.addView(sound, new LinearLayout.LayoutParams(-1, -2));

        status = new TextView(this);
        status.setTextColor(Color.rgb(150, 158, 175));
        status.setTextSize(12);
        column.addView(status, new LinearLayout.LayoutParams(-1, -2));

        touch = new TouchArea(this);
        column.addView(touch, new LinearLayout.LayoutParams(-1, 0, 1));

        LinearLayout bar = new LinearLayout(this);
        bar.addView(pressable("⋯", 1), new LinearLayout.LayoutParams(0, -2, 1));
        bar.addView(pressable("B", 2), new LinearLayout.LayoutParams(0, -2, 1));
        Button keys = new Button(this);
        keys.setText("⌨");
        keys.setOnClickListener(v -> touch.toggleKeyboard());
        bar.addView(keys, new LinearLayout.LayoutParams(0, -2, 1));
        Button recenter = new Button(this);
        recenter.setText("◎");
        recenter.setOnClickListener(v -> Native.recenter());
        bar.addView(recenter, new LinearLayout.LayoutParams(0, -2, 1));
        column.addView(bar, new LinearLayout.LayoutParams(-1, -2));

        if (!Settings.canDrawOverlays(this)) {
            // Once, and kept: what puts the glasses' picture above XREAL's placeholder.
            Button allow = new Button(this);
            allow.setText("Allow drawing over XREAL's placeholder");
            allow.setOnClickListener(v -> startActivity(new Intent(
                    Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
                    android.net.Uri.parse("package:" + getPackageName()))));
            column.addView(allow, new LinearLayout.LayoutParams(-1, -2));
        }
        setContentView(column);
        fillSoundPickers();

        registerReceiver(usbPermission, new IntentFilter(ACTION_USB), Context.RECEIVER_NOT_EXPORTED);
        // While running, plugging in is heard here too.
        registerReceiver(usbAttached, new IntentFilter(UsbManager.ACTION_USB_DEVICE_ATTACHED),
                Context.RECEIVER_EXPORTED);
        IntentFilter orange = new IntentFilter(ORANGE_DOWN);
        orange.addAction(ORANGE_UP);
        registerReceiver(orangeKey, orange, Context.RECEIVER_EXPORTED);
        getSystemService(DisplayManager.class).registerDisplayListener(displays, ui);

        sensors = getSystemService(SensorManager.class);
        showOnGlasses();
        takeGlasses(true);
        ui.post(refresh);
    }

    /** A bottom-bar button that is held as long as the finger is on it, as a Deck button is. */
    Button pressable(String label, int which) {
        Button b = new Button(this);
        b.setText(label);
        b.setOnTouchListener((v, e) -> {
            if (e.getActionMasked() == MotionEvent.ACTION_DOWN) {
                Native.button(which, true);
                v.setPressed(true);
            } else if (e.getActionMasked() == MotionEvent.ACTION_UP
                    || e.getActionMasked() == MotionEvent.ACTION_CANCEL) {
                Native.button(which, false);
                v.setPressed(false);
            }
            return true;
        });
        return b;
    }

    @Override
    protected void onResume() {
        super.onResume();
        // The game rotation vector: no magnetometer, so no swing near a speaker or a desk lamp;
        // its yaw is its own, which the session aligns with the head at every recentre.
        Sensor rotation = sensors.getDefaultSensor(Sensor.TYPE_GAME_ROTATION_VECTOR);
        if (rotation != null) sensors.registerListener(this, rotation, SensorManager.SENSOR_DELAY_GAME);
    }

    @Override
    protected void onPause() {
        sensors.unregisterListener(this);
        super.onPause();
    }

    @Override
    public void onSensorChanged(SensorEvent e) {
        float[] q = new float[4];
        SensorManager.getQuaternionFromVector(q, e.values);
        // Android gives w, x, y, z.
        Native.rotation(q[1], q[2], q[3], q[0]);
    }

    @Override
    public void onAccuracyChanged(Sensor sensor, int accuracy) {}

    /** A keyboard's keys go to the session, except the phone's own volume and back. */
    @Override
    public boolean dispatchKeyEvent(KeyEvent e) {
        int code = e.getKeyCode();
        boolean system = code == KeyEvent.KEYCODE_VOLUME_UP || code == KeyEvent.KEYCODE_VOLUME_DOWN
                || code == KeyEvent.KEYCODE_BACK || code == KeyEvent.KEYCODE_HOME
                || code == 206 /* KEYCODE_3D_MODE: the orange key, the framework's */;
        if (!system && e.getAction() != KeyEvent.ACTION_MULTIPLE
                && Native.key(code, e.getAction() == KeyEvent.ACTION_DOWN)) {
            return true;
        }
        return super.dispatchKeyEvent(e);
    }

    /**
     * The keyboard layouts the compositor's keyboard is made from, out of the APK and into the
     * app's files, once per version. See android/xkbcommon.
     */
    String unpackLayouts() {
        File root = new File(getFilesDir(), "xkb");
        File stamp = new File(root, ".version");
        try {
            if (stamp.exists() && new String(java.nio.file.Files.readAllBytes(stamp.toPath())).equals(XKB_VERSION)) {
                return root.getPath();
            }
            copyAssets("xkb", root);
            try (OutputStream out = new FileOutputStream(stamp)) {
                out.write(XKB_VERSION.getBytes());
            }
        } catch (Exception e) {
            Log.e(TAG, "could not unpack the keyboard layouts", e);
        }
        return root.getPath();
    }

    /** One file out of the APK, once. */
    void unpackAsset(String name, File to) {
        if (to.exists()) return;
        try (InputStream in = getAssets().open(name); OutputStream out = new FileOutputStream(to)) {
            byte[] buffer = new byte[65536];
            for (int n; (n = in.read(buffer)) > 0; ) out.write(buffer, 0, n);
        } catch (Exception e) {
            Log.e(TAG, "could not unpack " + name, e);
        }
    }

    void copyAssets(String from, File to) throws Exception {
        String[] names = getAssets().list(from);
        if (names == null || names.length == 0) {
            to.getParentFile().mkdirs();
            try (InputStream in = getAssets().open(from); OutputStream out = new FileOutputStream(to)) {
                byte[] buffer = new byte[16384];
                for (int n; (n = in.read(buffer)) > 0; ) out.write(buffer, 0, n);
            }
            return;
        }
        to.mkdirs();
        for (String name : names) copyAssets(from + "/" + name, new File(to, name));
    }

    /** What the phone can play to and listen with, for the pickers at the top. */
    void fillSoundPickers() {
        AudioManager audio = getSystemService(AudioManager.class);
        AudioDeviceInfo[] outs = audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS);
        fillPicker(output, outs, "Out");
        fillPicker(input, audio.getDevices(AudioManager.GET_DEVICES_INPUTS), "In");
        // The first entry is Android's own choice, which follows whatever is plugged in.
        output.setOnItemSelectedListener(new AdapterView.OnItemSelectedListener() {
            @Override
            public void onItemSelected(AdapterView<?> parent, View view, int position, long id) {
                Native.audioOutput(position == 0 ? 0 : outs[position - 1].getId());
            }

            @Override
            public void onNothingSelected(AdapterView<?> parent) {}
        });
    }

    void fillPicker(Spinner picker, AudioDeviceInfo[] devices, String what) {
        List<String> names = new ArrayList<>();
        names.add(what + ": automatic");
        for (AudioDeviceInfo d : devices) {
            String name = d.getProductName() == null ? "" : d.getProductName().toString();
            names.add(what + ": " + deviceKind(d.getType()) + (name.isEmpty() ? "" : " (" + name + ")"));
        }
        ArrayAdapter<String> adapter = new ArrayAdapter<>(this, android.R.layout.simple_spinner_item, names);
        adapter.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        picker.setAdapter(adapter);
    }

    static String deviceKind(int type) {
        switch (type) {
            case AudioDeviceInfo.TYPE_BUILTIN_SPEAKER: return "Speaker";
            case AudioDeviceInfo.TYPE_BUILTIN_EARPIECE: return "Earpiece";
            case AudioDeviceInfo.TYPE_BUILTIN_MIC: return "Microphone";
            case AudioDeviceInfo.TYPE_USB_DEVICE:
            case AudioDeviceInfo.TYPE_USB_HEADSET: return "Glasses / USB";
            case AudioDeviceInfo.TYPE_BLUETOOTH_A2DP:
            case AudioDeviceInfo.TYPE_BLUETOOTH_SCO:
            case AudioDeviceInfo.TYPE_BLE_HEADSET: return "Bluetooth";
            case AudioDeviceInfo.TYPE_WIRED_HEADPHONES:
            case AudioDeviceInfo.TYPE_WIRED_HEADSET: return "Headphones";
            default: return "Device " + type;
        }
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        // Plugged in again while already running.
        if (UsbManager.ACTION_USB_DEVICE_ATTACHED.equals(intent.getAction())) takeGlasses(false);
    }

    @Override
    protected void onDestroy() {
        ui.removeCallbacks(refresh);
        getSystemService(DisplayManager.class).unregisterDisplayListener(displays);
        unregisterReceiver(usbPermission);
        unregisterReceiver(usbAttached);
        unregisterReceiver(orangeKey);
        closeGlassesWindow();
        Native.stop();
        if (connection != null) connection.close();
        super.onDestroy();
    }

    // ------------------------------------------------------------------ the glasses' display

    Display glassesDisplay() {
        for (Display d : getSystemService(DisplayManager.class)
                .getDisplays(DisplayManager.DISPLAY_CATEGORY_PRESENTATION)) {
            if (d.getDeviceProductInfo() != null
                    && String.valueOf(d.getDeviceProductInfo().getManufacturerPnpId()).contains("MRG")) {
                return d;
            }
        }
        return null;
    }

    void showOnGlasses() {
        Display d = glassesDisplay();
        if (d == null) {
            Log.i(TAG, "no glasses display yet");
            return;
        }
        if (presentationDisplay == d.getDisplayId() && (overlay != null
                || (presentation != null && presentation.isShowing()))) {
            return;
        }
        closeGlassesWindow();
        presentationDisplay = d.getDisplayId();
        // Above XREAL's placeholder if Android lets us, as an overlay; as a Presentation, under
        // whatever the placeholder does, if not.
        boolean above = Settings.canDrawOverlays(this);
        Context on = above
                ? createDisplayContext(d).createWindowContext(
                        WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY, null)
                : null;
        if (!above) presentation = new Presentation(this, d);
        SurfaceView view = new SurfaceView(above ? on : presentation.getContext());
        view.getHolder().addCallback(new SurfaceHolder.Callback() {
            @Override
            public void surfaceCreated(SurfaceHolder holder) {
                holder.getSurface().setFrameRate(d.getRefreshRate(),
                        android.view.Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE);
                Native.surface(holder.getSurface());
            }

            @Override
            public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {}

            @Override
            public void surfaceDestroyed(SurfaceHolder holder) {
                Native.surfaceGone();
            }
        });
        if (above) {
            WindowManager.LayoutParams attrs = new WindowManager.LayoutParams(
                    WindowManager.LayoutParams.MATCH_PARENT,
                    WindowManager.LayoutParams.MATCH_PARENT,
                    WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
                    // Not NOT_TOUCHABLE: Android caps an untouchable overlay at 80% opacity, so
                    // the placeholder showed through. The glasses have no touch to take anyway.
                    WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE
                            | WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN
                            | WindowManager.LayoutParams.FLAG_FULLSCREEN,
                    PixelFormat.OPAQUE);
            attrs.preferredDisplayModeId = d.getMode().getModeId();
            overlayManager = on.getSystemService(WindowManager.class);
            overlayManager.addView(view, attrs);
            overlay = view;
        } else {
            presentation.setContentView(view);
            WindowManager.LayoutParams attrs = presentation.getWindow().getAttributes();
            attrs.preferredDisplayModeId = d.getMode().getModeId();
            presentation.getWindow().setAttributes(attrs);
            presentation.show();
        }
        Log.i(TAG, "on the glasses " + (above ? "above" : "under") + " XREAL's placeholder, display "
                + d.getDisplayId() + " " + d.getMode());
    }

    void closeGlassesWindow() {
        if (overlay != null) {
            try {
                overlayManager.removeViewImmediate(overlay);
            } catch (RuntimeException gone) {
                // Its display has gone already, and the window with it.
            }
            overlay = null;
        }
        if (presentation != null) {
            presentation.dismiss();
            presentation = null;
        }
        presentationDisplay = -1;
    }

    final DisplayManager.DisplayListener displays = new DisplayManager.DisplayListener() {
        @Override
        public void onDisplayAdded(int id) {
            showOnGlasses();
            // The glasses' display and their USB arrive together; whichever is heard first.
            if (connection == null) takeGlasses(false);
        }

        @Override
        public void onDisplayChanged(int id) {
            showOnGlasses();
        }

        @Override
        public void onDisplayRemoved(int id) {
            if (id == presentationDisplay) closeGlassesWindow();
        }
    };

    // ------------------------------------------------------------------ the glasses' USB

    UsbDevice glassesDevice() {
        for (UsbDevice d : getSystemService(UsbManager.class).getDeviceList().values()) {
            if (d.getVendorId() == VENDOR && d.getProductId() == PRODUCT) return d;
        }
        return null;
    }

    /**
     * Open the glasses if Android lets us. {@code ask} is whether to ask for them when it does
     * not yet: only when the app was opened by hand with the glasses already in. On plugging in,
     * Android asks itself -- "Open Spatiand to handle XREAL Air?" -- and that question, unlike
     * ours, has an "Always" that makes the answer permanent. Asking here as well would put
     * ours, which lasts only until unplugging, in front of it every time.
     */
    void takeGlasses(boolean ask) {
        UsbDevice d = glassesDevice();
        if (d == null) {
            Log.i(TAG, "no glasses on USB");
            return;
        }
        Log.i(TAG, "glasses on USB: " + d.getDeviceName());
        UsbManager usb = getSystemService(UsbManager.class);
        if (!usb.hasPermission(d)) {
            if (!ask) {
                Log.i(TAG, "glasses on USB, waiting for Android to hand them over");
                return;
            }
            Intent reply = new Intent(ACTION_USB).setPackage(getPackageName());
            usb.requestPermission(d, PendingIntent.getBroadcast(this, 0, reply, PendingIntent.FLAG_MUTABLE));
            return;
        }
        if (connection != null) {
            Native.stop();
            connection.close();
        }
        connection = usb.openDevice(d);
        if (connection == null) {
            Log.w(TAG, "could not open the glasses");
            return;
        }
        Native.start(connection.getFileDescriptor());
    }

    final BroadcastReceiver usbAttached = new BroadcastReceiver() {
        @Override
        public void onReceive(Context c, Intent i) {
            if (connection == null) takeGlasses(false);
        }
    };

    final BroadcastReceiver usbPermission = new BroadcastReceiver() {
        @Override
        public void onReceive(Context c, Intent i) {
            if (i.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)) takeGlasses(false);
        }
    };

    final BroadcastReceiver orangeKey = new BroadcastReceiver() {
        @Override
        public void onReceive(Context c, Intent i) {
            // STEAM: the HUD. Down and up, as the framework says them.
            Native.button(0, ORANGE_DOWN.equals(i.getAction()));
        }
    };

    // ------------------------------------------------------------------ the phone's screen

    final Runnable refresh = new Runnable() {
        @Override
        public void run() {
            status.setText(Native.status());
            Native.traffic(android.net.TrafficStats.getTotalRxBytes(), android.net.TrafficStats.getTotalTxBytes());
            ui.postDelayed(this, 1000);
        }
    };
}
