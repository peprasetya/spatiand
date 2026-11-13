package id.prasetya.spatiand;

import android.Manifest;
import android.app.Activity;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothDevice;
import android.bluetooth.BluetoothGatt;
import android.bluetooth.BluetoothGattCallback;
import android.bluetooth.BluetoothGattCharacteristic;
import android.bluetooth.BluetoothGattDescriptor;
import android.bluetooth.BluetoothGattService;
import android.bluetooth.BluetoothManager;
import android.bluetooth.BluetoothProfile;
import android.bluetooth.le.BluetoothLeScanner;
import android.bluetooth.le.ScanCallback;
import android.bluetooth.le.ScanRecord;
import android.bluetooth.le.ScanResult;
import android.bluetooth.le.ScanSettings;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.PackageManager;
import android.net.LocalServerSocket;
import android.net.LocalSocket;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.ParcelUuid;
import android.os.Process;
import android.util.Log;
import android.util.SparseArray;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.lang.ref.WeakReference;
import java.nio.charset.StandardCharsets;
import java.util.ArrayDeque;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.UUID;

/**
 * Web Bluetooth for SpatiMesh's Chromium on the Beam Pro. Chromium's Linux build reaches Bluetooth
 * through BlueZ on D-Bus, which Android has not got; this is its way to Android's Bluetooth LE: an
 * abstract socket, "spatiand-bluetooth", for this app's own processes alone, carrying one JSON object
 * a line each way. Chromium's half is device/bluetooth/spatiand (third_party/chromium/android-libc).
 *
 * Asked of it: "hello" (the adapter), "scan" and "stopScan", "connect" and "disconnect" a device,
 * "read", "write" and "notify" a characteristic, "readDescriptor" and "writeDescriptor". It tells:
 * "adapter", "device" (each advertisement seen), "connection", "services" (all of a device's, once
 * discovered), "read", "wrote", "notifying", "changed", "descriptorRead", "descriptorWrote". Values
 * are hex. Services, characteristics and descriptors are named by the device's address and their
 * places in what Android discovered ("AA:BB:../s0/c1/d0").
 *
 * Android's Bluetooth asks the wearer once (BLUETOOTH_SCAN, BLUETOOTH_CONNECT): the first page that
 * uses Bluetooth brings the question up on the phone, and until it is answered the adapter is off.
 */
final class BluetoothBridge {
    static final String TAG = "spatiand";
    static final String SOCKET = "spatiand-bluetooth";
    static final UUID CLIENT_CONFIG = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb");
    static final String[] PERMISSIONS = {
        Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT
    };
    static final int PERMISSION_REQUEST = 7;

    private static BluetoothBridge running;

    private final Context context;
    private volatile WeakReference<Activity> activity;
    private final Handler handler;
    private final BluetoothManager manager;
    private Client client;

    /** Started once, by the app; an activity made later is the one Android's question is asked through. */
    static synchronized void start(Activity activity) {
        if (running != null) {
            running.activity = new WeakReference<>(activity);
            return;
        }
        running = new BluetoothBridge(activity);
    }

    /** The wearer answered Android's question. */
    static synchronized void permissionsAnswered() {
        if (running != null) running.handler.post(running::sayAdapter);
    }

    private BluetoothBridge(Activity activity) {
        this.context = activity.getApplicationContext();
        this.activity = new WeakReference<>(activity);
        HandlerThread thread = new HandlerThread("bluetooth");
        thread.start();
        handler = new Handler(thread.getLooper());
        manager = context.getSystemService(BluetoothManager.class);
        context.registerReceiver(new BroadcastReceiver() {
            @Override
            public void onReceive(Context c, Intent intent) {
                handler.post(BluetoothBridge.this::sayAdapter);
            }
        }, new IntentFilter(BluetoothAdapter.ACTION_STATE_CHANGED), Context.RECEIVER_NOT_EXPORTED);
        new Thread(this::listen, "bluetooth-socket").start();
    }

    private void listen() {
        try (LocalServerSocket server = new LocalServerSocket(SOCKET)) {
            while (true) {
                LocalSocket socket = server.accept();
                // This app's own processes alone: an abstract socket is anyone's to reach.
                if (socket.getPeerCredentials().getUid() != Process.myUid()) {
                    socket.close();
                    continue;
                }
                Client c = new Client(socket);
                handler.post(() -> {
                    if (client != null) client.close();
                    client = c;
                });
                new Thread(c::read, "bluetooth-client").start();
            }
        } catch (IOException e) {
            Log.w(TAG, "Bluetooth's socket closed: " + e);
        }
    }

