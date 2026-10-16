import id.prasetya.spatiand.mesh.Wire;

/**
 * Android's engine reads and writes what SpatiMesh's window does: the bytes below were made
 * by crates/spatimesh/src/wire.rs (`cargo test -p spatimesh golden -- --nocapture`).
 *
 *   javac -d build/test android/src/id/prasetya/spatiand/mesh/Wire.java android/test/WireTest.java
 *   java -cp build/test WireTest
 */
public class WireTest {
    static int failures = 0;

    static byte[] bytes(String hex) {
        byte[] b = new byte[hex.length() / 2];
        for (int i = 0; i < b.length; i++) b[i] = (byte) Integer.parseInt(hex.substring(i * 2, i * 2 + 2), 16);
        return b;
    }

    static String hex(byte[] b) {
        StringBuilder s = new StringBuilder();
        for (byte x : b) s.append(String.format("%02x", x & 0xff));
        return s.toString();
    }

    static Wire.ToHost read(String hex) {
        byte[] all = bytes(hex);
        byte[] body = new byte[all.length - 5];
        System.arraycopy(all, 5, body, 0, body.length);
        return Wire.decode(all[0] & 0xff, body);
    }

    static void check(String what, boolean ok) {
        System.out.println((ok ? "ok    " : "FAIL  ") + what);
        if (!ok) failures++;
    }

    static void same(String what, byte[] made, String expected) {
        boolean ok = hex(made).equals(expected);
        check(what, ok);
        if (!ok) System.out.println("      made " + hex(made) + "\n      want " + expected);
    }

    public static void main(String[] args) {
        Wire.ToHost open = read("012e00000007000000000000001600000068747470733a2f2f6578616d706c652e636f6d2fc3a42c060000640300000000a03f");
        check("open", open != null && open.tag == Wire.OPEN && open.tab == 7 && open.url.equals("https://example.com/ä")
                && open.width == 1580 && open.height == 868 && open.zoom == 1.25f);
        Wire.ToHost pointer = read("091300000007000000000000000000484100002042010101");
        check("pointer", pointer != null && pointer.x == 12.5f && pointer.y == 40f && pointer.button == 1 && pointer.pressed && pointer.modifiers == 1);
        Wire.ToHost scroll = read("0b1900000007000000000000000000803f00000040000000000000f0c200");
        check("scroll", scroll != null && scroll.x == 1f && scroll.y == 2f && scroll.dx == 0f && scroll.dy == -120f);
        Wire.ToHost key = read("0c1a00000007000000000000001e0000000f61000000010000006101010002");
        check("key", key != null && key.code == 30 && key.key == 15 && key.ch == 'a' && key.text.equals("a") && key.hasText
                && key.pressed && !key.repeat && key.modifiers == 2);
        Wire.ToHost choose = read("120c000000070000000000000002000000");
        check("choose", choose != null && choose.tag == Wire.CHOOSE && choose.index == 2);
        Wire.ToHost answer = read("13110000000700000000000000010103000000796573");
        check("answer", answer != null && answer.accepted && answer.hasText && answer.text.equals("yes"));
        Wire.ToHost release = read("1009000000070000000000000001");
        check("release", release != null && release.tag == Wire.RELEASE && release.buffer == 1);
        Wire.ToHost quit = read("1100000000");
        check("quit", quit != null && quit.tag == Wire.QUIT);
        Wire.ToHost extensions = read("1415000000110000007b22657874656e73696f6e73223a5b5d7d");
        check("extensions", extensions != null && extensions.tag == Wire.EXTENSIONS && extensions.set.equals("{\"extensions\":[]}"));
        Wire.ToHost run = read("152a00000007000000000000000d00000073706174696d6573682d6162630103000000312b312a0000000000000001");
        check("run script", run != null && run.tag == Wire.RUN_SCRIPT && run.tab == 7 && run.world.equals("spatimesh-abc") && run.allFrames
                && run.script.equals("1+1") && run.request == 42 && run.boot);

        same("hello", Wire.hello("Android System WebView 140"), "011e0000001a000000416e64726f69642053797374656d205765625669657720313430");
        same("buffers", Wire.buffers(7, 1580, 868, true, 2), "021200000007000000000000002c060000640300000102");
        same("picture", Wire.picture(7, 0, 1580, 868), "031d0000000700000000000000000100000000000000000000002c06000064030000");
        same("title", Wire.title(7, "Ä title"), "0a14000000070000000000000008000000c384207469746c65");
        same("progress", Wire.progress(7, 0.5f, true), "0c0d00000007000000000000000000003f01");
        same("open tab", Wire.openTab(7, "https://a/", false), "111700000007000000000000000a00000068747470733a2f2f612f00");
        same("dialog", Wire.dialog(7, Wire.CONFIRM, "Sure?", ""), "161600000007000000000000000105000000537572653f00000000");
        same("extension", Wire.extension(7, "{\"c\":\"ready\"}"), "171900000007000000000000000d0000007b2263223a227265616479227d");
        same("script result", Wire.scriptResult(7, 42, true, "2"), "181600000007000000000000002a00000000000000010100000032");
        same("script failed", Wire.scriptResult(7, 43, false, "no"), "181700000007000000000000002b0000000000000000020000006e6f");

        if (failures > 0) {
            System.out.println(failures + " check(s) failed");
            System.exit(1);
        }
    }
}
