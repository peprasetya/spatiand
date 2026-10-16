package id.prasetya.spatiand.mesh;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Dnr.java, the declarativeNetRequest matcher of Android's engine, on rules as the window digests
 * them (crates/spatimesh/src/extensions/dnr.rs's `digest`: the expressions below are what its
 * url_filter_regex makes of the urlFilters in the comments), with the same answers as dnr.rs's
 * own tests. Run with a plain JDK: javac Dnr.java DnrTest.java; java id.prasetya.spatiand.mesh.DnrTest.
 */
public final class DnrTest {
    private static int checks;

    private static void check(boolean ok, String what) {
        checks++;
        if (!ok)
            throw new AssertionError(what);
    }

    /** A rule from key, value pairs: lists as String[], the rest as their text. */
    private static Dnr.Fields rule(Object... pairs) {
        Map<String, Object> m = new HashMap<>();
        for (int i = 0; i < pairs.length; i += 2)
            m.put((String) pairs[i], pairs[i + 1]);
        return new Dnr.Fields() {
            public String text(String key) {
                Object v = m.get(key);
                return v == null ? null : String.valueOf(v);
            }

            public long number(String key, long otherwise) {
                Object v = m.get(key);
                return v == null ? otherwise : ((Number) v).longValue();
            }

            public boolean flag(String key) {
                return Boolean.TRUE.equals(m.get(key));
            }

            public List<String> list(String key) {
                Object v = m.get(key);
                return v == null ? null : new ArrayList<>(Arrays.asList((String[]) v));
            }
        };
    }

    private static final String[] ALL_BUT_TOP = {"sub_frame", "stylesheet", "script", "image", "font", "object", "xmlhttprequest", "ping", "csp_report", "media",
            "websocket", "webtransport", "webbundle", "other"};
    private static final String[] FRAMES = {"main_frame", "sub_frame"};

    private static String blockOf(Dnr.Decision d) {
        return d == null ? "nothing" : d.block ? "block" : "to " + d.to;
    }