    private boolean permitted() {
        for (String p : PERMISSIONS) {
            if (context.checkSelfPermission(p) != PackageManager.PERMISSION_GRANTED) return false;
        }
        return true;
    }

    private void askPermission() {
        Activity a = activity.get();
        if (a == null) return;
        a.runOnUiThread(() -> a.requestPermissions(PERMISSIONS, PERMISSION_REQUEST));
    }

    private BluetoothAdapter adapter() {
        return manager == null ? null : manager.getAdapter();
    }

    private void sayAdapter() {
        if (client == null) return;
        BluetoothAdapter adapter = adapter();
        JSONObject o = event("adapter");
        put(o, "present", adapter != null);
        boolean permitted = permitted();
        put(o, "permitted", permitted);
        put(o, "powered", adapter != null && permitted && adapter.isEnabled());
        if (adapter != null && permitted) {
            try {
                put(o, "name", adapter.getName());
                put(o, "address", adapter.getAddress());
            } catch (SecurityException e) {
                // Asked again below.
            }
        }
        client.send(o);
    }

    // ---------------------------------------------------------------------- what Chromium asks

    private void handle(JSONObject m) {
        String op = m.optString("op");
        try {
            switch (op) {
                case "hello":
                    if (!permitted()) askPermission();
                    sayAdapter();
                    break;
                case "scan":
                    scan();
                    break;
                case "stopScan":
                    stopScan();
                    break;
                case "connect":
                    connect(m.getString("address"));
                    break;
                case "disconnect": {
                    Device d = client.devices.get(m.getString("address"));
                    if (d != null && d.gatt != null) d.gatt.disconnect();
                    break;
                }
                case "read":
                case "write":
                case "notify":
                case "readDescriptor":
                case "writeDescriptor": {
                    Device d = client.devices.get(m.getString("address"));
                    if (d == null || d.gatt == null) {
                        failed(m, BluetoothGatt.GATT_FAILURE);
                    } else {
                        d.queue.add(m);
                        if (d.queue.size() == 1) d.next();
                    }
                    break;
                }
                default:
                    Log.w(TAG, "Bluetooth: no such request: " + op);
            }
        } catch (JSONException | SecurityException | IllegalArgumentException e) {
            Log.w(TAG, "Bluetooth request " + op + " failed: " + e);
            failed(m, BluetoothGatt.GATT_FAILURE);
        }
    }

    /** The answer to a request that could not be made. */
    private void failed(JSONObject m, int status) {
        String op = m.optString("op");
        String answer = op.equals("read") ? "read" : op.equals("write") ? "wrote"
                : op.equals("notify") ? "notifying" : op.equals("readDescriptor") ? "descriptorRead"
                : op.equals("writeDescriptor") ? "descriptorWrote" : null;
        if (answer == null || client == null) return;
        JSONObject o = event(answer);
        put(o, "id", m.optString("id"));
        put(o, "status", status);
        client.send(o);
    }

    private final ScanCallback scanning = new ScanCallback() {
        @Override
        public void onScanResult(int type, ScanResult result) {
            handler.post(() -> sayDevice(result));
        }

        @Override
        public void onBatchScanResults(List<ScanResult> results) {
            handler.post(() -> {
                for (ScanResult r : results) sayDevice(r);
            });
        }

        @Override
        public void onScanFailed(int code) {
            Log.w(TAG, "Bluetooth scan failed: " + code);
            handler.post(() -> {
                if (client != null) client.send(event("scanFailed"));
            });
        }
    };
    private boolean isScanning;

    private void scan() {
        BluetoothAdapter adapter = adapter();
        if (adapter == null || !permitted()) return;
        BluetoothLeScanner scanner = adapter.getBluetoothLeScanner();
        if (scanner == null) return;
        if (isScanning) scanner.stopScan(scanning);
        // Unfiltered: Chromium filters what it is shown (by services, names and prefixes).
        scanner.startScan(null, new ScanSettings.Builder()
                .setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build(), scanning);
        isScanning = true;
    }

    private void stopScan() {
        BluetoothAdapter adapter = adapter();
        if (!isScanning || adapter == null || adapter.getBluetoothLeScanner() == null) return;
        adapter.getBluetoothLeScanner().stopScan(scanning);
        isScanning = false;
    }

