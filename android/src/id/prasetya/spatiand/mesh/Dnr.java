package id.prasetya.spatiand.mesh;

import java.net.URI;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.regex.PatternSyntaxException;

/**
 * declarativeNetRequest in Android's WebView, which has no rules of its own to be given: the
 * profile's extensions' rules matched here, for each request shouldInterceptRequest sees, as
 * crates/spatimesh/src/extensions/dnr.rs matches them (Chrome's order: the highest priority,
 * then allow, block, upgradeScheme, redirect; across extensions a block wins, then the first
 * that redirects). The window gives each extension's rules digested (dnr.rs's `digest`): their
 * urlFilters already regular expressions, their resource types already the list they are for.
 *
 * No Android class is used here, so that a plain JDK runs its test (android/test/DnrTest.java).
 */
final class Dnr {
    /** A rule's fields, as the window wrote them (a JSONObject's, in MeshEngine). */
    interface Fields {
        String text(String key);

        long number(String key, long otherwise);

        boolean flag(String key);

        /** null where the key is not there. */
        List<String> list(String key);
    }

    static final class Rule {
        long priority;
        /** Chrome's order among rules of one priority: lower first. */
        int rank;
        String action;
        Pattern url;
        Set<String> types, methods, initiators, requests;
        Set<String> notMethods, notInitiators, notRequests;
        Boolean thirdParty;
        String to, sub;

        boolean better(Rule than) {
            return than == null || priority > than.priority || (priority == than.priority && rank < than.rank);
        }
    }

    static final class Ext {
        final String id;
        final List<Rule> others = new ArrayList<>();
        final Map<String, List<Rule>> byDomain = new HashMap<>();
        final List<Rule> allowAll = new ArrayList<>();

        Ext(String id) {
            this.id = id;
        }
    }

    /** What is to happen to a request: blocked, or to `to` instead. */
    static final class Decision {
        final boolean block;
        final String to;

        Decision(boolean block, String to) {
            this.block = block;
            this.to = to;
        }
    }

    final List<Ext> extensions = new ArrayList<>();

    boolean isEmpty() {
        return extensions.isEmpty();
    }

    private static Set<String> set(List<String> list) {
        return list == null ? null : new HashSet<>(list);
    }

    private static Set<String> orEmpty(List<String> list) {
        return list == null ? new HashSet<>() : new HashSet<>(list);
    }

    /** An extension's rules; one whose expression Java cannot read is left out. */
    Ext add(String id, List<Fields> rules) {
        Ext ext = new Ext(id);
        for (Fields f : rules) {
            Rule r = new Rule();
            r.action = f.text("a");
            switch (r.action == null ? "" : r.action) {
                case "allow": case "allowAllRequests": r.rank = 0; break;
                case "block": r.rank = 1; break;
                case "upgradeScheme": r.rank = 2; break;
                case "redirect": r.rank = 3; break;
                default: continue;
            }
            r.priority = f.number("p", 1);
            String source = f.text("r");
            if (source != null) {
                try {
                    r.url = Pattern.compile(source, f.flag("cs") ? 0 : Pattern.CASE_INSENSITIVE);
                } catch (PatternSyntaxException e) {
                    continue;
                }
            }
            r.types = orEmpty(f.list("t"));
            r.methods = set(f.list("m"));
            r.initiators = set(f.list("i"));
            r.requests = set(f.list("q"));
            r.notMethods = orEmpty(f.list("xm"));
            r.notInitiators = orEmpty(f.list("xi"));
            r.notRequests = orEmpty(f.list("xq"));
            String tp = f.text("tp");
            r.thirdParty = tp == null ? null : Boolean.valueOf(tp);
            r.to = f.text("to");
            r.sub = f.text("sub");
            String domain = f.text("d");
            if (domain != null)
                ext.byDomain.computeIfAbsent(domain, k -> new ArrayList<>()).add(r);
            else
                ext.others.add(r);
            if (r.action.equals("allowAllRequests"))
                ext.allowAll.add(r);
        }
        extensions.add(ext);
        return ext;
    }