    public static void main(String[] args) {
        Dnr dnr = new Dnr();
        dnr.add("ext", Arrays.asList(
                // ||ads.example^ for third parties' scripts.
                rule("a", "block", "p", 1L, "r", "^[a-zA-Z][a-zA-Z0-9+.-]*://(?:[^/?#@]*@)?(?:[^/?#:@]*\\.)?ads\\.example(?:[^a-zA-Z0-9_.%-]|$)", "d", "ads.example",
                        "t", new String[] {"script"}, "tp", true),
                // /blocked for what scripts fetch.
                rule("a", "block", "p", 1L, "r", "/blocked", "t", new String[] {"xmlhttprequest"}),
                // track.js redirected to the extension's own stub, above an allow of a lower priority.
                rule("a", "allow", "p", 1L, "r", "track\\.js", "t", ALL_BUT_TOP),
                rule("a", "redirect", "p", 2L, "r", "track\\.js", "t", ALL_BUT_TOP, "to", "https://ext.spatimesh-extension.invalid/stub.js"),
                // An allow and a block of one priority: the allow.
                rule("a", "block", "p", 5L, "r", "both", "t", ALL_BUT_TOP),
                rule("a", "allow", "p", 5L, "r", "both", "t", ALL_BUT_TOP),
                // A page of trusted.example lets all its requests through.
                rule("a", "allowAllRequests", "p", 9L, "r", "^[a-zA-Z][a-zA-Z0-9+.-]*://(?:[^/?#@]*@)?(?:[^/?#:@]*\\.)?trusted\\.example", "d", "trusted.example",
                        "t", FRAMES),
                // http:// upgraded; a regexSubstitution.
                rule("a", "upgradeScheme", "p", 1L, "r", "^http://insecure\\.example/", "t", ALL_BUT_TOP),
                rule("a", "redirect", "p", 1L, "r", "^https://old\\.example/(.*)$", "t", ALL_BUT_TOP, "sub", "https://new.example/\\1"),
                // Methods and initiators.
                rule("a", "block", "p", 1L, "r", "/form", "t", ALL_BUT_TOP, "m", new String[] {"post"}, "xi", new String[] {"friend.example"})));
        String page = "https://site.example/page";

        check(dnr.decide("https://cdn.ads.example/x.js", page, "script", "get").block, "a third party's script from ads.example");
        check(dnr.decide("https://cdn.ADS.example/x.js", page, "script", "get") != null, "case does not matter");
        check(dnr.decide("https://ads.example/x.js", "https://www.ads.example/", "script", "get") == null, "not from its own site");
        check(dnr.decide("https://ads.example/x.png", page, "image", "get") == null, "nor an image");
        check(dnr.decide("https://notads.example/x.js", page, "script", "get") == null, "nor another domain ending the same");

        check(dnr.decide("https://site.example/blocked.txt", page, "xmlhttprequest", "get").block, "a fetch of /blocked");
        check(dnr.decide("https://site.example/fine.txt", page, "xmlhttprequest", "get") == null, "not another");
        check(dnr.decide("https://site.example/blocked.txt", null, "main_frame", "get") == null, "nor the top frame");

        check(blockOf(dnr.decide("https://t.example/track.js", page, "script", "get")).equals("to https://ext.spatimesh-extension.invalid/stub.js"),
                "the higher priority's redirect");
        check(dnr.decide("https://site.example/both", page, "script", "get") == null, "allow before block at one priority");
        check(dnr.decide("https://cdn.ads.example/x.js", "https://www.trusted.example/", "script", "get") == null, "a trusted page's requests let through");
        check(dnr.decide("https://site.example/blocked", "https://trusted.example/a", "xmlhttprequest", "get") == null, "all of them");

        check(blockOf(dnr.decide("http://insecure.example/a?b", page, "image", "get")).equals("to https://insecure.example/a?b"), "upgraded");
        check(blockOf(dnr.decide("https://old.example/x/y", page, "image", "get")).equals("to https://new.example/x/y"), "substituted");

        check(dnr.decide("https://site.example/form", page, "xmlhttprequest", "post").block, "a POST");
        check(dnr.decide("https://site.example/form", page, "xmlhttprequest", "get") == null, "not a GET");
        check(dnr.decide("https://site.example/form", "https://a.friend.example/", "xmlhttprequest", "post") == null, "not from a friend");

        // Across extensions: a block wins over another's redirect, and one's allow decides
        // nothing for another.
        Dnr two = new Dnr();
        two.add("a", Arrays.asList(rule("a", "redirect", "p", 1L, "r", "x", "t", ALL_BUT_TOP, "to", "https://elsewhere.example/")));
        two.add("b", Arrays.asList(rule("a", "allow", "p", 1L, "r", "x", "t", ALL_BUT_TOP)));
        two.add("c", Arrays.asList(rule("a", "block", "p", 1L, "r", "xx", "t", ALL_BUT_TOP)));
        check(blockOf(two.decide("https://s.example/x", page, "image", "get")).equals("to https://elsewhere.example/"), "a redirect, another's allow notwithstanding");
        check(two.decide("https://s.example/xx", page, "image", "get").block, "a block wins");

        // An expression Java cannot read leaves its rule out, not the others.
        Dnr bad = new Dnr();
        bad.add("e", Arrays.asList(rule("a", "block", "r", "(", "t", ALL_BUT_TOP), rule("a", "block", "r", "ok", "t", ALL_BUT_TOP)));
        check(bad.decide("https://s.example/ok", page, "image", "get").block, "the readable one stays");

        // What a request is for, as far as the WebView says.
        Map<String, String> html = new HashMap<>();
        html.put("Accept", "text/html,application/xhtml+xml");
        Map<String, String> css = new HashMap<>();
        css.put("accept", "text/css,*/*;q=0.1");
        Map<String, String> any = new HashMap<>();
        any.put("Accept", "*/*");
        check(Dnr.typeOf(true, "https://a/", html).equals("main_frame"), "the top frame");
        check(Dnr.typeOf(false, "https://a/f", html).equals("sub_frame"), "a frame");
        check(Dnr.typeOf(false, "https://a/s", css).equals("stylesheet"), "a style sheet");
        check(Dnr.typeOf(false, "https://a/x.js?v=1", any).equals("script"), "a script");
        check(Dnr.typeOf(false, "https://a/f.woff2", any).equals("font"), "a font");
        check(Dnr.typeOf(false, "https://a/api/data", any).equals("xmlhttprequest"), "a fetch");
        check(Dnr.typeOf(false, "https://a.js/api", null).equals("xmlhttprequest"), "a host's dot is no ending");

        System.out.println("DnrTest: " + checks + " checks passed");
    }
}