    private void sayDevice(ScanResult result) {
        if (client == null) return;
        BluetoothDevice device = result.getDevice();
        JSONObject o = event("device");
        put(o, "address", device.getAddress());
        ScanRecord record = result.getScanRecord();
        String name = null;
        try {
            name = device.getName();
        } catch (SecurityException e) {
            // From the advertisement, then.
        }
        if (name == null && record != null) name = record.getDeviceName();
        if (name != null) put(o, "name", name);
        put(o, "rssi", result.getRssi());
        if (record != null) {
            if (record.getTxPowerLevel() != Integer.MIN_VALUE) put(o, "txPower", record.getTxPowerLevel());
            put(o, "flags", record.getAdvertiseFlags());
            JSONArray uuids = new JSONArray();
            if (record.getServiceUuids() != null) {
                for (ParcelUuid u : record.getServiceUuids()) uuids.put(u.toString());
            }
            put(o, "uuids", uuids);
            JSONObject serviceData = new JSONObject();
            if (record.getServiceData() != null) {
                for (Map.Entry<ParcelUuid, byte[]> e : record.getServiceData().entrySet()) {
                    put(serviceData, e.getKey().toString(), hex(e.getValue()));
                }
            }
            put(o, "serviceData", serviceData);
            JSONObject manufacturerData = new JSONObject();
            SparseArray<byte[]> m = record.getManufacturerSpecificData();
            if (m != null) {
                for (int i = 0; i < m.size(); i++) put(manufacturerData, String.valueOf(m.keyAt(i)), hex(m.valueAt(i)));
            }
            put(o, "manufacturerData", manufacturerData);
        }
        client.send(o);
    }

    private void connect(String address) {
        BluetoothAdapter adapter = adapter();
        if (adapter == null || client == null) return;
        Device d = client.devices.get(address);
        if (d == null) {
            d = new Device(address);
            client.devices.put(address, d);
        }
        if (d.gatt != null) {
            // Asked again while connected: say so again.
            if (d.connected) d.sayConnection(true, BluetoothGatt.GATT_SUCCESS);
            else d.gatt.connect();
            return;
        }
        d.gatt = adapter.getRemoteDevice(address).connectGatt(context, false, d, BluetoothDevice.TRANSPORT_LE);
    }

    // ---------------------------------------------------------------------- one device

    /** A device Chromium connected to: its GATT and the requests waiting their turn. */
    private final class Device extends BluetoothGattCallback {
        final String address;
        BluetoothGatt gatt;
        boolean connected;
        final ArrayDeque<JSONObject> queue = new ArrayDeque<>();
        final Map<String, BluetoothGattCharacteristic> characteristics = new HashMap<>();
        final Map<String, BluetoothGattDescriptor> descriptors = new HashMap<>();
        final Map<BluetoothGattCharacteristic, String> characteristicIds = new HashMap<>();
        final Map<BluetoothGattDescriptor, String> descriptorIds = new HashMap<>();

        Device(String address) {
            this.address = address;
        }

        void sayConnection(boolean isConnected, int status) {
            if (client == null) return;
            JSONObject o = event("connection");
            put(o, "address", address);
            put(o, "connected", isConnected);
            put(o, "status", status);
            client.send(o);
        }

        @Override
        public void onConnectionStateChange(BluetoothGatt g, int status, int state) {
            handler.post(() -> {
                connected = state == BluetoothProfile.STATE_CONNECTED;
                sayConnection(connected, status);
                if (connected) {
                    try {
                        // As large a packet as the device takes, which BlueZ asks for by itself
                        // on the Deck: Android keeps ATT's 23 bytes unless asked, and cut a
                        // notification of more than 20 bytes short. Its services are looked
                        // for once the device has answered.
                        if (!g.requestMtu(517)) g.discoverServices();
                    } catch (SecurityException e) {
                        Log.w(TAG, "Bluetooth: " + e);
                    }
                } else {
                    // Whatever waited will not be answered by the device: answered here.
                    for (JSONObject m : queue) failed(m, BluetoothGatt.GATT_FAILURE);
                    queue.clear();
                    try {
                        g.close();
                    } catch (SecurityException e) {
                        // Closing is all that is left.
                    }
                    gatt = null;
                }
            });
        }

        @Override
        public void onMtuChanged(BluetoothGatt g, int mtu, int status) {
            handler.post(() -> {
                Log.i(TAG, "Bluetooth: " + address + " takes " + (mtu - 3) + " bytes a packet");
                if (!connected) return;
                try {
                    g.discoverServices();
                } catch (SecurityException e) {
                    Log.w(TAG, "Bluetooth: " + e);
                }
            });
        }

