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
import android.hardware.input.InputManager;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbManager;
import android.media.AudioDeviceInfo;
import android.media.AudioManager;
import android.os.Bundle;
import android.os.CombinedVibration;
import android.os.VibrationEffect;
import android.os.VibratorManager;
import android.view.InputDevice;
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
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * The phone: the Beam Pro's controller for the session on the glasses, and the owner of what
 * Android only gives an activity -- the glasses' display and their USB.
 *
 * The session is the Deck's own compositor (crates/spatiand-android); this is its controller:
 *
 *   top      the sound's way out and way in, small, side by side
 *   middle   the touch area: the Deck's left pad
 *   bottom   back (B), settings (STEAM: the HUD), ⋯ (the launcher), and Android's keyboard,
 *            last, so that the button that brings it up and the one that puts it away -- the
 *            same one, sitting just above it -- are under the same thumb
 *
 * and, without a place on the screen, the orange key as recentre and the phone itself,
 * pointed, as the right pad. The glasses' display comes and goes -- switching them to 3D removes it and
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
    /** Each gamepad's gyro listener, so it can be let go of when the pad goes. */
    final Map<Integer, SensorEventListener> padGyros = new HashMap<>();

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
        // The touch area, and over it, while there are no glasses, the reminder to plug them in.
        android.widget.FrameLayout middle = new android.widget.FrameLayout(this);
        middle.addView(touch, new android.widget.FrameLayout.LayoutParams(-1, -1));
        reminder = new TextView(this);
        reminder.setText("Plug in your XR glasses\n\nConnect XREAL Air glasses over USB-C\nand Spatiand will open on them.");
        reminder.setTextColor(Color.rgb(222, 228, 240));
        reminder.setTextSize(18);
        reminder.setGravity(android.view.Gravity.CENTER);
        reminder.setBackgroundColor(Color.rgb(9, 10, 14));
        middle.addView(reminder, new android.widget.FrameLayout.LayoutParams(-1, -1));
        column.addView(middle, new LinearLayout.LayoutParams(-1, 0, 1));

        LinearLayout bar = new LinearLayout(this);
        bar.setGravity(android.view.Gravity.CENTER_VERTICAL);
        bar.setPadding(0, 24, 0, 8);
        addSpread(bar, pressable(RoundButton.Icon.BACK, 2));
        addSpread(bar, pressable(RoundButton.Icon.SETTINGS, 0));
        addSpread(bar, pressable(RoundButton.Icon.MENU, 1));
        keys = new RoundButton(this, RoundButton.Icon.KEYBOARD);
        keys.setOnClickListener(v -> {
            buzz(0);
            touch.toggleKeyboard();
        });
        addSpread(bar, keys);
        column.addView(bar, new LinearLayout.LayoutParams(-1, -2));
        // Whether Android's keyboard is up, to draw the button that puts it away. The window
        // is resized above the keyboard, so that button sits just over it.
        column.getViewTreeObserver().addOnGlobalLayoutListener(() -> {
            android.view.WindowInsets insets = column.getRootWindowInsets();
            boolean up = insets != null && insets.isVisible(android.view.WindowInsets.Type.ime());
            keys.setIcon(up ? RoundButton.Icon.KEYBOARD_HIDE : RoundButton.Icon.KEYBOARD);
        });

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
        registerReceiver(usbDetached, new IntentFilter(UsbManager.ACTION_USB_DEVICE_DETACHED),
                Context.RECEIVER_EXPORTED);
        IntentFilter orange = new IntentFilter(ORANGE_DOWN);
        orange.addAction(ORANGE_UP);
        registerReceiver(orangeKey, orange, Context.RECEIVER_EXPORTED);
        getSystemService(DisplayManager.class).registerDisplayListener(displays, ui);

        sensors = getSystemService(SensorManager.class);
        if (checkSelfPermission(android.Manifest.permission.RECORD_AUDIO)
                != android.content.pm.PackageManager.PERMISSION_GRANTED) {
            // Asked once; a host only gets the microphone when one of its apps is listening.
            requestPermissions(new String[] {android.Manifest.permission.RECORD_AUDIO}, 1);
        }
        InputManager inputs = getSystemService(InputManager.class);
        inputs.registerInputDeviceListener(padsListener, ui);
        for (int id : inputs.getInputDeviceIds()) padsListener.onInputDeviceAdded(id);
        showOnGlasses();
        takeGlasses(true);
        showReminder();
        ui.post(refresh);
        ui.post(rumbler);
        startBuzzer();
    }

    // ------------------------------------------------------------------ the phone's own buzz

    android.os.Vibrator phoneMotor;

    /** Buzz the phone: 0 a click, 1 a tick, 2 an alert. As the Deck's pads buzz under a thumb. */
    void buzz(int feel) {
        if (phoneMotor == null) {
            phoneMotor = getSystemService(VibratorManager.class).getDefaultVibrator();
        }
        if (!phoneMotor.hasVibrator()) return;
        VibrationEffect effect;
        switch (feel) {
            case 1: effect = VibrationEffect.createPredefined(VibrationEffect.EFFECT_TICK); break;
            case 2: effect = VibrationEffect.createPredefined(VibrationEffect.EFFECT_DOUBLE_CLICK); break;
            default: effect = VibrationEffect.createPredefined(VibrationEffect.EFFECT_CLICK); break;
        }
        phoneMotor.vibrate(effect);
    }

    /** A thread that waits on the session for buzzes: a key's is felt as the key goes down. */
    void startBuzzer() {
        Thread t = new Thread(() -> {
            while (true) {
                int feel = Native.nextBuzz();
                if (feel >= 0) buzz(feel);
            }
        }, "spatiand-buzz");
        t.setDaemon(true);
        t.start();
    }

    RoundButton keys;
    TextView reminder;

    /** Show the reminder to plug the glasses in while they are not. */
    void showReminder() {
        if (reminder != null) reminder.setVisibility(connection == null ? View.VISIBLE : View.GONE);
    }

    /** A cell of the bar, each as wide as the others, with the button centred in it. */
    void addSpread(LinearLayout bar, View button) {
        LinearLayout cell = new LinearLayout(this);
        cell.setGravity(android.view.Gravity.CENTER);
        cell.addView(button);
        bar.addView(cell, new LinearLayout.LayoutParams(0, -2, 1));
    }

    /** A bottom-bar button that is held as long as the finger is on it, as a Deck button is. */
    View pressable(RoundButton.Icon icon, int which) {
        RoundButton b = new RoundButton(this, icon);
        b.setOnTouchListener((v, e) -> {
            if (e.getActionMasked() == MotionEvent.ACTION_DOWN) {
                Native.button(which, true);
                buzz(0);
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

    static boolean fromPad(int source) {
        return (source & InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD
                || (source & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK;
    }

    boolean padSeen, padKeySeen;

    /** A gamepad's keys go to its pad; a keyboard's to the session, except the phone's own. */
    @Override
    public boolean dispatchKeyEvent(KeyEvent e) {
        int code = e.getKeyCode();
        if (!padKeySeen && fromPad(e.getSource())) {
            padKeySeen = true;
            Log.i(TAG, "pad key: first " + KeyEvent.keyCodeToString(code) + " from device "
                    + e.getDeviceId() + ", source 0x" + Integer.toHexString(e.getSource()));
        }
        if (fromPad(e.getSource()) && e.getAction() != KeyEvent.ACTION_MULTIPLE
                && Native.padButton(e.getDeviceId(), code, e.getAction() == KeyEvent.ACTION_DOWN)) {
            return true;
        }
        boolean system = code == KeyEvent.KEYCODE_VOLUME_UP || code == KeyEvent.KEYCODE_VOLUME_DOWN
                || code == KeyEvent.KEYCODE_BACK || code == KeyEvent.KEYCODE_HOME
                || code == 206 /* KEYCODE_3D_MODE: the orange key, the framework's */;
        if (!system && e.getAction() != KeyEvent.ACTION_MULTIPLE
                && Native.key(code, e.getAction() == KeyEvent.ACTION_DOWN)) {
            return true;
        }
        return super.dispatchKeyEvent(e);
    }

    /** A gamepad's sticks, triggers and hat. */
    @Override
    public boolean dispatchGenericMotionEvent(MotionEvent e) {
        if ((e.getSource() & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK
                && e.getActionMasked() == MotionEvent.ACTION_MOVE) {
            float lt = Math.max(e.getAxisValue(MotionEvent.AXIS_LTRIGGER), e.getAxisValue(MotionEvent.AXIS_BRAKE));
            float rt = Math.max(e.getAxisValue(MotionEvent.AXIS_RTRIGGER), e.getAxisValue(MotionEvent.AXIS_GAS));
            Native.padAxes(e.getDeviceId(),
                    e.getAxisValue(MotionEvent.AXIS_X), e.getAxisValue(MotionEvent.AXIS_Y),
                    e.getAxisValue(MotionEvent.AXIS_Z), e.getAxisValue(MotionEvent.AXIS_RZ),
                    lt, rt,
                    e.getAxisValue(MotionEvent.AXIS_HAT_X), e.getAxisValue(MotionEvent.AXIS_HAT_Y));
            return true;
        }
        return super.dispatchGenericMotionEvent(e);
    }

    // ------------------------------------------------------------------ gamepads

    /**
     * A pad with a touchpad, a mouse, or a keyboard's trackpad: Android makes each a pointer on
     * the phone's screen unless the app captures it, and captured each moves the session's.
     */
    boolean anyPadTouchpad() {
        for (int id : getSystemService(InputManager.class).getInputDeviceIds()) {
            InputDevice d = InputDevice.getDevice(id);
            if (d != null && d.isExternal()
                    && ((d.getSources() & InputDevice.SOURCE_MOUSE) == InputDevice.SOURCE_MOUSE
                        || (d.getSources() & InputDevice.SOURCE_TOUCHPAD) == InputDevice.SOURCE_TOUCHPAD)) {
                return true;
            }
        }
        return false;
    }

    /** A mouse's travel, buttons and wheel, captured: relative, every sample of the event. */
    boolean mouseEvent(MotionEvent e) {
        float dx = 0, dy = 0;
        for (int h = 0; h < e.getHistorySize(); h++) {
            dx += e.getHistoricalX(h);
            dy += e.getHistoricalY(h);
        }
        dx += e.getX();
        dy += e.getY();
        Native.mouse(dx, dy, e.getButtonState(),
                e.getAxisValue(MotionEvent.AXIS_VSCROLL), e.getAxisValue(MotionEvent.AXIS_HSCROLL));
        return true;
    }

    /**
     * Take a pad's touchpad from Android's pointer: captured, it reports where the finger is,
     * which is the Deck's right trackpad. Only while this has focus, which is when it can be.
     */
    void capturePadTouchpad() {
        // Captured events go to the focused view, so the listener is the touch area's, and the
        // touch area keeps the focus (it is also where Android's keyboard types).
        View root = touch;
        if (!anyPadTouchpad() || !root.hasWindowFocus()) return;
        if (!root.isFocused()) root.requestFocus();
        root.setOnCapturedPointerListener((v, e) -> {
            if (!padSeen) {
                padSeen = true;
                Log.i(TAG, "pad touchpad: first event from " + e.getDevice());
            }
            InputDevice d = e.getDevice();
            if (d == null) return false;
            // A mouse or a keyboard's trackpad, not a pad's: it moves by how far it travelled.
            if (!fromPad(d.getSources())) {
                if ((e.getSource() & InputDevice.SOURCE_MOUSE_RELATIVE) == InputDevice.SOURCE_MOUSE_RELATIVE) {
                    return mouseEvent(e);
                }
                return false;
            }
            InputDevice.MotionRange rx = d.getMotionRange(MotionEvent.AXIS_X, e.getSource());
            InputDevice.MotionRange ry = d.getMotionRange(MotionEvent.AXIS_Y, e.getSource());
            float w = rx != null ? rx.getRange() : 1920, h = ry != null ? ry.getRange() : 942;
            float x0 = rx != null ? rx.getMin() : 0, y0 = ry != null ? ry.getMin() : 0;
            int action = e.getActionMasked();
            boolean touched = action != MotionEvent.ACTION_UP && action != MotionEvent.ACTION_CANCEL
                    && e.getPointerCount() > 0;
            boolean clicked = (e.getButtonState() & MotionEvent.BUTTON_PRIMARY) != 0;
            Native.padTouch(e.getDeviceId(), (e.getX() - x0) / Math.max(1, w),
                    (e.getY() - y0) / Math.max(1, h), touched, clicked);
            return true;
        });
        root.requestPointerCapture();
    }

    @Override
    public void onWindowFocusChanged(boolean focused) {
        super.onWindowFocusChanged(focused);
        if (focused) capturePadTouchpad();
    }

    final InputManager.InputDeviceListener padsListener = new InputManager.InputDeviceListener() {
        @Override
        public void onInputDeviceAdded(int id) {
            InputDevice d = InputDevice.getDevice(id);
            // A mouse or a trackpad arriving is captured too.
            if (d != null && !fromPad(d.getSources())) {
                capturePadTouchpad();
                return;
            }
            if (d == null || !fromPad(d.getSources()) || padGyros.containsKey(id)) return;
            Log.i(TAG, "gamepad: " + d.getName());
            // Its own gyro, through its own sensors: what aims on the Deck's pads.
            SensorManager own = d.getSensorManager();
            Sensor gyro = own.getDefaultSensor(Sensor.TYPE_GYROSCOPE);
            if (gyro != null) {
                SensorEventListener listener = new SensorEventListener() {
                    @Override
                    public void onSensorChanged(SensorEvent e) {
                        Native.padGyro(id, e.values[0], e.values[1], e.values[2]);
                    }

                    @Override
                    public void onAccuracyChanged(Sensor s, int accuracy) {}
                };
                own.registerListener(listener, gyro, SensorManager.SENSOR_DELAY_GAME);
                padGyros.put(id, listener);
            } else {
                padGyros.put(id, null);
            }
            capturePadTouchpad();
        }

        @Override
        public void onInputDeviceRemoved(int id) {
            if (padGyros.containsKey(id)) {
                padGyros.remove(id);
                Native.padGone(id);
            }
        }

        @Override
        public void onInputDeviceChanged(int id) {}
    };

    /** A game's rumble, played on every pad that has motors. */
    final Runnable rumbler = new Runnable() {
        @Override
        public void run() {
            long r = Native.rumble();
            if (r >= 0) {
                int strong = (int) ((r >> 16) & 0xffff), weak = (int) (r & 0xffff);
                int amplitude = Math.max(strong, weak) / 257;
                for (int id : padGyros.keySet()) {
                    InputDevice d = InputDevice.getDevice(id);
                    if (d == null) continue;
                    VibratorManager motors = d.getVibratorManager();
                    if (amplitude == 0) {
                        motors.cancel();
                    } else {
                        motors.vibrate(CombinedVibration.createParallel(
                                VibrationEffect.createOneShot(500, Math.max(1, Math.min(255, amplitude)))));
                    }
                }
            }
            ui.postDelayed(this, 30);
        }
    };

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
        AudioDeviceInfo[] ins = audio.getDevices(AudioManager.GET_DEVICES_INPUTS);
        fillPicker(output, outs, "Out");
        fillPicker(input, ins, "In");
        input.setOnItemSelectedListener(new AdapterView.OnItemSelectedListener() {
            @Override
            public void onItemSelected(AdapterView<?> parent, View view, int position, long id) {
                Native.audioInput(position == 0 ? 0 : ins[position - 1].getId());
            }

            @Override
            public void onNothingSelected(AdapterView<?> parent) {}
        });
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
        if (UsbManager.ACTION_USB_DEVICE_ATTACHED.equals(intent.getAction())) takeGlasses(true);
    }

    @Override
    protected void onDestroy() {
        ui.removeCallbacks(refresh);
        ui.removeCallbacks(rumbler);
        getSystemService(InputManager.class).unregisterInputDeviceListener(padsListener);
        getSystemService(DisplayManager.class).unregisterDisplayListener(displays);
        unregisterReceiver(usbPermission);
        unregisterReceiver(usbAttached);
        unregisterReceiver(usbDetached);
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
        showReminder();
    }

    /**
     * Plugged in while running. Asked for here if Android has not handed them over: the app is
     * already in front, so its own question would only come after ours anyway.
     */
    final BroadcastReceiver usbAttached = new BroadcastReceiver() {
        @Override
        public void onReceive(Context c, Intent i) {
            if (connection == null) takeGlasses(true);
        }
    };

    /** Unplugged: let the session go of them, and ask for them back on the phone. */
    final BroadcastReceiver usbDetached = new BroadcastReceiver() {
        @Override
        public void onReceive(Context c, Intent i) {
            UsbDevice d = i.getParcelableExtra(UsbManager.EXTRA_DEVICE, UsbDevice.class);
            if (d == null || d.getVendorId() != VENDOR || d.getProductId() != PRODUCT) return;
            Log.i(TAG, "glasses unplugged");
            if (connection != null) {
                Native.stop();
                connection.close();
                connection = null;
            }
            showReminder();
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
            // Recentre: the one thing wanted without looking, from anywhere. STEAM, the HUD,
            // has a button of its own on the bar.
            if (ORANGE_DOWN.equals(i.getAction())) {
                buzz(0);
                Native.recenter();
            }
        }
    };

    // ------------------------------------------------------------------ the phone's screen

    final Runnable refresh = new Runnable() {
        @Override
        public void run() {
            status.setText(Native.status());
            ui.postDelayed(this, 1000);
        }
    };
}
