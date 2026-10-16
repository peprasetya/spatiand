package id.prasetya.spatiand.mesh;

import android.util.Log;
import android.webkit.WebView;

import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;

/**
 * The WebView's own support-library interfaces -- what androidx.webkit calls -- reached by
 * reflection, as androidx.webkit reaches them (the WebView package's
 * org.chromium.support_lib_glue.SupportLibReflectionUtil, and its
 * org.chromium.support_lib_boundary interfaces, called through the InvocationHandlers they hand
 * out). Only what SpatiMesh's extensions need: scripts at the start of every frame's every page,
 * and a message listener in every frame that answers each frame on its own.
 *
 * Written on Linux against Android 14's classes; not yet run on a device.
 */
final class Boundary {
    private static final String TAG = "spatimesh";
    private static final String GLUE = "org.chromium.support_lib_glue.SupportLibReflectionUtil";
    private static final String BOUNDARY = "org.chromium.support_lib_boundary.";

    private final InvocationHandler factory;
    private final Method createWebView;
    private final Method addDocumentStart;
    private final Method addListener;
    private final Method removeScript;
    private final Method getData;
    private final Method getPayload;
    private final Method payloadText;
    private final Method reply;

    private Boundary(ClassLoader loader, InvocationHandler factory) throws ReflectiveOperationException {
        this.factory = factory;
        createWebView = boundary(loader, "WebViewProviderFactoryBoundaryInterface").getMethod("createWebView", WebView.class);
        Class<?> provider = boundary(loader, "WebViewProviderBoundaryInterface");
        addDocumentStart = provider.getMethod("addDocumentStartJavaScript", String.class, String[].class);
        addListener = provider.getMethod("addWebMessageListener", String.class, String[].class, InvocationHandler.class);
        removeScript = boundary(loader, "ScriptHandlerBoundaryInterface").getMethod("remove");
        Class<?> message = boundary(loader, "WebMessageBoundaryInterface");
        getData = optional(message, "getData");
        getPayload = optional(message, "getMessagePayload");
        Class<?> payload = optionalClass(loader, "WebMessagePayloadBoundaryInterface");
        payloadText = payload == null ? null : optional(payload, "getAsString");
        reply = boundary(loader, "JsReplyProxyBoundaryInterface").getMethod("postMessage", String.class);
    }

    private static Class<?> boundary(ClassLoader loader, String name) throws ClassNotFoundException {
        return Class.forName(BOUNDARY + name, false, loader);
    }

    private static Class<?> optionalClass(ClassLoader loader, String name) {
        try {
            return boundary(loader, name);
        } catch (ClassNotFoundException e) {
            return null;
        }
    }

    private static Method optional(Class<?> c, String name) {
        try {
            return c.getMethod(name);
        } catch (NoSuchMethodException e) {
            return null;
        }
    }

    /** This WebView's, or null where it has not got both features (logged). */
    static Boundary find() {
        try {
            ClassLoader loader = WebView.getWebViewClassLoader();
            Class<?> glue = Class.forName(GLUE, false, loader);
            InvocationHandler factory = (InvocationHandler) glue.getDeclaredMethod("createWebViewProviderFactory").invoke(null);
            Method features = boundary(loader, "WebViewProviderFactoryBoundaryInterface").getMethod("getSupportedFeatures");
            String[] has = (String[]) factory.invoke(null, features, null);
            boolean start = false, listener = false;
            for (String f : has) {
                // "DOCUMENT_START_SCRIPT:1" in the WebViews that have it as androidx.webkit asks.
                start |= f.startsWith("DOCUMENT_START_SCRIPT");
                listener |= f.equals("WEB_MESSAGE_LISTENER");
            }
            if (!start || !listener) {
                Log.w(TAG, "this WebView has no document-start scripts or message listeners: extensions do not run");
                return null;
            }
            return new Boundary(loader, factory);
        } catch (Throwable e) {
            Log.w(TAG, "this WebView's support-library interfaces are not there: extensions do not run", e);
            return null;
        }
    }

    /** What the interfaces call `view`. */
    InvocationHandler provider(WebView view) throws Throwable {
        return (InvocationHandler) factory.invoke(null, createWebView, new Object[] {view});
    }

    /** `script` at the start of every frame's every page from now on; its handle, to take it off. */
    Object addDocumentStart(InvocationHandler provider, String script) throws Throwable {
        return provider.invoke(null, addDocumentStart, new Object[] {script, new String[] {"*"}});
    }

    void remove(Object script) {
        try {
            ((InvocationHandler) script).invoke(null, removeScript, null);
        } catch (Throwable e) {
            Log.w(TAG, "a document-start script could not be taken off", e);
        }
    }

    /** `window[name]` in every frame, whose postMessage calls `listener`'s onPostMessage(view,
     *  message, origin, isMainFrame, reply). */
    void addListener(InvocationHandler provider, String name, InvocationHandler listener) throws Throwable {
        provider.invoke(null, addListener, new Object[] {name, new String[] {"*"}, listener});
    }

    /** A posted message's text. */
    String text(Object message) {
        try {
            InvocationHandler m = (InvocationHandler) message;
            if (getData != null) {
                Object data = m.invoke(null, getData, null);
                if (data instanceof String)
                    return (String) data;
            }
            if (getPayload != null && payloadText != null) {
                Object payload = m.invoke(null, getPayload, null);
                if (payload != null)
                    return (String) ((InvocationHandler) payload).invoke(null, payloadText, null);
            }
        } catch (Throwable e) {
            Log.w(TAG, "a message could not be read", e);
        }
        return null;
    }

    /** `text` to the frame a reply proxy is for (its listener's onmessage). */
    void reply(Object proxy, String text) {
        try {
            ((InvocationHandler) proxy).invoke(null, reply, new Object[] {text});
        } catch (Throwable e) {
            Log.w(TAG, "a frame could not be answered", e);
        }
    }

    /** A listener for addListener: `said` gets each message's text, whether its frame is the
     *  top one, and that frame's reply proxy. */
    interface Said {
        void said(String text, boolean top, Object reply);
    }

    InvocationHandler listener(Said said) {
        return (proxy, method, args) -> {
            switch (method.getName()) {
                case "onPostMessage":
                    said.said(text(args[1]), Boolean.TRUE.equals(args[3]), args[4]);
                    return null;
                case "getSupportedFeatures":
                    return new String[0];
                case "hashCode":
                    return System.identityHashCode(proxy);
                case "equals":
                    return proxy == args[0];
                case "toString":
                    return "SpatiMesh's extensions' listener";
                default:
                    return null;
            }
        };
    }
}