        @Override
        public void onServicesDiscovered(BluetoothGatt g, int status) {
            handler.post(() -> {
                characteristics.clear();
                descriptors.clear();
                characteristicIds.clear();
                descriptorIds.clear();
                JSONArray services = new JSONArray();
                List<BluetoothGattService> found = g.getServices();
                for (int s = 0; s < found.size(); s++) {
                    BluetoothGattService service = found.get(s);
                    String sid = address + "/s" + s;
                    JSONObject so = new JSONObject();
                    put(so, "id", sid);
                    put(so, "uuid", service.getUuid().toString());
                    put(so, "primary", service.getType() == BluetoothGattService.SERVICE_TYPE_PRIMARY);
                    JSONArray chars = new JSONArray();
                    List<BluetoothGattCharacteristic> cs = service.getCharacteristics();
                    for (int c = 0; c < cs.size(); c++) {
                        BluetoothGattCharacteristic ch = cs.get(c);
                        String cid = sid + "/c" + c;
                        characteristics.put(cid, ch);
                        characteristicIds.put(ch, cid);
                        JSONObject co = new JSONObject();
                        put(co, "id", cid);
                        put(co, "uuid", ch.getUuid().toString());
                        put(co, "properties", ch.getProperties());
                        put(co, "permissions", ch.getPermissions());
                        JSONArray descs = new JSONArray();
                        List<BluetoothGattDescriptor> ds = ch.getDescriptors();
                        for (int d = 0; d < ds.size(); d++) {
                            BluetoothGattDescriptor de = ds.get(d);
                            String did = cid + "/d" + d;
                            descriptors.put(did, de);
                            descriptorIds.put(de, did);
                            JSONObject dob = new JSONObject();
                            put(dob, "id", did);
                            put(dob, "uuid", de.getUuid().toString());
                            descs.put(dob);
                        }
                        put(co, "descriptors", descs);
                        chars.put(co);
                    }
                    put(so, "characteristics", chars);
                    services.put(so);
                }
                if (client == null) return;
                JSONObject o = event("services");
                put(o, "address", address);
                put(o, "status", status);
                put(o, "services", services);
                client.send(o);
            });
        }

        /** The request at the head of the queue, made. Android takes one at a time. */
        void next() {
            while (!queue.isEmpty()) {
                JSONObject m = queue.peek();
                if (start(m)) return;
                queue.poll();
                failed(m, BluetoothGatt.GATT_FAILURE);
            }
        }

        /** Its answer came: the next. */
        void done() {
            queue.poll();
            next();
        }

        private boolean start(JSONObject m) {
            if (gatt == null) return false;
            try {
                String id = m.getString("id");
                switch (m.getString("op")) {
                    case "read": {
                        BluetoothGattCharacteristic c = characteristics.get(id);
                        return c != null && gatt.readCharacteristic(c);
                    }
                    case "write": {
                        BluetoothGattCharacteristic c = characteristics.get(id);
                        if (c == null) return false;
                        int type = switch (m.optString("type")) {
                            case "noResponse" -> BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE;
                            case "signed" -> BluetoothGattCharacteristic.WRITE_TYPE_SIGNED;
                            default -> BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT;
                        };
                        return gatt.writeCharacteristic(c, unhex(m.getString("value")), type)
                                == BluetoothGatt.GATT_SUCCESS;
                    }
                    case "notify": {
                        BluetoothGattCharacteristic c = characteristics.get(id);
                        if (c == null) return false;
                        boolean enable = m.optBoolean("enable");
                        if (!gatt.setCharacteristicNotification(c, enable)) return false;
                        BluetoothGattDescriptor config = c.getDescriptor(CLIENT_CONFIG);
                        if (config == null) {
                            // Nothing to write: notifying as asked already.
                            JSONObject o = event("notifying");
                            put(o, "id", id);
                            put(o, "status", BluetoothGatt.GATT_SUCCESS);
                            client.send(o);
                            handler.post(this::done);
                            return true;
                        }
                        byte[] value = !enable ? BluetoothGattDescriptor.DISABLE_NOTIFICATION_VALUE
                                : (c.getProperties() & BluetoothGattCharacteristic.PROPERTY_NOTIFY) != 0
                                        ? BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
                                        : BluetoothGattDescriptor.ENABLE_INDICATION_VALUE;
                        return gatt.writeDescriptor(config, value) == BluetoothGatt.GATT_SUCCESS;
                    }
                    case "readDescriptor": {
                        BluetoothGattDescriptor d = descriptors.get(id);
                        return d != null && gatt.readDescriptor(d);
                    }
                    case "writeDescriptor": {
                        BluetoothGattDescriptor d = descriptors.get(id);
                        return d != null
                                && gatt.writeDescriptor(d, unhex(m.getString("value"))) == BluetoothGatt.GATT_SUCCESS;
                    }
                    default:
                        return false;
                }
            } catch (JSONException | SecurityException | IllegalArgumentException e) {
                Log.w(TAG, "Bluetooth: " + e);
                return false;
            }
        }

