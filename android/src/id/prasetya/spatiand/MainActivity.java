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
import android.hardware.display.DisplayManager;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbManager;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.provider.Settings;
import android.util.Log;
import android.view.Display;
import android.view.Gravity;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.TextView;

/**
 * The phone's screen, and the owner of everything Android only gives an activity: the
 * glasses' display, through a Presentation, and the glasses' USB.
 *
 * The glasses' display comes and goes: switching them to side-by-side 3D removes it and adds
 * a double-width one. So the Presentation is put back whenever an XREAL display (PnP id "MRG")
 * appears, and the drawing follows its surface.
 */
public class MainActivity extends Activity {
    static final String TAG = "spatiand";
    static final int VENDOR = 0x3318, PRODUCT = 0x0424;
    static final String ACTION_USB = "id.prasetya.spatiand.USB";
    /** The Beam Pro's orange key, as the framework rebroadcasts it while glasses are on. */
    static final String ORANGE_DOWN = "XREAL.switchMode.down";

    final Handler ui = new Handler(Looper.getMainLooper());
    TextView status;
    Presentation presentation;
    /** The glasses' window when it is an overlay, above XREAL's placeholder. */
    SurfaceView overlay;
    WindowManager overlayManager;
    int presentationDisplay = -1;
    UsbDeviceConnection connection;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        Native.configDir(getFilesDir().getPath());

        LinearLayout column = new LinearLayout(this);
        column.setOrientation(LinearLayout.VERTICAL);
        column.setBackgroundColor(Color.rgb(12, 14, 20));
        column.setPadding(48, 96, 48, 48);
        status = new TextView(this);
        status.setTextColor(Color.rgb(220, 226, 236));
        status.setTextSize(16);
        column.addView(status, new LinearLayout.LayoutParams(-1, 0, 1));
        Button recenter = new Button(this);
        recenter.setText("Recenter");
        recenter.setOnClickListener(v -> Native.recenter());
        column.addView(recenter, new LinearLayout.LayoutParams(-1, -2));
        if (!Settings.canDrawOverlays(this)) {
            // Once, and kept: what puts the glasses' picture above XREAL's placeholder.
            Button allow = new Button(this);
            allow.setText("Allow drawing over XREAL's placeholder");
            allow.setOnClickListener(v -> startActivity(new Intent(
                    Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
                    android.net.Uri.parse("package:" + getPackageName()))));
            column.addView(allow, new LinearLayout.LayoutParams(-1, -2));
        }
        column.setGravity(Gravity.BOTTOM);
        setContentView(column);

        registerReceiver(usbPermission, new IntentFilter(ACTION_USB), Context.RECEIVER_NOT_EXPORTED);
        // While running, plugging in is heard here too: the attach intent only starts the
        // activity when it is the app chosen for the glasses, and not before.
        registerReceiver(usbAttached, new IntentFilter(UsbManager.ACTION_USB_DEVICE_ATTACHED),
                Context.RECEIVER_EXPORTED);
        registerReceiver(orangeKey, new IntentFilter(ORANGE_DOWN), Context.RECEIVER_EXPORTED);
        getSystemService(DisplayManager.class).registerDisplayListener(displays, ui);

        showOnGlasses();
        takeGlasses(true);
        ui.post(refresh);
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
            // For now it recentres; it becomes the settings menu once the shell is here.
            Log.i(TAG, "orange key");
            Native.recenter();
        }
    };

    // ------------------------------------------------------------------ the phone's screen

    final Runnable refresh = new Runnable() {
        @Override
        public void run() {
            status.setText("Spatiand\n\n" + Native.status());
            ui.postDelayed(this, 500);
        }
    };
}