    /** `url`'s host, lower case, without brackets; "" if it has none. */
    static String hostOf(String url) {
        if (url == null)
            return "";
        try {
            String host = new URI(url).getHost();
            if (host == null)
                return "";
            if (host.startsWith("[") && host.endsWith("]"))
                host = host.substring(1, host.length() - 1);
            return host.toLowerCase(Locale.ROOT);
        } catch (Exception e) {
            return "";
        }
    }

    /** Whether `host` is `domain` or one of its subdomains. */
    static boolean within(String host, String domain) {
        return host.equals(domain) || (host.endsWith(domain) && host.charAt(host.length() - domain.length() - 1) == '.');
    }

    private static boolean anyWithin(String host, Set<String> domains) {
        for (String d : domains)
            if (within(host, d))
                return true;
        return false;
    }

    /** As dnr.rs's same_site: the last two labels (three where the second-last is short). */
    static String siteOf(String host) {
        if (host.indexOf(':') >= 0 || host.matches("[0-9.]+"))
            return host;
        String[] labels = host.split("\\.");
        int n = labels.length;
        if (n <= 2)
            return host;
        int take = labels[n - 2].length() <= 3 && labels[n - 1].length() == 2 ? 3 : 2;
        return String.join(".", Arrays.copyOfRange(labels, Math.max(0, n - take), n));
    }

    static boolean matches(Rule r, String url, String host, String initiatorHost, String type, String method) {
        if (!r.types.contains(type))
            return false;
        if ((r.methods != null && !r.methods.contains(method)) || r.notMethods.contains(method))
            return false;
        if ((r.requests != null && !anyWithin(host, r.requests)) || anyWithin(host, r.notRequests))
            return false;
        if (initiatorHost != null) {
            if ((r.initiators != null && !anyWithin(initiatorHost, r.initiators)) || anyWithin(initiatorHost, r.notInitiators))
                return false;
        } else if (r.initiators != null) {
            return false;
        }
        if (r.thirdParty != null) {
            boolean third = initiatorHost != null && !siteOf(initiatorHost).equals(siteOf(host));
            if (third != r.thirdParty)
                return false;
        }
        return r.url == null || r.url.matcher(url).find();
    }

    /** The rule of `ext`'s that decides a request, if one does. */
    static Rule winner(Ext ext, String url, String initiator, String type, String method) {
        String host = hostOf(url);
        String initiatorHost = initiator == null ? null : hostOf(initiator);
        if (initiatorHost != null && initiatorHost.isEmpty())
            initiatorHost = null;
        Rule best = null;
        for (Rule r : ext.others)
            if (r.better(best) && matches(r, url, host, initiatorHost, type, method))
                best = r;
        for (String parent = host; !parent.isEmpty(); ) {
            List<Rule> list = ext.byDomain.get(parent);
            if (list != null)
                for (Rule r : list)
                    if (r.better(best) && matches(r, url, host, initiatorHost, type, method))
                        best = r;
            int dot = parent.indexOf('.');
            parent = dot < 0 ? "" : parent.substring(dot + 1);
        }
        // A frame an allowAllRequests rule took in lets all its requests through, below that
        // rule's priority: asked of the frame's own address.
        if (!type.equals("main_frame") && initiator != null && initiator.contains("://")) {
            String pageHost = hostOf(initiator);
            for (Rule r : ext.allowAll)
                if (matches(r, initiator, pageHost, null, "main_frame", "get") && (best == null || r.priority >= best.priority))
                    return r;
        }
        return best;
    }

    /** http:// as https://, ws:// as wss://; null for any other. */
    static String upgraded(String url) {
        if (url.regionMatches(true, 0, "http:", 0, 5))
            return "https:" + url.substring(5);
        if (url.regionMatches(true, 0, "ws:", 0, 3))
            return "wss:" + url.substring(3);
        return null;
    }

