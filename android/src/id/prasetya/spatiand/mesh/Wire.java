package id.prasetya.spatiand.mesh;

import java.io.ByteArrayOutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;

/**
 * What SpatiMesh's window and this engine say to each other: the same bytes as
 * crates/spatimesh/src/wire.rs, which is the authority (android/test/WireTest.java checks this
 * against bytes it produced). Each message is a byte saying what it is, four (little-endian)
 * saying how long, and that much.
 *
 * Plain Java, so that it is tested off Android.
 */
public final class Wire {
    private Wire() {}

    // From the window.
    public static final int OPEN = 1, CLOSE = 2, LOAD = 3, BACK = 4, FORWARD = 5, RELOAD = 6, STOP = 7,
            RESIZE = 8, POINTER = 9, POINTER_LEFT = 10, SCROLL = 11, KEY = 12, FOCUS = 13, WINDOW = 14,
            MUTE = 15, RELEASE = 16, QUIT = 17, CHOOSE = 18, ANSWER = 19;

    // Dialogs, as Happened::Dialog numbers them.
    public static final int ALERT = 0, CONFIRM = 1, PROMPT = 2, LEAVE = 3;

    /** A message from the window, read: the fields its kind has. */
    public static final class ToHost {
        public int tag;
        public long tab;
        public String url = "", text = "";
        public boolean hasText;
        public int width, height;
        public float zoom = 1f;
        public float x, y, dx, dy;
        /** 0 none, 1 left, 2 middle, 3 right, 4 back, 5 forward. */
        public int button;
        public boolean pressed, repeat, focused, muted, accepted;
        /** 1 Shift, 2 Control, 4 Alt, 8 the logo key. */
        public int modifiers;
        /** Linux's number for the key; the wire's name for it (1 Enter ... 15 a character); the character. */
        public int code, key, ch;
        public int window, buffer;
        /** What was chosen from a list: -1 for nothing. */
        public int index = -1;
    }

    private static final class In {
        final ByteBuffer b;

        In(byte[] body) {
            b = ByteBuffer.wrap(body).order(ByteOrder.LITTLE_ENDIAN);
        }

        int u8() { return b.get() & 0xff; }
        int u32() { return b.getInt(); }
        long u64() { return b.getLong(); }
        float f32() { return b.getFloat(); }
        boolean bool() { return u8() != 0; }

        String str() {
            byte[] s = new byte[u32()];
            b.get(s);
            return new String(s, StandardCharsets.UTF_8);
        }
    }

    /** One message's body, read; null for one this engine does not know. */
    public static ToHost decode(int tag, byte[] body) {
        In i = new In(body);
        ToHost m = new ToHost();
        m.tag = tag;
        try {
            switch (tag) {
                case OPEN:
                    m.tab = i.u64(); m.url = i.str(); m.width = i.u32(); m.height = i.u32(); m.zoom = i.f32();
                    break;
                case LOAD:
                    m.tab = i.u64(); m.url = i.str();
                    break;
                case CLOSE: case BACK: case FORWARD: case RELOAD: case STOP: case POINTER_LEFT:
                    m.tab = i.u64();
                    break;
                case RESIZE:
                    m.tab = i.u64(); m.width = i.u32(); m.height = i.u32(); m.zoom = i.f32();
                    break;
                case POINTER:
                    m.tab = i.u64(); m.x = i.f32(); m.y = i.f32(); m.button = i.u8(); m.pressed = i.bool(); m.modifiers = i.u8();
                    break;
                case SCROLL:
                    m.tab = i.u64(); m.x = i.f32(); m.y = i.f32(); m.dx = i.f32(); m.dy = i.f32(); m.modifiers = i.u8();
                    break;
                case KEY:
                    m.tab = i.u64(); m.code = i.u32(); m.key = i.u8(); m.ch = i.u32(); m.text = i.str(); m.hasText = i.bool();
                    m.pressed = i.bool(); m.repeat = i.bool(); m.modifiers = i.u8();
                    break;
                case FOCUS:
                    m.tab = i.u64(); m.focused = i.bool();
                    break;
                case WINDOW:
                    m.tab = i.u64(); m.window = i.u32();
                    break;
                case MUTE:
                    m.tab = i.u64(); m.muted = i.bool();
                    break;
                case RELEASE:
                    m.tab = i.u64(); m.buffer = i.u8();
                    break;
                case QUIT:
                    break;
                case CHOOSE: {
                    m.tab = i.u64();
                    int index = i.u32();
                    m.index = index == -1 ? -1 : index;
                    break;
                }
                case ANSWER:
                    m.tab = i.u64(); m.accepted = i.bool(); m.hasText = i.bool(); m.text = i.str();
                    break;
                default:
                    return null;
            }
        } catch (RuntimeException e) {
            return null;
        }
        return m;
    }