        @Override
        public void onCharacteristicRead(BluetoothGatt g, BluetoothGattCharacteristic c, byte[] value, int status) {
            handler.post(() -> {
                JSONObject o = event("read");
                put(o, "id", characteristicIds.get(c));
                put(o, "status", status);
                put(o, "value", hex(value));
                if (client != null) client.send(o);
                done();
            });
        }

        @Override
        public void onCharacteristicWrite(BluetoothGatt g, BluetoothGattCharacteristic c, int status) {
            handler.post(() -> {
                JSONObject o = event("wrote");
                put(o, "id", characteristicIds.get(c));
                put(o, "status", status);
                if (client != null) client.send(o);
                done();
            });
        }

        @Override
        public void onCharacteristicChanged(BluetoothGatt g, BluetoothGattCharacteristic c, byte[] value) {
            handler.post(() -> {
                JSONObject o = event("changed");
                put(o, "id", characteristicIds.get(c));
                put(o, "value", hex(value));
                if (client != null) client.send(o);
            });
        }

        @Override
        public void onDescriptorRead(BluetoothGatt g, BluetoothGattDescriptor d, int status, byte[] value) {
            handler.post(() -> {
                JSONObject o = event("descriptorRead");
                put(o, "id", descriptorIds.get(d));
                put(o, "status", status);
                put(o, "value", hex(value));
                if (client != null) client.send(o);
                done();
            });
        }

        @Override
        public void onDescriptorWrite(BluetoothGatt g, BluetoothGattDescriptor d, int status) {
            handler.post(() -> {
                JSONObject head = queue.peek();
                boolean forNotify = head != null && head.optString("op").equals("notify");
                JSONObject o = event(forNotify ? "notifying" : "descriptorWrote");
                put(o, "id", forNotify ? head.optString("id") : descriptorIds.get(d));
                put(o, "status", status);
                if (client != null) client.send(o);
                done();
            });
        }
    }

    // ---------------------------------------------------------------------- one Chromium

    /** The connected Chromium: its socket, and the devices it has connected to. */
    private final class Client {
        final LocalSocket socket;
        final OutputStream out;
        final Map<String, Device> devices = new HashMap<>();

        Client(LocalSocket socket) throws IOException {
            this.socket = socket;
            this.out = socket.getOutputStream();
        }

        void read() {
            try (BufferedReader in = new BufferedReader(
                    new InputStreamReader(socket.getInputStream(), StandardCharsets.UTF_8))) {
                String line;
                while ((line = in.readLine()) != null) {
                    JSONObject m;
                    try {
                        m = new JSONObject(line);
                    } catch (JSONException e) {
                        continue;
                    }
                    handler.post(() -> {
                        if (client == this) handle(m);
                    });
                }
            } catch (IOException e) {
                // Gone.
            }
            handler.post(() -> {
                if (client == this) {
                    close();
                    client = null;
                }
            });
        }

        synchronized void send(JSONObject o) {
            try {
                out.write((o.toString() + "\n").getBytes(StandardCharsets.UTF_8));
                out.flush();
            } catch (IOException e) {
                // Its reader sees the socket go.
            }
        }

        /** Chromium has gone: what it left connected or scanning is let go of. */
        void close() {
            stopScan();
            for (Device d : devices.values()) {
                if (d.gatt != null) {
                    try {
                        d.gatt.disconnect();
                        d.gatt.close();
                    } catch (SecurityException e) {
                        // Let go of already.
                    }
                }
            }
            devices.clear();
            try {
                socket.close();
            } catch (IOException e) {
                // Closed.
            }
        }
    }

    // ---------------------------------------------------------------------- small things

    private static JSONObject event(String name) {
        JSONObject o = new JSONObject();
        put(o, "ev", name);
        return o;
    }

    private static void put(JSONObject o, String key, Object value) {
        try {
            o.put(key, value);
        } catch (JSONException e) {
            // Only for a NaN, which nothing here puts.
        }
    }

    private static final char[] HEX = "0123456789abcdef".toCharArray();

    static String hex(byte[] bytes) {
        if (bytes == null) return "";
        char[] out = new char[bytes.length * 2];
        for (int i = 0; i < bytes.length; i++) {
            out[i * 2] = HEX[(bytes[i] >> 4) & 15];
            out[i * 2 + 1] = HEX[bytes[i] & 15];
        }
        return new String(out);
    }

    static byte[] unhex(String s) {
        byte[] out = new byte[s.length() / 2];
        for (int i = 0; i < out.length; i++) {
            out[i] = (byte) Integer.parseInt(s.substring(i * 2, i * 2 + 2), 16);
        }
        return out;
    }
}