    /** Chrome's \1 ... \9 and \0 in a regexSubstitution, as the matched groups. */
    static String substituted(Rule r, String url) {
        if (r.url == null)
            return null;
        Matcher m = r.url.matcher(url);
        if (!m.find())
            return null;
        StringBuilder out = new StringBuilder();
        String sub = r.sub;
        for (int i = 0; i < sub.length(); i++) {
            char c = sub.charAt(i);
            if (c == '\\' && i + 1 < sub.length() && Character.isDigit(sub.charAt(i + 1))) {
                int g = sub.charAt(++i) - '0';
                if (g <= m.groupCount() && m.group(g) != null)
                    out.append(m.group(g));
                continue;
            }
            out.append(c);
        }
        return out.toString();
    }

    /** What one extension's rules make of a request, if anything. */
    static Decision decide(Ext ext, String url, String initiator, String type, String method) {
        Rule r = winner(ext, url, initiator, type, method);
        if (r == null)
            return null;
        String to;
        switch (r.action) {
            case "block": return new Decision(true, null);
            case "upgradeScheme": to = upgraded(url); break;
            case "redirect": to = r.to != null ? r.to : r.sub != null ? substituted(r, url) : null; break;
            default: return null;
        }
        return to == null || to.equals(url) ? null : new Decision(false, to);
    }

    /** What all the profile's extensions make of a request: `type` as declarativeNetRequest
     *  names it, `method` in lower case, `initiator` the page that makes it (null for the
     *  browser's own). One that blocks wins; then the first that redirects or upgrades. */
    Decision decide(String url, String initiator, String type, String method) {
        Decision first = null;
        for (Ext ext : extensions) {
            Decision d = decide(ext, url, initiator, type, method);
            if (d == null)
                continue;
            if (d.block)
                return d;
            if (first == null)
                first = d;
        }
        return first;
    }

    /** What a request is for, as well as the WebView lets one tell: it says only whether it is
     *  the top frame's, its address and its headers. A frame's document asks for HTML, a style
     *  sheet for CSS, an image for images; the rest by the address's ending, and what is left is
     *  taken as a script's request (fetch or XMLHttpRequest). */
    static String typeOf(boolean mainFrame, String url, Map<String, String> headers) {
        if (mainFrame)
            return "main_frame";
        String accept = "";
        if (headers != null)
            for (Map.Entry<String, String> h : headers.entrySet())
                if (h.getKey() != null && h.getKey().equalsIgnoreCase("Accept") && h.getValue() != null)
                    accept = h.getValue().toLowerCase(Locale.ROOT);
        if (accept.startsWith("text/html"))
            return "sub_frame";
        if (accept.startsWith("text/css"))
            return "stylesheet";
        if (accept.startsWith("image/"))
            return "image";
        String path = url;
        int end = path.length();
        for (char c : new char[] {'?', '#'}) {
            int at = path.indexOf(c);
            if (at >= 0 && at < end)
                end = at;
        }
        path = path.substring(0, end).toLowerCase(Locale.ROOT);
        int dot = path.lastIndexOf('.');
        int slash = path.lastIndexOf('/');
        String ending = dot > slash ? path.substring(dot + 1) : "";
        switch (ending) {
            case "js": case "mjs": return "script";
            case "css": return "stylesheet";
            case "png": case "jpg": case "jpeg": case "gif": case "webp": case "svg": case "ico": case "avif": case "bmp": return "image";
            case "woff": case "woff2": case "ttf": case "otf": case "eot": return "font";
            case "mp4": case "webm": case "m4a": case "mp3": case "ogg": case "oga": case "wav": case "m3u8": case "mpd": case "ts": case "m4s": return "media";
            default: return "xmlhttprequest";
        }
    }
}