    /** A message being written. */
    private static final class Out {
        final ByteArrayOutputStream body = new ByteArrayOutputStream();
        final int tag;

        Out(int tag) {
            this.tag = tag;
        }

        Out u8(int v) { body.write(v & 0xff); return this; }

        Out u32(int v) {
            for (int s = 0; s < 32; s += 8) body.write((v >>> s) & 0xff);
            return this;
        }

        Out u64(long v) {
            for (int s = 0; s < 64; s += 8) body.write((int) ((v >>> s) & 0xff));
            return this;
        }

        Out f32(float v) { return u32(Float.floatToIntBits(v)); }
        Out bool(boolean v) { return u8(v ? 1 : 0); }

        Out str(String s) {
            byte[] b = (s == null ? "" : s).getBytes(StandardCharsets.UTF_8);
            u32(b.length);
            body.write(b, 0, b.length);
            return this;
        }

        byte[] bytes() {
            byte[] b = body.toByteArray();
            ByteBuffer f = ByteBuffer.allocate(5 + b.length).order(ByteOrder.LITTLE_ENDIAN);
            f.put((byte) tag).putInt(b.length).put(b);
            return f.array();
        }
    }

    public static byte[] hello(String engine) { return new Out(1).str(engine).bytes(); }

    /** A tab's buffers: `count` descriptors go with this message. */
    public static byte[] buffers(long tab, int width, int height, boolean rgba, int count) {
        return new Out(2).u64(tab).u32(width).u32(height).u8(rgba ? 1 : 0).u8(count).bytes();
    }

    /** The newest picture is in `buffer`; all of it is new. */
    public static byte[] picture(long tab, int buffer, int width, int height) {
        return new Out(3).u64(tab).u8(buffer).u32(1).u32(0).u32(0).u32(width).u32(height).bytes();
    }

    public static byte[] title(long tab, String title) { return new Out(10).u64(tab).str(title).bytes(); }
    public static byte[] url(long tab, String url) { return new Out(11).u64(tab).str(url).bytes(); }

    public static byte[] progress(long tab, float progress, boolean loading) {
        return new Out(12).u64(tab).f32(progress).bool(loading).bytes();
    }

    public static byte[] history(long tab, boolean back, boolean forward) {
        return new Out(13).u64(tab).bool(back).bool(forward).bytes();
    }

    public static byte[] sound(long tab, boolean playing) { return new Out(16).u64(tab).bool(playing).bytes(); }

    public static byte[] openTab(long opener, String url, boolean background) {
        return new Out(17).u64(opener).str(url).bool(background).bytes();
    }

    public static byte[] closeTab(long tab) { return new Out(18).u64(tab).bytes(); }
    public static byte[] crashed(long tab, String why) { return new Out(19).u64(tab).str(why).bytes(); }

    public static byte[] failed(long tab, String url, String why) {
        return new Out(20).u64(tab).str(url).str(why).bytes();
    }

    /** A <select>'s list: its lines as label, enabled, selected, heading. */
    public static byte[] choose(long tab, int x, int y, int width, int height, String[] labels, boolean[] enabled, boolean[] selected) {
        Out o = new Out(21).u64(tab).u32(x).u32(y).u32(width).u32(height).u32(labels.length);
        for (int k = 0; k < labels.length; k++) o.str(labels[k]).bool(enabled[k]).bool(selected[k]).bool(false);
        return o.bytes();
    }

    public static byte[] dialog(long tab, int kind, String message, String text) {
        return new Out(22).u64(tab).u8(kind).str(message).str(text).bytes();
    }
}
