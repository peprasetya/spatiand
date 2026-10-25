/* WPE WebKit behind the few calls SpatiMesh's engine host makes: the C interface every engine's
 * native half provides (crates/spatimesh-shim). The same on every platform WPE is built for --
 * the Mac (third_party/wpe, ported), Linux, Android.
 *
 * WPE draws through a platform of the embedder's: here SpatiMesh's own (a display, a view and a
 * toplevel, below), shown on no screen. Each tab is a WebKitWebView on it; every picture WPE
 * finishes is handed to its view, which keeps it for the window to read -- only where it
 * changed (WPE's damage) -- and gives the buffer back. The wearer's input is given to the view
 * as WPE events made here. Whatever WebKit would otherwise answer with a window of its own -- a
 * <select>'s list, a script's alert, a file chooser, a context menu -- is answered here, or
 * handed to SpatiMesh's window to draw.
 *
 * Pictures stay on the GPU where they can: on a Mac each is an IOSurface the web process drew
 * into, which the window shows as it is (spatimesh_surface), the same memory, nothing copied.
 * Where WPE hands over shared memory, the picture is copied (spatimesh_copy).
 *
 * Most of this is the WebKitGTK engine's (crates/spatimesh-webkitgtk), whose API WPE's is but
 * for windows and input. What happens to pages is queued as events, popped by the Rust side
 * after each pump. */

#include <wpe/webkit.h>
#include <glib-unix.h>
#include <libsoup/soup.h>
#include <epoxy/egl.h>
#include <math.h>
#include <stdint.h>
#include <string.h>
#include <fontconfig/fontconfig.h>
#ifdef __APPLE__
#include <CoreFoundation/CoreFoundation.h>
#include <CoreText/CoreText.h>
#include <limits.h>
#endif

enum {
    EV_TITLE = 1,
    EV_URL,
    EV_PROGRESS,
    EV_HISTORY,
    EV_CURSOR,
    EV_SOUND,
    EV_OPEN_TAB,
    EV_CLOSE_TAB,
    EV_CRASHED,
    EV_FAILED,
    EV_CHOOSE,
    EV_DIALOG,
    EV_EXTENSION,
    EV_SCRIPT_RESULT,
    EV_MEDIA,
    EV_COOKIES,
    EV_SOUND_NAME,
};

typedef struct {
    int kind;
    uint64_t tab;
    double number;
    int a, b;
    char *text;
    char *text2;
    int x, y, w, h;
} MeshEvent;

struct Engine;

/* ---- SpatiMesh's WPE platform -------------------------------------------------------------- */

/* The view keeps the newest picture WPE finished for it, and where it changed since it was
 * last read; the tab reads it from there. */
typedef struct {
    WPEView parent;
    GBytes *pixels;
    int width, height, stride;
    int damaged, dx0, dy0, dx1, dy1;
    /* Pictures on the GPU (IOSurfaces, on a Mac): the newest not yet offered to the window, and
     * every one the window holds -- given back to WPE only when it lets go of it, so that WPE
     * never draws into what is being shown. */
    WPEBuffer *fresh_surface;
    GPtrArray *held_surfaces;
    void (*on_picture)(gpointer);
    gpointer data;
    /* The size its window is to have, where WebKit makes the window (the developer tools'),
     * and what to do when WebKit takes the window away (they were closed). */
    int tab_width, tab_height;
    void (*on_gone)(gpointer);
    /* The window being watched for that: WebKit moves the view out of it and back while it
     * sets the developer tools up, and lets the window go when they close. */
    WPEToplevel *watched;
} MeshView;
typedef struct { WPEViewClass parent; } MeshViewClass;
G_DEFINE_TYPE(MeshView, mesh_view, WPE_TYPE_VIEW)

static void add_damage(MeshView *v, int x, int y, int w, int h) {
    if (!v->damaged) {
        v->dx0 = x; v->dy0 = y; v->dx1 = x + w; v->dy1 = y + h;
    } else {
        v->dx0 = MIN(v->dx0, x); v->dy0 = MIN(v->dy0, y);
        v->dx1 = MAX(v->dx1, x + w); v->dy1 = MAX(v->dy1, y + h);
    }
    v->damaged = 1;
}

static gboolean mesh_view_render_buffer(WPEView *view, WPEBuffer *buffer, const WPERectangle *damage, guint n_damage, GError **error) {
    MeshView *v = (MeshView *)view;
    int width = wpe_buffer_get_width(buffer), height = wpe_buffer_get_height(buffer);
#ifdef WPE_PLATFORM_BUFFER_IOSURFACE
    if (WPE_IS_BUFFER_IOSURFACE(buffer)) {
        /* Shown as it is. One the window never took is WPE's again at once. */
        if (v->fresh_surface) {
            wpe_view_buffer_released(view, v->fresh_surface);
            g_object_unref(v->fresh_surface);
        }
        v->fresh_surface = g_object_ref(buffer);
        wpe_view_buffer_rendered(view, buffer);
        if (v->on_picture)
            v->on_picture(v->data);
        return TRUE;
    }
#endif
    GBytes *pixels = NULL;
    int stride = width * 4;
    if (WPE_IS_BUFFER_SHM(buffer)) {
        pixels = g_bytes_ref(wpe_buffer_shm_get_data(WPE_BUFFER_SHM(buffer)));
        stride = (int)wpe_buffer_shm_get_stride(WPE_BUFFER_SHM(buffer));
    } else {
        pixels = wpe_buffer_import_to_pixels(buffer, error);
        if (!pixels)
            return FALSE;
        pixels = g_bytes_ref(pixels);
    }
    /* A picture of another size is all new; otherwise only what WPE says changed. */
    gboolean whole = width != v->width || height != v->height || !n_damage;
    if (v->pixels)
        g_bytes_unref(v->pixels);
    v->pixels = pixels;
    v->width = width;
    v->height = height;
    v->stride = stride;
    if (whole)
        add_damage(v, 0, 0, width, height);
    else
        for (guint i = 0; i < n_damage; i++)
            add_damage(v, damage[i].x, damage[i].y, damage[i].width, damage[i].height);
    /* Shown at once, as far as WPE is to know: it may draw the next. The buffer is ours to give
     * back now, its pixels being kept (shared memory, which WPE does not write again until
     * then). */
    wpe_view_buffer_rendered(view, buffer);
    wpe_view_buffer_released(view, buffer);
    if (v->on_picture)
        v->on_picture(v->data);
    return TRUE;
}

static void toplevel_gone(gpointer data, GObject *was) {
    MeshView *v = data;
    v->watched = NULL;
    if (v->on_gone)
        v->on_gone(v->data);
}

/* Watch the window the view is in, once it is to be told of its going (the developer tools). */
static void watch_toplevel(MeshView *v) {
    WPEToplevel *toplevel = wpe_view_get_toplevel(WPE_VIEW(v));
    if (!v->on_gone || !toplevel || v->watched)
        return;
    v->watched = toplevel;
    g_object_weak_ref(G_OBJECT(toplevel), toplevel_gone, v);
}

static void unwatch_toplevel(MeshView *v) {
    if (v->watched)
        g_object_weak_unref(G_OBJECT(v->watched), toplevel_gone, v);
    v->watched = NULL;
}

static void on_toplevel(WPEView *view, GParamSpec *p, gpointer data) {
    WPEToplevel *toplevel = wpe_view_get_toplevel(view);
    MeshView *mesh = (MeshView *)view;
    if (!toplevel) {
        wpe_view_unmap(view);
        return;
    }
    watch_toplevel(mesh);
    /* The developer tools' view comes in a window WebKit sized; it is the size of its tab. */
    if (mesh->tab_width && mesh->tab_height)
        wpe_toplevel_resize(toplevel, mesh->tab_width, mesh->tab_height);
    int width, height;
    wpe_toplevel_get_size(toplevel, &width, &height);
    if (width && height)
        wpe_view_resized(view, width, height);
    wpe_view_map(view);
}

static void mesh_view_constructed(GObject *object) {
    G_OBJECT_CLASS(mesh_view_parent_class)->constructed(object);
    g_signal_connect(object, "notify::toplevel", G_CALLBACK(on_toplevel), NULL);
}

static void mesh_view_dispose(GObject *object) {
    MeshView *v = (MeshView *)object;
    unwatch_toplevel(v);
    g_clear_pointer(&v->pixels, g_bytes_unref);
    g_clear_object(&v->fresh_surface);
    g_clear_pointer(&v->held_surfaces, g_ptr_array_unref);
    G_OBJECT_CLASS(mesh_view_parent_class)->dispose(object);
}

static void mesh_view_class_init(MeshViewClass *klass) {
    G_OBJECT_CLASS(klass)->constructed = mesh_view_constructed;
    G_OBJECT_CLASS(klass)->dispose = mesh_view_dispose;
    WPE_VIEW_CLASS(klass)->render_buffer = mesh_view_render_buffer;
}

static void mesh_view_init(MeshView *v) {
    v->held_surfaces = g_ptr_array_new_with_free_func(g_object_unref);
}

/* A toplevel of one view, as large as the tab. */
typedef struct { WPEToplevel parent; } MeshToplevel;
typedef struct { WPEToplevelClass parent; } MeshToplevelClass;
G_DEFINE_TYPE(MeshToplevel, mesh_toplevel, WPE_TYPE_TOPLEVEL)

static gboolean resize_view(WPEToplevel *toplevel, WPEView *view, gpointer data) {
    int width, height;
    wpe_toplevel_get_size(toplevel, &width, &height);
    wpe_view_resized(view, width, height);
    return FALSE;
}

static gboolean mesh_toplevel_resize(WPEToplevel *toplevel, int width, int height) {
    wpe_toplevel_resized(toplevel, width, height);
    wpe_toplevel_foreach_view(toplevel, resize_view, NULL);
    return TRUE;
}

static void mesh_toplevel_constructed(GObject *object) {
    G_OBJECT_CLASS(mesh_toplevel_parent_class)->constructed(object);
    wpe_toplevel_state_changed(WPE_TOPLEVEL(object), WPE_TOPLEVEL_STATE_ACTIVE);
}

static void mesh_toplevel_class_init(MeshToplevelClass *klass) {
    G_OBJECT_CLASS(klass)->constructed = mesh_toplevel_constructed;
    WPE_TOPLEVEL_CLASS(klass)->resize = mesh_toplevel_resize;
}

static void mesh_toplevel_init(MeshToplevel *t) {
    (void)t;
}

/* The display: views and toplevels of the kinds above, and EGL's own default display -- on a
 * Mac ANGLE's (on Metal), on Linux the system's. */
typedef struct { WPEDisplay parent; } MeshDisplay;
typedef struct { WPEDisplayClass parent; } MeshDisplayClass;
G_DEFINE_TYPE(MeshDisplay, mesh_display, WPE_TYPE_DISPLAY)

static gboolean mesh_display_connect(WPEDisplay *display, GError **error) {
    return TRUE;
}

static void bind_inspector_view(MeshView *v);

static WPEView *mesh_display_create_view(WPEDisplay *display) {
    MeshView *v = (MeshView *)g_object_new(mesh_view_get_type(), "display", display, NULL);
    bind_inspector_view(v);
    return WPE_VIEW(v);
}

static WPEToplevel *mesh_display_create_toplevel(WPEDisplay *display, guint max_views) {
    return WPE_TOPLEVEL(g_object_new(mesh_toplevel_get_type(), "display", display, NULL));
}

static gpointer mesh_display_get_egl_display(WPEDisplay *display, GError **error) {
#ifdef __APPLE__
    /* ANGLE on Metal, asked for by name, with the attributes WebKit asks with (its
     * PlatformDisplayDefault), so that it is the same display. */
    const EGLint metal[] = { 0x3203 /* EGL_PLATFORM_ANGLE_TYPE_ANGLE */, 0x3489 /* ..._METAL_ANGLE */, EGL_NONE };
    EGLDisplay egl = eglGetPlatformDisplayEXT(0x3202 /* EGL_PLATFORM_ANGLE_ANGLE */, NULL /* the default */, metal);
#else
    EGLDisplay egl = eglGetDisplay(EGL_DEFAULT_DISPLAY);
#endif
    if (egl == EGL_NO_DISPLAY) {
        g_set_error_literal(error, WPE_EGL_ERROR, WPE_EGL_ERROR_NOT_AVAILABLE, "no EGL display");
        return NULL;
    }
    return egl;
}

static void mesh_display_class_init(MeshDisplayClass *klass) {
    WPEDisplayClass *display = WPE_DISPLAY_CLASS(klass);
    display->connect = mesh_display_connect;
    display->create_view = mesh_display_create_view;
    display->create_toplevel = mesh_display_create_toplevel;
    display->get_egl_display = mesh_display_get_egl_display;
}

static void mesh_display_init(MeshDisplay *d) {
    (void)d;
}

/* ---- the platform's look: its interface font, light or dark -------------------------------- */

#ifdef __APPLE__
/* The Mac's interface font, by the name fontconfig knows its file by (CoreText's own name for it,
 * .AppleSystemUIFont, is hidden from fontconfig), at its size: "System Font 13px". */
static char *platform_font_name(void) {
    char *name = NULL;
    CTFontRef font = CTFontCreateUIFontForLanguage(kCTFontUIFontSystem, 0, NULL);
    if (!font)
        return g_strdup("sans-serif 13px");
    double size = CTFontGetSize(font);
    CFURLRef url = CTFontCopyAttribute(font, kCTFontURLAttribute);
    char path[PATH_MAX];
    if (url && CFURLGetFileSystemRepresentation(url, true, (UInt8 *)path, sizeof path)) {
        FcPattern *query = FcPatternCreate();
        FcPatternAddString(query, FC_FILE, (const FcChar8 *)path);
        FcObjectSet *wanted = FcObjectSetBuild(FC_FAMILY, NULL);
        FcFontSet *found = FcFontList(NULL, query, wanted);
        FcChar8 *family = NULL;
        if (found && found->nfont && FcPatternGetString(found->fonts[0], FC_FAMILY, 0, &family) == FcResultMatch)
            name = g_strdup_printf("%s %dpx", (const char *)family, (int)(size + 0.5));
        if (found)
            FcFontSetDestroy(found);
        FcObjectSetDestroy(wanted);
        FcPatternDestroy(query);
    }
    if (url)
        CFRelease(url);
    CFRelease(font);
    return name ? name : g_strdup_printf("sans-serif %dpx", (int)(size + 0.5));
}

/* Whether the Mac is in Dark Mode (System Settings, Appearance). */
static gboolean platform_dark(void) {
    CFPreferencesAppSynchronize(kCFPreferencesAnyApplication);
    CFStringRef style = CFPreferencesCopyAppValue(CFSTR("AppleInterfaceStyle"), kCFPreferencesAnyApplication);
    gboolean dark = style && CFStringCompare(style, CFSTR("Dark"), kCFCompareCaseInsensitive) == kCFCompareEqualTo;
    if (style)
        CFRelease(style);
    return dark;
}
#else
/* A Linux desktop's interface font and colour scheme, from GNOME's settings where they are. */
static GSettings *desktop_settings(void) {
    static GSettings *settings;
    static gboolean looked;
    if (!looked) {
        looked = TRUE;
        GSettingsSchemaSource *source = g_settings_schema_source_get_default();
        GSettingsSchema *schema = source ? g_settings_schema_source_lookup(source, "org.gnome.desktop.interface", TRUE) : NULL;
        if (schema) {
            settings = g_settings_new("org.gnome.desktop.interface");
            g_settings_schema_unref(schema);
        }
    }
    return settings;
}

static char *platform_font_name(void) {
    GSettings *settings = desktop_settings();
    char *name = settings ? g_settings_get_string(settings, "font-name") : NULL;
    if (name && *name)
        return name;
    g_free(name);
    return g_strdup("Sans 10");
}

static gboolean platform_dark(void) {
    GSettings *settings = desktop_settings();
    if (!settings)
        return FALSE;
    char *scheme = g_settings_get_string(settings, "color-scheme");
    gboolean dark = scheme && strcmp(scheme, "prefer-dark") == 0;
    g_free(scheme);
    return dark;
}
#endif

/* Pages are told light or dark as the platform is, and again whenever it changes. */
static gboolean check_dark_mode(gpointer data) {
    WPEDisplay *display = data;
    gboolean dark = platform_dark();
    GError *error = NULL;
    if (wpe_settings_get_boolean(wpe_display_get_settings(display), WPE_SETTING_DARK_MODE, NULL) != dark)
        wpe_settings_set_boolean(wpe_display_get_settings(display), WPE_SETTING_DARK_MODE, dark, WPE_SETTINGS_SOURCE_PLATFORM, &error);
    g_clear_error(&error);
    return G_SOURCE_CONTINUE;
}

static void follow_dark_mode(WPEDisplay *display) {
    check_dark_mode(display);
    g_timeout_add_seconds(1, check_dark_mode, display);
}

static WPEDisplay *mesh_display(void) {
    static WPEDisplay *display;
    if (!display) {
        display = WPE_DISPLAY(g_object_new(mesh_display_get_type(), NULL));
        GError *error = NULL;
        if (!wpe_display_connect(display, &error)) {
            g_warning("SpatiMesh's WPE display: %s", error ? error->message : "?");
            g_clear_error(&error);
        }
        /* The platform's own interface font and light or dark, as its desktop says them: form
         * controls and the developer tools are drawn with the font (unsaid, they take the page's
         * standard font, a serif), and pages are told the colour scheme (prefers-color-scheme). */
        char *font = platform_font_name();
        wpe_settings_set_string(wpe_display_get_settings(display), WPE_SETTING_FONT_NAME, font, WPE_SETTINGS_SOURCE_PLATFORM, NULL);
        g_free(font);
        follow_dark_mode(display);
    }
    return display;
}

typedef struct {
    uint64_t id;
    struct Engine *engine;
    WebKitWebView *view;
    MeshView *wpe;
    int width, height;
    /* Changed since the last pump: a reason for the pump not to wait. */
    int fresh;
    WebKitOptionMenu *menu;
    WebKitScriptDialog *dialog;
    int entered;
    WPEModifiers buttons;
    int cursor;
    int loading;
    double x, y;
    guint presses;
    guint32 last_press;
    /* The extensions' worlds a message handler is registered in, for this tab's view. */
    GPtrArray *worlds;
    int main_handler;
    /* The media script's way out is registered once a tab has one. */
    int media_handler;
    /* Developer tools: a page's (`inspector`, the tab showing them), or, for that tab, the page
     * they are for (`inspected`). Such a tab has no web view of its own: WebKit makes its view
     * on SpatiMesh's display, and it is `wpe` once made. */
    uint64_t inspector, inspected;
} Tab;

/* One extension, as the window gave it (crates/spatimesh-shim): its scheme, its content
 * scripts' world, and the code that sets its API up in a content script's world and in its
 * own pages. */
typedef struct {
    char *id;
    char *scheme;
    char *world;
    char *content_boot;
    char *page_boot;
} Ext;

/* One content-script entry of an extension's: when, where, for which pages (WebKit's own
 * patterns: allow NULL for every page), and its files' code. `guard` is JavaScript that is
 * true where the entry is wanted, for what the patterns cannot say exactly (`exact` 0). */
typedef struct {
    Ext *ext;
    int run_at;
    int all_frames;
    int main_world;
    char **allow;
    char **block;
    char *guard;
    int exact;
    GPtrArray *js;
    GPtrArray *css;
} Entry;

typedef struct Engine {
    WebKitWebContext *context;
    WebKitNetworkSession *session;
    WebKitSettings *settings;
    GHashTable *tabs;
    /* Views WebKit made for window.open, each waiting for the tab SpatiMesh opens for it (its
     * address is OPENED_PREFIX and the number here). */
    GHashTable *opened;
    unsigned opened_next;
    GQueue *events;
    MeshEvent current;
    int woken, timed_out;
    guint watch;
    /* The profile's extensions, and their content scripts. */
    GPtrArray *exts;
    GPtrArray *entries;
    /* Their declarativeNetRequest rules as WebKit's content blockers: compiled into the store,
     * by extension id (the JSON given, until compiled; the filter, once). */
    WebKitUserContentFilterStore *filter_store;
    GHashTable *rule_sources;
    /* SpatiMesh's own script for every frame's every document (its media). */
    char *page_script;
    GHashTable *filters;
    WebKitCookieManager *cookies;
} Engine;

static guint32 now_ms(void) {
    return (guint32)(g_get_monotonic_time() / 1000);
}

static void push(Engine *e, int kind, uint64_t tab, double number, int a, int b, const char *text, const char *text2) {
    MeshEvent *ev = g_new0(MeshEvent, 1);
    ev->kind = kind;
    ev->tab = tab;
    ev->number = number;
    ev->a = a;
    ev->b = b;
    ev->text = text ? g_strdup(text) : NULL;
    ev->text2 = text2 ? g_strdup(text2) : NULL;
    g_queue_push_tail(e->events, ev);
}

static Tab *find(Engine *e, uint64_t id) {
    return g_hash_table_lookup(e->tabs, &id);
}

/* A tab with a page of its own (not the developer tools'). */
static Tab *find_page(Engine *e, uint64_t id) {
    Tab *t = find(e, id);
    return t && t->view ? t : NULL;
}

/* A tab with a view to show and to give input to (the developer tools' has none until WebKit
 * has made it). */
static Tab *find_shown(Engine *e, uint64_t id) {
    Tab *t = find(e, id);
    return t && t->wpe ? t : NULL;
}


/* ---- what pages do ---------------------------------------------------------------------- */

static void history(Tab *t) {
    push(t->engine, EV_HISTORY, t->id, 0, webkit_web_view_can_go_back(t->view), webkit_web_view_can_go_forward(t->view), NULL, NULL);
}

static void on_title(WebKitWebView *view, GParamSpec *p, Tab *t) {
    push(t->engine, EV_TITLE, t->id, 0, 0, 0, webkit_web_view_get_title(view), NULL);
}

static void on_uri(WebKitWebView *view, GParamSpec *p, Tab *t) {
    const char *uri = webkit_web_view_get_uri(view);
    push(t->engine, EV_URL, t->id, 0, 0, 0, uri ? uri : "", NULL);
    history(t);
}

static void on_progress(WebKitWebView *view, GParamSpec *p, Tab *t) {
    t->loading = webkit_web_view_is_loading(view);
    push(t->engine, EV_PROGRESS, t->id, webkit_web_view_get_estimated_load_progress(view), t->loading, 0, NULL, NULL);
}

static void on_load_changed(WebKitWebView *view, WebKitLoadEvent event, Tab *t) {
    static const char *const names[] = { "started", "redirected", "committed", "finished" };
    fprintf(stderr, "spatimesh-wpe: tab %llu %s %s\n", (unsigned long long)t->id, event < 4 ? names[event] : "?", webkit_web_view_get_uri(view) ? webkit_web_view_get_uri(view) : "");
    on_progress(view, NULL, t);
    if (event == WEBKIT_LOAD_COMMITTED || event == WEBKIT_LOAD_FINISHED)
        history(t);
}

static void on_audio(WebKitWebView *view, GParamSpec *p, Tab *t) {
    push(t->engine, EV_SOUND, t->id, 0, webkit_web_view_is_playing_audio(view), 0, NULL, NULL);
}

static void on_target(WebKitWebView *view, WebKitHitTestResult *hit, guint modifiers, Tab *t) {
    /* Cursors by what is under the pointer: what WPE would set on a view nobody sees. */
    int cursor = 0;
    if (webkit_hit_test_result_context_is_link(hit))
        cursor = 1;
    else if (webkit_hit_test_result_context_is_editable(hit))
        cursor = 2;
    if (cursor != t->cursor) {
        t->cursor = cursor;
        push(t->engine, EV_CURSOR, t->id, 0, cursor, 0, NULL, NULL);
    }
}

static gboolean on_policy(WebKitWebView *view, WebKitPolicyDecision *decision, WebKitPolicyDecisionType type, Tab *t) {
    if (type == WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION || type == WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION) {
        WebKitNavigationAction *action = webkit_navigation_policy_decision_get_navigation_action(WEBKIT_NAVIGATION_POLICY_DECISION(decision));
        guint button = webkit_navigation_action_get_mouse_button(action);
        guint modifiers = webkit_navigation_action_get_modifiers(action);
        gboolean link = webkit_navigation_action_get_navigation_type(action) == WEBKIT_NAVIGATION_TYPE_LINK_CLICKED;
        gboolean background = button == 2 || (modifiers & (WPE_MODIFIER_KEYBOARD_CONTROL | WPE_MODIFIER_KEYBOARD_META));
        if (type == WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION || (link && background)) {
            const char *uri = webkit_uri_request_get_uri(webkit_navigation_action_get_request(action));
            push(t->engine, EV_OPEN_TAB, t->id, 0, background, 0, uri ? uri : "", NULL);
            webkit_policy_decision_ignore(decision);
            return TRUE;
        }
        return FALSE;
    }
    if (type == WEBKIT_POLICY_DECISION_TYPE_RESPONSE) {
        WebKitResponsePolicyDecision *response = WEBKIT_RESPONSE_POLICY_DECISION(decision);
        if (!webkit_response_policy_decision_is_mime_type_supported(response)) {
            webkit_policy_decision_download(decision);
            return TRUE;
        }
    }
    return FALSE;
}

#define OPENED_PREFIX "spatimesh-opened:"

/* window.open: WebKit is given a view related to the opener's -- so the page opened has
 * window.opener, can post messages back and close itself, as a sign-in window does -- and
 * SpatiMesh opens a tab of its own for it, which takes this view (spatimesh_open). */
static WebKitWebView *on_create(WebKitWebView *view, WebKitNavigationAction *action, Tab *t) {
    Engine *e = t->engine;
    WebKitWebView *opened = WEBKIT_WEB_VIEW(g_object_new(WEBKIT_TYPE_WEB_VIEW, "display", mesh_display(),
        "related-view", view, "settings", e->settings, NULL));
    unsigned number = ++e->opened_next;
    g_hash_table_insert(e->opened, GUINT_TO_POINTER(number), g_object_ref(opened));
    char *address = g_strdup_printf(OPENED_PREFIX "%u", number);
    push(e, EV_OPEN_TAB, t->id, 0, 0, 0, address, NULL);
    g_free(address);
    return opened;
}

/* The name this tab's sound reaches the room under: the sink in its web process says
 * <application id>/page<page id> (third_party/wpe/gst/spatiandaudiosink.c), the page id being
 * WebKit's for the view, which a process swap keeps. */
static void tell_sound_name(WebKitWebView *view, GParamSpec *spec, Tab *t) {
    const char *app = g_getenv("SPATIAND_APP_ID");
    guint64 page = webkit_web_view_get_page_id(view);
    if (!app || !page)
        return;
    char *name = g_strdup_printf("%s/page%" G_GUINT64_FORMAT, app, page);
    push(t->engine, EV_SOUND_NAME, t->id, 0, 0, 0, name, NULL);
    g_free(name);
}

static void on_close(WebKitWebView *view, Tab *t) {
    push(t->engine, EV_CLOSE_TAB, t->id, 0, 0, 0, NULL, NULL);
}

static void on_terminated(WebKitWebView *view, WebKitWebProcessTerminationReason reason, Tab *t) {
    const char *why = reason == WEBKIT_WEB_PROCESS_CRASHED ? "The page's process crashed"
        : reason == WEBKIT_WEB_PROCESS_EXCEEDED_MEMORY_LIMIT ? "The page used too much memory"
        : "The page's process was ended";
    fprintf(stderr, "spatimesh-wpe: tab %llu: %s\n", (unsigned long long)t->id, why);
    push(t->engine, EV_CRASHED, t->id, 0, 0, 0, why, NULL);
}

static gboolean on_failed(WebKitWebView *view, WebKitLoadEvent event, const char *uri, GError *error, Tab *t) {
    fprintf(stderr, "spatimesh-wpe: tab %llu could not load %s: %s\n", (unsigned long long)t->id, uri ? uri : "", error ? error->message : "?");
    /* Stopped, or turned into a download: not failures anyone needs telling about. */
    if (g_error_matches(error, WEBKIT_NETWORK_ERROR, WEBKIT_NETWORK_ERROR_CANCELLED)
        || g_error_matches(error, WEBKIT_POLICY_ERROR, WEBKIT_POLICY_ERROR_FRAME_LOAD_INTERRUPTED_BY_POLICY_CHANGE))
        return FALSE;
    push(t->engine, EV_FAILED, t->id, 0, 0, 0, uri ? uri : "", error ? error->message : "it could not be loaded");
    return FALSE; /* WebKit's own error page is drawn. */
}

static gboolean on_tls(WebKitWebView *view, const char *uri, GTlsCertificate *certificate, GTlsCertificateFlags errors, Tab *t) {
    push(t->engine, EV_FAILED, t->id, 0, 0, 0, uri ? uri : "", "The site's certificate cannot be trusted");
    return FALSE;
}

static gboolean on_option_menu(WebKitWebView *view, WebKitOptionMenu *menu, WebKitRectangle *rect, Tab *t) {
    if (t->menu)
        g_object_unref(t->menu);
    t->menu = g_object_ref(menu);
    MeshEvent *ev = g_new0(MeshEvent, 1);
    ev->kind = EV_CHOOSE;
    ev->tab = t->id;
    ev->x = rect->x;
    ev->y = rect->y;
    ev->w = rect->width;
    ev->h = rect->height;
    ev->a = (int)webkit_option_menu_get_n_items(menu);
    g_queue_push_tail(t->engine->events, ev);
    return TRUE;
}

static gboolean on_dialog(WebKitWebView *view, WebKitScriptDialog *dialog, Tab *t) {
    if (t->dialog) {
        webkit_script_dialog_close(t->dialog);
        webkit_script_dialog_unref(t->dialog);
    }
    t->dialog = webkit_script_dialog_ref(dialog);
    int kind = 0;
    switch (webkit_script_dialog_get_dialog_type(dialog)) {
    case WEBKIT_SCRIPT_DIALOG_CONFIRM: kind = 1; break;
    case WEBKIT_SCRIPT_DIALOG_PROMPT: kind = 2; break;
    case WEBKIT_SCRIPT_DIALOG_BEFORE_UNLOAD_CONFIRM: kind = 3; break;
    default: kind = 0; break;
    }
    const char *text = kind == 2 ? webkit_script_dialog_prompt_get_default_text(dialog) : "";
    push(t->engine, EV_DIALOG, t->id, 0, kind, 0, webkit_script_dialog_get_message(dialog), text ? text : "");
    return TRUE;
}

/* What would otherwise be a window of the platform's own. */
static gboolean on_file_chooser(WebKitWebView *view, WebKitFileChooserRequest *request, Tab *t) {
    webkit_file_chooser_request_cancel(request);
    return TRUE;
}

static gboolean on_context_menu(WebKitWebView *view, WebKitContextMenu *menu, WebKitHitTestResult *hit, Tab *t) {
    return TRUE;
}

static gboolean on_permission(WebKitWebView *view, WebKitPermissionRequest *request, Tab *t) {
    webkit_permission_request_deny(request);
    return TRUE;
}

static gboolean on_swallow(WebKitWebView *view, gpointer what, Tab *t) {
    return TRUE;
}

static void on_picture(gpointer data) {
    ((Tab *)data)->fresh = 1;
}

/* The developer tools' tab waiting for the view WebKit makes for them (spatimesh_inspect): the
 * next view made is theirs. */
static Tab *waiting_inspector;
static Tab *find(struct Engine *e, uint64_t id);

/* The developer tools were closed (from inside them, or with their page): their tab goes. */
static void on_inspector_gone(gpointer data) {
    Tab *tools = data;
    if (!tools->inspected)
        return;
    Tab *page = find(tools->engine, tools->inspected);
    if (page)
        page->inspector = 0;
    tools->inspected = 0;
    push(tools->engine, EV_CLOSE_TAB, tools->id, 0, 0, 0, NULL, NULL);
}

static void bind_inspector_view(MeshView *v) {
    Tab *t = waiting_inspector;
    if (!t)
        return;
    waiting_inspector = NULL;
    /* Held by the tab: WebKit lets go of it when the developer tools close, maybe before the
     * window has closed their tab. */
    t->wpe = g_object_ref(v);
    v->on_picture = on_picture;
    v->data = t;
    v->tab_width = t->width;
    v->tab_height = t->height;
    v->on_gone = on_inspector_gone;
    watch_toplevel(v);
    wpe_view_set_visible(WPE_VIEW(v), TRUE);
}

/* ---- the engine --------------------------------------------------------------------------- */

static void free_tab(Tab *t) {
    if (t->menu) {
        webkit_option_menu_close(t->menu);
        g_object_unref(t->menu);
    }
    if (t->dialog) {
        webkit_script_dialog_close(t->dialog);
        webkit_script_dialog_unref(t->dialog);
    }
    if (t->wpe)
        t->wpe->on_picture = NULL;
    if (waiting_inspector == t)
        waiting_inspector = NULL;
    if (t->inspected) {
        /* The developer tools' tab: they close with it. */
        Tab *page = find_page(t->engine, t->inspected);
        if (page) {
            page->inspector = 0;
            if (t->wpe)
                t->wpe->on_gone = NULL;
            webkit_web_view_toggle_inspector(page->view);
        }
    } else if (t->inspector) {
        /* A page's: its developer tools go with it. */
        Tab *tools = find(t->engine, t->inspector);
        if (tools) {
            tools->inspected = 0;
            push(t->engine, EV_CLOSE_TAB, tools->id, 0, 0, 0, NULL, NULL);
        }
    }
    if (t->view)
        g_object_unref(t->view);
    else if (t->wpe) {
        t->wpe->on_gone = NULL;
        unwatch_toplevel(t->wpe);
        g_object_unref(t->wpe);
    }
    if (t->worlds)
        g_ptr_array_unref(t->worlds);
    g_free(t);
}

const char *spatimesh_version(void) {
    static char version[64];
    g_snprintf(version, sizeof version, "WPE WebKit %u.%u.%u", webkit_get_major_version(), webkit_get_minor_version(), webkit_get_micro_version());
    return version;
}

/* ---- extensions (docs/spatimesh.md, "Extensions") ---------------------------------------- */

/* The window's runtime answers everything; here are only the bridge (a script message handler
 * named spatimeshExt, in each extension's own world and in the pages' world), the content
 * scripts and style sheets as WebKit's user scripts and style sheets, the extensions' files
 * (served by the Rust side, crates/spatimesh-shim), and code run in a tab when asked. */

/* What answers chrome-extension:// and moz-extension://: the shim's, handed over at start. */
typedef int (*ServeFn)(const char *url, const char *initiator, const char **mime, unsigned char **bytes, size_t *len);
typedef void (*ServeFreeFn)(unsigned char *bytes, size_t len);
static ServeFn serve_fn;
static ServeFreeFn serve_free_fn;

void spatimesh_ext_server(ServeFn serve, ServeFreeFn free_bytes) {
    serve_fn = serve;
    serve_free_fn = free_bytes;
}

/* What a context's api.js hands its calls to: the handler in its own world. */
const char *spatimesh_ext_bridge(void) {
    return "(function(){var h=window.webkit.messageHandlers.spatimeshExt;return function(s){h.postMessage(s);};})()";
}

static void on_scheme(WebKitURISchemeRequest *request, gpointer data) {
    const char *uri = webkit_uri_scheme_request_get_uri(request);
    /* Who asks: the page whose request it is, as its headers say, or the tab's page. */
    const char *initiator = NULL;
    SoupMessageHeaders *headers = webkit_uri_scheme_request_get_http_headers(request);
    if (headers) {
        initiator = soup_message_headers_get_one(headers, "Referer");
        if (!initiator)
            initiator = soup_message_headers_get_one(headers, "Origin");
    }
    WebKitWebView *view = webkit_uri_scheme_request_get_web_view(request);
    if (!initiator && view)
        initiator = webkit_web_view_get_uri(view);
    const char *mime = "text/plain";
    unsigned char *bytes = NULL;
    size_t len = 0;
    int status = serve_fn ? serve_fn(uri, initiator ? initiator : "", &mime, &bytes, &len) : 404;
    GBytes *body = g_bytes_new(bytes, len);
    if (serve_free_fn)
        serve_free_fn(bytes, len);
    GInputStream *stream = g_memory_input_stream_new_from_bytes(body);
    WebKitURISchemeResponse *response = webkit_uri_scheme_response_new(stream, (gint64)len);
    webkit_uri_scheme_response_set_status(response, status, NULL);
    webkit_uri_scheme_response_set_content_type(response, mime);
    SoupMessageHeaders *out = soup_message_headers_new(SOUP_MESSAGE_HEADERS_RESPONSE);
    soup_message_headers_append(out, "Access-Control-Allow-Origin", "*");
    soup_message_headers_append(out, "Cache-Control", "no-cache");
    webkit_uri_scheme_response_set_http_headers(response, out);
    webkit_uri_scheme_request_finish_with_response(request, response);
    g_object_unref(response);
    g_object_unref(stream);
    g_bytes_unref(body);
}

static void free_ext(Ext *x) {
    g_free(x->id);
    g_free(x->scheme);
    g_free(x->world);
    g_free(x->content_boot);
    g_free(x->page_boot);
    g_free(x);
}

static void free_entry(Entry *n) {
    g_strfreev(n->allow);
    g_strfreev(n->block);
    g_free(n->guard);
    g_ptr_array_unref(n->js);
    g_ptr_array_unref(n->css);
    g_free(n);
}

/* "a\nb" as a NULL-ended list; NULL for "". */
static char **lines(const char *text) {
    if (!text || !*text)
        return NULL;
    return g_strsplit(text, "\n", -1);
}

static Ext *ext_of_world(Engine *e, const char *world) {
    for (guint i = 0; e->exts && i < e->exts->len; i++) {
        Ext *x = g_ptr_array_index(e->exts, i);
        if (g_strcmp0(x->world, world) == 0)
            return x;
    }
    return NULL;
}

/* The compiled content blockers on a tab's view, in place of what was there. */
static void apply_filters(Tab *t) {
    WebKitUserContentManager *m = webkit_web_view_get_user_content_manager(t->view);
    webkit_user_content_manager_remove_all_filters(m);
    if (!t->engine->filters)
        return;
    GHashTableIter it;
    gpointer key, value;
    g_hash_table_iter_init(&it, t->engine->filters);
    while (g_hash_table_iter_next(&it, &key, &value))
        webkit_user_content_manager_add_filter(m, value);
}

static void on_filter_saved(GObject *source, GAsyncResult *result, gpointer data) {
    Engine *e = data;
    GError *error = NULL;
    WebKitUserContentFilter *filter = webkit_user_content_filter_store_save_finish(WEBKIT_USER_CONTENT_FILTER_STORE(source), result, &error);
    if (!filter) {
        g_warning("an extension's declarativeNetRequest rules could not be made WebKit's: %s", error ? error->message : "?");
        g_clear_error(&error);
        return;
    }
    const char *id = webkit_user_content_filter_get_identifier(filter);
    /* An extension gone since it was asked for keeps no rules. */
    if (!e->rule_sources || !g_hash_table_contains(e->rule_sources, id)) {
        webkit_user_content_filter_unref(filter);
        return;
    }
    g_hash_table_replace(e->filters, g_strdup(id), filter);
    GHashTableIter it;
    gpointer key, value;
    g_hash_table_iter_init(&it, e->tabs);
    while (g_hash_table_iter_next(&it, &key, &value))
        apply_filters(value);
}

/* declarativeNetRequest: each extension's rules as WebKit's content blockers, compiled (in the
 * background) and put on every tab's view; an extension's that are not in the new set taken
 * off. Pages loaded from then on have them. */
static void compile_filters(Engine *e) {
    if (!e->filters)
        e->filters = g_hash_table_new_full(g_str_hash, g_str_equal, g_free, (GDestroyNotify)webkit_user_content_filter_unref);
    GHashTableIter it;
    gpointer key, value;
    g_hash_table_iter_init(&it, e->filters);
    while (g_hash_table_iter_next(&it, &key, &value))
        if (!e->rule_sources || !g_hash_table_contains(e->rule_sources, key))
            g_hash_table_iter_remove(&it);
    if (!e->rule_sources || !e->filter_store)
        return;
    g_hash_table_iter_init(&it, e->rule_sources);
    while (g_hash_table_iter_next(&it, &key, &value)) {
        GBytes *json = g_bytes_new(value, strlen(value));
        webkit_user_content_filter_store_save(e->filter_store, key, json, NULL, on_filter_saved, e);
        g_bytes_unref(json);
    }
}

/* What a profile's extensions' rules make of a request is WebKit's to decide here: no asking. */
typedef int (*DecideFn)(const char *url, const char *initiator, int kind, const char *method, char *redirect, size_t cap);
void spatimesh_ext_decider(DecideFn decide) {
    (void)decide;
}

/* An extension's rules, as WebKit's content blockers (JSON): compiled when the set is whole. */
void spatimesh_ext_rules(Engine *e, const char *id, const char *webkit) {
    if (!e->rule_sources)
        e->rule_sources = g_hash_table_new_full(g_str_hash, g_str_equal, g_free, g_free);
    g_hash_table_replace(e->rule_sources, g_strdup(id), g_strdup(webkit));
}

/* What the media script in a tab's top frame says to the window. */
static void on_media_message(WebKitUserContentManager *manager, JSCValue *value, Tab *t) {
    char *text = jsc_value_to_string(value);
    push(t->engine, EV_MEDIA, t->id, 0, 0, 0, text, NULL);
    g_free(text);
}

static void on_ext_message(WebKitUserContentManager *manager, JSCValue *value, Tab *t) {
    char *text = jsc_value_to_string(value);
    push(t->engine, EV_EXTENSION, t->id, 0, 0, 0, text, NULL);
    g_free(text);
}

/* The profile's extensions into a tab's view: their scripts, their style sheets, and the
 * handlers their contexts speak to, in place of what was there. Pages loaded from now on
 * have them. */
static void apply_extensions(Tab *t) {
    Engine *e = t->engine;
    WebKitUserContentManager *m = webkit_web_view_get_user_content_manager(t->view);
    webkit_user_content_manager_remove_all_scripts(m);
    webkit_user_content_manager_remove_all_style_sheets(m);
    if (!t->worlds) {
        t->worlds = g_ptr_array_new_with_free_func(g_free);
        g_signal_connect(m, "script-message-received::spatimeshExt", G_CALLBACK(on_ext_message), t);
    }
    /* SpatiMesh's media script first of all, in every frame, the top one given its way out. */
    if (e->page_script) {
        if (!t->media_handler) {
            g_signal_connect(m, "script-message-received::spatimeshMedia", G_CALLBACK(on_media_message), t);
            t->media_handler = webkit_user_content_manager_register_script_message_handler(m, "spatimeshMedia", NULL);
        }
        char *code = g_strdup_printf(
            "if(window.top===window){(function(){var h=window.webkit&&window.webkit.messageHandlers&&window.webkit.messageHandlers.spatimeshMedia;"
            "if(h){var p=h.postMessage.bind(h);window.__spatimeshMediaPost=function(s){p(s);};}})();}\n%s",
            e->page_script);
        WebKitUserScript *script = webkit_user_script_new(code, WEBKIT_USER_CONTENT_INJECT_ALL_FRAMES, WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_START, NULL, NULL);
        webkit_user_content_manager_add_script(m, script);
        webkit_user_script_unref(script);
        g_free(code);
    }
    for (guint i = 0; i < t->worlds->len; i++)
        webkit_user_content_manager_unregister_script_message_handler(m, "spatimeshExt", g_ptr_array_index(t->worlds, i));
    g_ptr_array_set_size(t->worlds, 0);
    if (t->main_handler) {
        webkit_user_content_manager_unregister_script_message_handler(m, "spatimeshExt", NULL);
        t->main_handler = 0;
    }
    if (!e->exts || e->exts->len == 0)
        return;
    t->main_handler = webkit_user_content_manager_register_script_message_handler(m, "spatimeshExt", NULL);
    for (guint i = 0; i < e->exts->len; i++) {
        Ext *x = g_ptr_array_index(e->exts, i);
        if (webkit_user_content_manager_register_script_message_handler(m, "spatimeshExt", x->world))
            g_ptr_array_add(t->worlds, g_strdup(x->world));
        /* Its own pages, in every frame they are in: their API in the pages' own world. */
        char *own = g_strdup_printf("%s://%s/*", x->scheme, x->id);
        const char *allow[] = {own, NULL};
        char *prefix = g_strdup_printf("%s://%s/", x->scheme, x->id);
        char *quoted = g_strescape(prefix, NULL);
        char *code = g_strdup_printf("if(location.href.indexOf(\"%s\")===0){%s}", quoted, x->page_boot);
        WebKitUserScript *script = webkit_user_script_new(code, WEBKIT_USER_CONTENT_INJECT_ALL_FRAMES, WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_START, allow, NULL);
        webkit_user_content_manager_add_script(m, script);
        webkit_user_script_unref(script);
        g_free(code);
        g_free(quoted);
        g_free(prefix);
        g_free(own);
    }
    for (guint i = 0; e->entries && i < e->entries->len; i++) {
        Entry *n = g_ptr_array_index(e->entries, i);
        WebKitUserContentInjectedFrames frames = n->all_frames ? WEBKIT_USER_CONTENT_INJECT_ALL_FRAMES : WEBKIT_USER_CONTENT_INJECT_TOP_FRAME;
        WebKitUserScriptInjectionTime when = n->run_at == 0 ? WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_START : WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_END;
        const char *world = n->main_world ? NULL : n->ext->world;
        const char *const *allow = (const char *const *)n->allow;
        const char *const *block = (const char *const *)n->block;
        for (guint c = 0; c < n->css->len; c++) {
            WebKitUserStyleSheet *sheet = world
                ? webkit_user_style_sheet_new_for_world(g_ptr_array_index(n->css, c), frames, WEBKIT_USER_STYLE_LEVEL_AUTHOR, world, allow, block)
                : webkit_user_style_sheet_new(g_ptr_array_index(n->css, c), frames, WEBKIT_USER_STYLE_LEVEL_AUTHOR, allow, block);
            webkit_user_content_manager_add_style_sheet(m, sheet);
            webkit_user_style_sheet_unref(sheet);
        }
        /* Its API first, in its world (a script in the page's own world has none). */
        if (world) {
            char *code = n->exact ? g_strdup(n->ext->content_boot) : g_strdup_printf("if(%s){%s}", n->guard, n->ext->content_boot);
            WebKitUserScript *boot = webkit_user_script_new_for_world(code, frames, when, world, allow, block);
            webkit_user_content_manager_add_script(m, boot);
            webkit_user_script_unref(boot);
            g_free(code);
        }
        for (guint j = 0; j < n->js->len; j++) {
            /* Each file a script of its own, as in Chrome: its top-level names shared with the
             * others in the world. Where the patterns could not say exactly which pages, the
             * file stops at once where it is not wanted. */
            const char *file = g_ptr_array_index(n->js, j);
            char *code = n->exact ? g_strdup(file) : g_strdup_printf("if(!(%s))throw new Error('not for this page');\n%s", n->guard, file);
            WebKitUserScript *script = world ? webkit_user_script_new_for_world(code, frames, when, world, allow, block) : webkit_user_script_new(code, frames, when, allow, block);
            webkit_user_content_manager_add_script(m, script);
            webkit_user_script_unref(script);
            g_free(code);
        }
    }
}

void spatimesh_ext_clear(Engine *e) {
    if (e->entries)
        g_ptr_array_unref(e->entries);
    if (e->exts)
        g_ptr_array_unref(e->exts);
    e->exts = g_ptr_array_new_with_free_func((GDestroyNotify)free_ext);
    e->entries = g_ptr_array_new_with_free_func((GDestroyNotify)free_entry);
    if (e->rule_sources)
        g_hash_table_remove_all(e->rule_sources);
}

void spatimesh_ext_add(Engine *e, const char *id, const char *scheme, const char *world, const char *content_boot, const char *page_boot) {
    if (!e->exts)
        spatimesh_ext_clear(e);
    Ext *x = g_new0(Ext, 1);
    x->id = g_strdup(id);
    x->scheme = g_strdup(scheme);
    x->world = g_strdup(world);
    x->content_boot = g_strdup(content_boot);
    x->page_boot = g_strdup(page_boot);
    g_ptr_array_add(e->exts, x);
}

void spatimesh_ext_entry(Engine *e, const char *id, int run_at, int all_frames, int main_world, const char *allow, const char *block, const char *guard, int exact) {
    if (!e->exts || e->exts->len == 0)
        return;
    Ext *x = NULL;
    for (guint i = 0; i < e->exts->len; i++) {
        Ext *each = g_ptr_array_index(e->exts, i);
        if (strcmp(each->id, id) == 0)
            x = each;
    }
    if (!x)
        return;
    Entry *n = g_new0(Entry, 1);
    n->ext = x;
    n->run_at = run_at;
    n->all_frames = all_frames;
    n->main_world = main_world;
    n->allow = lines(allow);
    n->block = lines(block);
    n->guard = g_strdup(guard && *guard ? guard : "true");
    n->exact = exact;
    n->js = g_ptr_array_new_with_free_func(g_free);
    n->css = g_ptr_array_new_with_free_func(g_free);
    g_ptr_array_add(e->entries, n);
}

/* A file of the entry added last. */
void spatimesh_ext_js(Engine *e, const char *code) {
    if (e->entries && e->entries->len)
        g_ptr_array_add(((Entry *)g_ptr_array_index(e->entries, e->entries->len - 1))->js, g_strdup(code));
}

void spatimesh_ext_css(Engine *e, const char *code) {
    if (e->entries && e->entries->len)
        g_ptr_array_add(((Entry *)g_ptr_array_index(e->entries, e->entries->len - 1))->css, g_strdup(code));
}

/* The set is whole: every tab has it for the pages it loads from now on. */
void spatimesh_ext_done(Engine *e) {
    compile_filters(e);
    GHashTableIter it;
    gpointer key, value;
    g_hash_table_iter_init(&it, e->tabs);
    while (g_hash_table_iter_next(&it, &key, &value)) {
        apply_extensions(value);
        apply_filters(value);
    }
}

typedef struct {
    Engine *engine;
    uint64_t tab;
    uint64_t request;
} Asked;

static void on_script_done(GObject *source, GAsyncResult *result, gpointer data) {
    Asked *asked = data;
    GError *error = NULL;
    JSCValue *value = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(source), result, &error);
    if (asked->request) {
        if (value) {
            char *json = jsc_value_is_undefined(value) ? NULL : jsc_value_to_json(value, 0);
            push(asked->engine, EV_SCRIPT_RESULT, asked->tab, (double)asked->request, 1, 0, json ? json : "null", NULL);
            g_free(json);
        } else {
            push(asked->engine, EV_SCRIPT_RESULT, asked->tab, (double)asked->request, 0, 0, error ? error->message : "the script failed", NULL);
        }
    }
    if (value)
        g_object_unref(value);
    g_clear_error(&error);
    g_free(asked);
}

/* SpatiMesh's own script for every frame's every document (its media), from the next document
 * on in every tab. */
void spatimesh_set_page_script(Engine *e, const char *code) {
    /* SpatiMesh's own player, put into pages by this script, is for engines that cannot play
     * pages' media themselves. WPE plays it with its own GStreamer player (decoding with
     * Spatiand's FFmpeg and the platform's hardware), so pages are left as they are. */
    (void)code;
    g_free(e->page_script);
    e->page_script = NULL;
    GHashTableIter it;
    gpointer key, value;
    g_hash_table_iter_init(&it, e->tabs);
    while (g_hash_table_iter_next(&it, &key, &value))
        apply_extensions(value);
}

/* Code run in a tab's top frame, in `world` ("" the page's own), the extension's API set up
 * there first if `boot`. Only the top frame here: WebKit's GLib API runs scripts from outside the
 * page in no other (`all_frames` is for engines that can). */
void spatimesh_run_script(Engine *e, uint64_t id, const char *world, int all_frames, const char *code, uint64_t request, int boot) {
    Tab *t = find_page(e, id);
    if (!t) {
        if (request)
            push(e, EV_SCRIPT_RESULT, id, (double)request, 0, 0, "No such tab", NULL);
        return;
    }
    (void)all_frames;
    const char *in = world && *world ? world : NULL;
    char *full = NULL;
    if (boot && in) {
        Ext *x = ext_of_world(e, in);
        if (x)
            full = g_strdup_printf("%s\n%s", x->content_boot, code);
    }
    Asked *asked = g_new0(Asked, 1);
    asked->engine = e;
    asked->tab = id;
    asked->request = request;
    webkit_web_view_evaluate_javascript(t->view, full ? full : code, -1, in, NULL, NULL, on_script_done, asked);
    g_free(full);
}

/* A profile's engine: `data` and `cache` are its folders, or NULL for the private profile,
 * which keeps everything in memory. */
Engine *spatimesh_new(const char *data, const char *cache) {
    Engine *e = g_new0(Engine, 1);
    mesh_display();
    e->session = data && cache ? webkit_network_session_new(data, cache) : webkit_network_session_new_ephemeral();
    e->context = webkit_web_context_new();
    WebKitCookieManager *cookies = webkit_network_session_get_cookie_manager(e->session);
    if (data) {
        char *file = g_build_filename(data, "cookies.sqlite", NULL);
        webkit_cookie_manager_set_persistent_storage(cookies, file, WEBKIT_COOKIE_PERSISTENT_STORAGE_SQLITE);
        g_free(file);
    }
    webkit_cookie_manager_set_accept_policy(cookies, WEBKIT_COOKIE_POLICY_ACCEPT_NO_THIRD_PARTY);
    e->cookies = g_object_ref(cookies);
    webkit_web_context_set_cache_model(e->context, WEBKIT_CACHE_MODEL_WEB_BROWSER);
    e->settings = webkit_settings_new();
    webkit_settings_set_enable_smooth_scrolling(e->settings, TRUE);
    webkit_settings_set_javascript_can_open_windows_automatically(e->settings, FALSE);
    /* The developer tools (Web Inspector), opened from SpatiMesh's menu. */
    webkit_settings_set_enable_developer_extras(e->settings, TRUE);
    e->tabs = g_hash_table_new_full(g_int64_hash, g_int64_equal, g_free, (GDestroyNotify)free_tab);
    e->opened = g_hash_table_new_full(g_direct_hash, g_direct_equal, NULL, g_object_unref);
    e->events = g_queue_new();
    /* Extensions' own addresses: their files, secure, and reachable from other origins where
     * they let them be (what the shim's serve says). */
    WebKitSecurityManager *security = webkit_web_context_get_security_manager(e->context);
    const char *schemes[] = {"chrome-extension", "moz-extension"};
    for (int i = 0; i < 2; i++) {
        webkit_web_context_register_uri_scheme(e->context, schemes[i], on_scheme, e, NULL);
        webkit_security_manager_register_uri_scheme_as_secure(security, schemes[i]);
        webkit_security_manager_register_uri_scheme_as_cors_enabled(security, schemes[i]);
    }
    if (g_getenv("SPATIMESH_CONSOLE"))
        webkit_settings_set_enable_write_console_messages_to_stdout(e->settings, TRUE);
    /* Extensions' content blockers, compiled: in the profile's cache, or a folder of the
     * process's own for the private profile (compiled rules are no one's data). */
    char *filters = cache ? g_build_filename(cache, "content-filters", NULL) : g_dir_make_tmp("spatimesh-filters-XXXXXX", NULL);
    if (filters) {
        e->filter_store = webkit_user_content_filter_store_new(filters);
        g_free(filters);
    }
    return e;
}

static gboolean on_wake(gint fd, GIOCondition condition, gpointer data) {
    ((Engine *)data)->woken = 1;
    return G_SOURCE_CONTINUE;
}

/* Stop waiting when `fd` has something to read: the window's socket. */
void spatimesh_watch(Engine *e, int fd) {
    if (e->watch)
        g_source_remove(e->watch);
    e->watch = g_unix_fd_add(fd, G_IO_IN | G_IO_HUP | G_IO_ERR, on_wake, e);
}

int spatimesh_open(Engine *e, uint64_t id, const char *uri, int width, int height, double zoom) {
    fprintf(stderr, "spatimesh-wpe: tab %llu opens %s\n", (unsigned long long)id, uri ? uri : "");
    if (find(e, id))
        return 0;
    Tab *t = g_new0(Tab, 1);
    t->id = id;
    t->engine = e;
    t->width = MAX(width, 1);
    t->height = MAX(height, 1);
    /* A view WebKit already made, for a page's window.open (on_create). */
    WebKitWebView *opened = NULL;
    if (uri && g_str_has_prefix(uri, OPENED_PREFIX)) {
        gpointer number = GUINT_TO_POINTER((unsigned)g_ascii_strtoull(uri + strlen(OPENED_PREFIX), NULL, 10));
        opened = g_hash_table_lookup(e->opened, number);
        if (opened) {
            g_object_ref(opened);
            g_hash_table_remove(e->opened, number);
        }
        uri = opened ? NULL : "about:blank";
    }
    t->view = opened ? opened : WEBKIT_WEB_VIEW(g_object_new(WEBKIT_TYPE_WEB_VIEW, "display", mesh_display(), "web-context", e->context,
        "network-session", e->session, "settings", e->settings, NULL));
    t->wpe = (MeshView *)webkit_web_view_get_wpe_view(t->view);
    t->wpe->on_picture = on_picture;
    t->wpe->data = t;
    wpe_toplevel_resize(wpe_view_get_toplevel(WPE_VIEW(t->wpe)), t->width, t->height);
    wpe_view_set_visible(WPE_VIEW(t->wpe), TRUE);
    webkit_web_view_set_zoom_level(t->view, zoom > 0.1 ? zoom : 1.0);
    g_signal_connect(t->view, "notify::title", G_CALLBACK(on_title), t);
    g_signal_connect(t->view, "notify::uri", G_CALLBACK(on_uri), t);
    g_signal_connect(t->view, "notify::estimated-load-progress", G_CALLBACK(on_progress), t);
    g_signal_connect(t->view, "load-changed", G_CALLBACK(on_load_changed), t);
    g_signal_connect(t->view, "notify::is-playing-audio", G_CALLBACK(on_audio), t);
    g_signal_connect(t->view, "mouse-target-changed", G_CALLBACK(on_target), t);
    g_signal_connect(t->view, "decide-policy", G_CALLBACK(on_policy), t);
    g_signal_connect(t->view, "create", G_CALLBACK(on_create), t);
    g_signal_connect(t->view, "close", G_CALLBACK(on_close), t);
    g_signal_connect(t->view, "notify::page-id", G_CALLBACK(tell_sound_name), t);
    tell_sound_name(t->view, NULL, t);
    g_signal_connect(t->view, "web-process-terminated", G_CALLBACK(on_terminated), t);
    g_signal_connect(t->view, "load-failed", G_CALLBACK(on_failed), t);
    g_signal_connect(t->view, "load-failed-with-tls-errors", G_CALLBACK(on_tls), t);
    g_signal_connect(t->view, "show-option-menu", G_CALLBACK(on_option_menu), t);
    g_signal_connect(t->view, "script-dialog", G_CALLBACK(on_dialog), t);
    g_signal_connect(t->view, "run-file-chooser", G_CALLBACK(on_file_chooser), t);
    g_signal_connect(t->view, "context-menu", G_CALLBACK(on_context_menu), t);
    g_signal_connect(t->view, "permission-request", G_CALLBACK(on_permission), t);
    g_signal_connect(t->view, "show-notification", G_CALLBACK(on_swallow), t);
    uint64_t *key = g_new(uint64_t, 1);
    *key = id;
    g_hash_table_insert(e->tabs, key, t);
    apply_extensions(t);
    apply_filters(t);
    if (opened) {
        /* Loading already, as its opener asked: the window is told where it is. */
        on_uri(t->view, NULL, t);
        on_title(t->view, NULL, t);
    } else
        webkit_web_view_load_uri(t->view, uri && *uri ? uri : "about:blank");
    return 1;
}

void spatimesh_close(Engine *e, uint64_t id) {
    g_hash_table_remove(e->tabs, &id);
}

/* The developer tools (Web Inspector) for the tab `id`, as the tab `inspector`, `width` by
 * `height`. WebKit makes their page and view itself, on SpatiMesh's display: the next view made
 * is theirs (bind_inspector_view). 0 if the tab has no page, or has them open already. */
int spatimesh_inspect(Engine *e, uint64_t id, uint64_t inspector, int width, int height) {
    Tab *page = find_page(e, id);
    if (!page || page->inspector || find(e, inspector))
        return 0;
    Tab *t = g_new0(Tab, 1);
    t->id = inspector;
    t->engine = e;
    t->width = MAX(width, 1);
    t->height = MAX(height, 1);
    t->inspected = id;
    uint64_t *key = g_new(uint64_t, 1);
    *key = inspector;
    g_hash_table_insert(e->tabs, key, t);
    page->inspector = inspector;
    waiting_inspector = t;
    webkit_web_view_toggle_inspector(page->view);
    push(e, EV_TITLE, inspector, 0, 0, 0, "Developer tools", NULL);
    return 1;
}

void spatimesh_load(Engine *e, uint64_t id, const char *uri) {
    Tab *t = find_page(e, id);
    if (t)
        webkit_web_view_load_uri(t->view, uri);
}

void spatimesh_back(Engine *e, uint64_t id) {
    Tab *t = find_page(e, id);
    if (t)
        webkit_web_view_go_back(t->view);
}

void spatimesh_forward(Engine *e, uint64_t id) {
    Tab *t = find_page(e, id);
    if (t)
        webkit_web_view_go_forward(t->view);
}

void spatimesh_reload(Engine *e, uint64_t id) {
    Tab *t = find_page(e, id);
    if (t)
        webkit_web_view_reload(t->view);
}

void spatimesh_stop(Engine *e, uint64_t id) {
    Tab *t = find_page(e, id);
    if (t)
        webkit_web_view_stop_loading(t->view);
}

void spatimesh_resize(Engine *e, uint64_t id, int width, int height, double zoom) {
    Tab *t = find(e, id);
    if (!t)
        return;
    t->width = MAX(width, 1);
    t->height = MAX(height, 1);
    if (t->wpe && t->inspected) {
        t->wpe->tab_width = t->width;
        t->wpe->tab_height = t->height;
    }
    if (t->wpe && wpe_view_get_toplevel(WPE_VIEW(t->wpe)))
        wpe_toplevel_resize(wpe_view_get_toplevel(WPE_VIEW(t->wpe)), t->width, t->height);
    if (t->view && zoom > 0.1 && webkit_web_view_get_zoom_level(t->view) != zoom)
        webkit_web_view_set_zoom_level(t->view, zoom);
}

void spatimesh_mute(Engine *e, uint64_t id, int muted) {
    Tab *t = find_page(e, id);
    if (t)
        webkit_web_view_set_is_muted(t->view, muted);
}

/* ---- input ---------------------------------------------------------------------------------- */

static WPEModifiers modifiers_of(int modifiers) {
    guint m = 0;
    if (modifiers & 1) m |= WPE_MODIFIER_KEYBOARD_SHIFT;
    if (modifiers & 2) m |= WPE_MODIFIER_KEYBOARD_CONTROL;
    if (modifiers & 4) m |= WPE_MODIFIER_KEYBOARD_ALT;
    if (modifiers & 8) m |= WPE_MODIFIER_KEYBOARD_META;
    return (WPEModifiers)m;
}

static void deliver(Tab *t, WPEEvent *ev) {
    wpe_view_event(WPE_VIEW(t->wpe), ev);
    wpe_event_unref(ev);
}

static guint button_mask(int button) {
    switch (button) {
    case 1: return WPE_MODIFIER_POINTER_BUTTON1;
    case 2: return WPE_MODIFIER_POINTER_BUTTON2;
    case 3: return WPE_MODIFIER_POINTER_BUTTON3;
    default: return 0;
    }
}

static void cross(Tab *t, double x, double y, gboolean in) {
    deliver(t, wpe_event_pointer_move_new(in ? WPE_EVENT_POINTER_ENTER : WPE_EVENT_POINTER_LEAVE, WPE_VIEW(t->wpe), WPE_INPUT_SOURCE_MOUSE,
        now_ms(), t->buttons, x, y, 0, 0));
}

/* `button`: 0 none (a move), 1 left, 2 middle, 3 right, 4 back, 5 forward. */
void spatimesh_pointer(Engine *e, uint64_t id, double x, double y, int button, int pressed, int modifiers) {
    Tab *t = find_shown(e, id);
    if (!t)
        return;
    if (!t->entered) {
        cross(t, x, y, TRUE);
        t->entered = 1;
    }
    WPEModifiers state = (WPEModifiers)(modifiers_of(modifiers) | t->buttons);
    if (button == 0) {
        deliver(t, wpe_event_pointer_move_new(WPE_EVENT_POINTER_MOVE, WPE_VIEW(t->wpe), WPE_INPUT_SOURCE_MOUSE, now_ms(), state, x, y, x - t->x, y - t->y));
        t->x = x;
        t->y = y;
        return;
    }
    /* WPE numbers buttons as GDK does: back and forward 8 and 9. */
    guint wpe_button = button == 4 ? 8 : button == 5 ? 9 : (guint)button;
    guint32 now = now_ms();
    if (pressed) {
        /* Twice in the same place, soon enough, is a double click: WPE is told the count. */
        t->presses = now - t->last_press < 400 && fabs(x - t->x) < 4 && fabs(y - t->y) < 4 ? t->presses + 1 : 1;
        t->last_press = now;
    }
    deliver(t, wpe_event_pointer_button_new(pressed ? WPE_EVENT_POINTER_DOWN : WPE_EVENT_POINTER_UP, WPE_VIEW(t->wpe), WPE_INPUT_SOURCE_MOUSE,
        now, state, wpe_button, x, y, pressed ? t->presses : 0));
    t->x = x;
    t->y = y;
    if (pressed)
        t->buttons |= button_mask(button);
    else
        t->buttons &= ~button_mask(button);
}

void spatimesh_leave(Engine *e, uint64_t id) {
    Tab *t = find_shown(e, id);
    if (t && t->entered) {
        cross(t, -1, -1, FALSE);
        t->entered = 0;
        t->buttons = 0;
    }
}

void spatimesh_scroll(Engine *e, uint64_t id, double x, double y, double dx, double dy, int modifiers) {
    Tab *t = find_shown(e, id);
    if (!t)
        return;
    if (!t->entered) {
        cross(t, x, y, TRUE);
        t->entered = 1;
    }
    /* In pixels, as a trackpad's: the page moves by what the wearer moved. */
    deliver(t, wpe_event_scroll_new(WPE_VIEW(t->wpe), WPE_INPUT_SOURCE_MOUSE, now_ms(), modifiers_of(modifiers), -dx, -dy, TRUE, FALSE, x, y));
}

/* A key's WPE name from SpatiMesh's: `key` is the wire's number (1 Enter ... 15 a character,
 * `ch`), `code` Linux's number for the key, for the ones the wire has no name for. */
static guint keyval_of(int key, guint32 ch, int code) {
    switch (key) {
    case 1: return WPE_KEY_Return;
    case 2: return WPE_KEY_Escape;
    case 3: return WPE_KEY_BackSpace;
    case 4: return WPE_KEY_Delete;
    case 5: return WPE_KEY_Tab;
    case 6: return WPE_KEY_space;
    case 7: return WPE_KEY_Left;
    case 8: return WPE_KEY_Right;
    case 9: return WPE_KEY_Up;
    case 10: return WPE_KEY_Down;
    case 11: return WPE_KEY_Home;
    case 12: return WPE_KEY_End;
    case 13: return WPE_KEY_Page_Up;
    case 14: return WPE_KEY_Page_Down;
    case 15: return wpe_unicode_to_keyval(ch);
    default: break;
    }
    if (code >= 59 && code <= 68)
        return WPE_KEY_F1 + (guint)(code - 59);
    switch (code) {
    case 87: return WPE_KEY_F11;
    case 88: return WPE_KEY_F12;
    case 110: return WPE_KEY_Insert;
    case 42: return WPE_KEY_Shift_L;
    case 54: return WPE_KEY_Shift_R;
    case 29: return WPE_KEY_Control_L;
    case 97: return WPE_KEY_Control_R;
    case 56: return WPE_KEY_Alt_L;
    case 100: return WPE_KEY_Alt_R;
    case 125: return WPE_KEY_Super_L;
    case 126: return WPE_KEY_Super_R;
    case 127: return WPE_KEY_Menu;
    default: return WPE_KEY_VoidSymbol;
    }
}

void spatimesh_key(Engine *e, uint64_t id, int key, guint32 ch, int code, int pressed, int modifiers, const char *text) {
    Tab *t = find_shown(e, id);
    if (!t)
        return;
    /* What it typed, where it typed something: the key alone is its unshifted letter, and WebKit
     * types the key's own character (Shift-S would type "s"). */
    if (key == 15 && text && *text)
        ch = g_utf8_get_char(text);
    /* The hardware code as X and evdev number keys (Linux's plus 8), which WPE's keymap reads. */
    deliver(t, wpe_event_keyboard_new(pressed ? WPE_EVENT_KEYBOARD_KEY_DOWN : WPE_EVENT_KEYBOARD_KEY_UP, WPE_VIEW(t->wpe), WPE_INPUT_SOURCE_KEYBOARD,
        now_ms(), modifiers_of(modifiers), (guint)(code + 8), keyval_of(key, ch, code)));
}

void spatimesh_focus(Engine *e, uint64_t id, int focused) {
    Tab *t = find_shown(e, id);
    if (!t)
        return;
    if (focused)
        wpe_view_focus_in(WPE_VIEW(t->wpe));
    else
        wpe_view_focus_out(WPE_VIEW(t->wpe));
}

/* ---- <select> lists and script dialogs ----------------------------------------------------- */

/* The `index`th line of the tab's open list: its label (owned by WebKit) and flags (1 enabled,
 * 2 selected, 4 a group's heading). NULL past the end. */
const char *spatimesh_choice(Engine *e, uint64_t id, int index, int *flags) {
    Tab *t = find(e, id);
    if (!t || !t->menu || index < 0 || (guint)index >= webkit_option_menu_get_n_items(t->menu))
        return NULL;
    WebKitOptionMenuItem *item = webkit_option_menu_get_item(t->menu, (guint)index);
    *flags = (webkit_option_menu_item_is_enabled(item) ? 1 : 0) | (webkit_option_menu_item_is_selected(item) ? 2 : 0)
        | (webkit_option_menu_item_is_group_label(item) ? 4 : 0);
    return webkit_option_menu_item_get_label(item);
}

void spatimesh_choose(Engine *e, uint64_t id, int index) {
    Tab *t = find(e, id);
    if (!t || !t->menu)
        return;
    if (index >= 0 && (guint)index < webkit_option_menu_get_n_items(t->menu))
        webkit_option_menu_activate_item(t->menu, (guint)index);
    webkit_option_menu_close(t->menu);
    g_object_unref(t->menu);
    t->menu = NULL;
}

void spatimesh_answer(Engine *e, uint64_t id, int accepted, const char *text) {
    Tab *t = find(e, id);
    if (!t || !t->dialog)
        return;
    switch (webkit_script_dialog_get_dialog_type(t->dialog)) {
    case WEBKIT_SCRIPT_DIALOG_CONFIRM:
    case WEBKIT_SCRIPT_DIALOG_BEFORE_UNLOAD_CONFIRM:
        webkit_script_dialog_confirm_set_confirmed(t->dialog, accepted ? TRUE : FALSE);
        break;
    case WEBKIT_SCRIPT_DIALOG_PROMPT:
        if (accepted)
            webkit_script_dialog_prompt_set_text(t->dialog, text ? text : "");
        break;
    default:
        break;
    }
    webkit_script_dialog_close(t->dialog);
    webkit_script_dialog_unref(t->dialog);
    t->dialog = NULL;
}

static gboolean on_timeout(gpointer data) {
    ((Engine *)data)->timed_out = 1;
    return G_SOURCE_REMOVE;
}

static gboolean any_fresh(Engine *e) {
    GHashTableIter it;
    gpointer key, value;
    g_hash_table_iter_init(&it, e->tabs);
    while (g_hash_table_iter_next(&it, &key, &value))
        if (((Tab *)value)->fresh)
            return TRUE;
    return FALSE;
}

/* Let GTK and WebKit work for at most `timeout_ms`, returning as soon as there is something
 * to tell, to draw, or to read from the window. */
void spatimesh_pump(Engine *e, int timeout_ms) {
    GHashTableIter it;
    gpointer key, value;
    g_hash_table_iter_init(&it, e->tabs);
    while (g_hash_table_iter_next(&it, &key, &value))
        ((Tab *)value)->fresh = 0;
    e->woken = 0;
    e->timed_out = 0;
    while (!e->woken && g_main_context_iteration(NULL, FALSE))
        ;
    if (e->woken || timeout_ms <= 0 || !g_queue_is_empty(e->events) || any_fresh(e))
        return;
    guint timer = g_timeout_add((guint)timeout_ms, on_timeout, e);
    while (!e->woken && !e->timed_out && g_queue_is_empty(e->events) && !any_fresh(e))
        g_main_context_iteration(NULL, TRUE);
    if (!e->timed_out)
        g_source_remove(timer);
}

/* The next thing that happened, into `out`; its strings live until the next call. */
int spatimesh_next_event(Engine *e, MeshEvent *out) {
    g_free(e->current.text);
    g_free(e->current.text2);
    memset(&e->current, 0, sizeof e->current);
    MeshEvent *ev = g_queue_pop_head(e->events);
    if (!ev)
        return 0;
    e->current = *ev;
    g_free(ev);
    *out = e->current;
    return 1;
}

/* Whether a tab's picture changed and has not been read, and what changed. */
int spatimesh_damage(Engine *e, uint64_t id, int *x, int *y, int *w, int *h) {
    Tab *t = find_shown(e, id);
    if (!t || !t->wpe->damaged || !t->wpe->pixels)
        return 0;
    MeshView *v = t->wpe;
    *x = MAX(v->dx0, 0);
    *y = MAX(v->dy0, 0);
    *w = MIN(v->dx1, v->width) - *x;
    *h = MIN(v->dy1, v->height) - *y;
    if (*w <= 0 || *h <= 0) {
        v->damaged = 0;
        return 0;
    }
    return 1;
}

/* The tab's newest picture on the GPU, by its IOSurface's number, once: from then on the
 * window holds it, until spatimesh_release_surface. (A Mac's; elsewhere there are none.) */
int spatimesh_surface(Engine *e, uint64_t id, uint32_t *surface, int *width, int *height) {
#ifdef WPE_PLATFORM_BUFFER_IOSURFACE
    Tab *t = find_shown(e, id);
    if (!t || !t->wpe->fresh_surface)
        return 0;
    WPEBuffer *buffer = t->wpe->fresh_surface;
    t->wpe->fresh_surface = NULL;
    g_ptr_array_add(t->wpe->held_surfaces, buffer);
    *surface = wpe_buffer_iosurface_get_id(WPE_BUFFER_IOSURFACE(buffer));
    *width = wpe_buffer_get_width(buffer);
    *height = wpe_buffer_get_height(buffer);
    return 1;
#else
    (void)e; (void)id; (void)surface; (void)width; (void)height;
    return 0;
#endif
}

/* The window let go of a picture: WPE may draw into it again. */
void spatimesh_release_surface(Engine *e, uint64_t id, uint32_t surface) {
#ifdef WPE_PLATFORM_BUFFER_IOSURFACE
    Tab *t = find_shown(e, id);
    if (!t)
        return;
    GPtrArray *held = t->wpe->held_surfaces;
    for (guint i = 0; i < held->len; i++) {
        WPEBuffer *buffer = g_ptr_array_index(held, i);
        if (wpe_buffer_iosurface_get_id(WPE_BUFFER_IOSURFACE(buffer)) == surface) {
            wpe_view_buffer_released(WPE_VIEW(t->wpe), buffer);
            g_ptr_array_remove_index(held, i);
            return;
        }
    }
#else
    (void)e; (void)id; (void)surface;
#endif
}

/* The tab's picture, where it changed, into `pixels` (`width` by `height`, four bytes a pixel,
 * cairo's order -- which is WPE's ARGB8888 in memory), and the change forgotten. */
int spatimesh_copy(Engine *e, uint64_t id, unsigned char *pixels, int width, int height) {
    Tab *t = find_shown(e, id);
    if (!t || !t->wpe->damaged || !t->wpe->pixels)
        return 0;
    MeshView *v = t->wpe;
    gsize size;
    const unsigned char *from = g_bytes_get_data(v->pixels, &size);
    int x0 = MAX(v->dx0, 0), y0 = MAX(v->dy0, 0);
    int x1 = MIN(MIN(v->dx1, v->width), width), y1 = MIN(MIN(v->dy1, v->height), height);
    for (int y = y0; y < y1; y++) {
        if ((gsize)(y * v->stride + x1 * 4) > size)
            break;
        memcpy(pixels + (size_t)y * width * 4 + x0 * 4, from + (size_t)y * v->stride + x0 * 4, (size_t)(x1 - x0) * 4);
    }
    v->damaged = 0;
    return 1;
}

/* For tests: a script run in the tab's page. */
void spatimesh_run(Engine *e, uint64_t id, const char *script) {
    Tab *t = find_page(e, id);
    if (t)
        webkit_web_view_evaluate_javascript(t->view, script, -1, NULL, NULL, NULL, NULL, NULL);
}

/* The window a tab is in: where its sound is to come from. Its page's sound plays from its
 * own web process; placing it at the window needs the compositor's sink per window
 * (docs/spatimesh.md, "Sound"). Until then a tab sounds where SpatiMesh does. */
void spatimesh_set_window(Engine *e, uint64_t id, guint32 window) {
    (void)e;
    (void)id;
    (void)window;
}

/* ---- cookies (docs/spatimesh.md, "Logins and history on every machine") ------------------- */

#if WEBKIT_CHECK_VERSION(2, 42, 0)
static void got_cookies(GObject *source, GAsyncResult *result, gpointer data) {
    Engine *e = data;
    GList *all = webkit_cookie_manager_get_all_cookies_finish(WEBKIT_COOKIE_MANAGER(source), result, NULL);
    GString *lines = g_string_new(NULL);
    for (GList *l = all; l; l = l->next) {
        SoupCookie *c = l->data;
        GDateTime *expires = soup_cookie_get_expires(c);
        const char *same = "";
        switch (soup_cookie_get_same_site_policy(c)) {
        case SOUP_SAME_SITE_POLICY_LAX: same = "Lax"; break;
        case SOUP_SAME_SITE_POLICY_STRICT: same = "Strict"; break;
        case SOUP_SAME_SITE_POLICY_NONE: same = "None"; break;
        default: break;
        }
        g_string_append_printf(lines, "%s\t%s\t%s\t%s\t%lld\t%d\t%d\t%s\n", soup_cookie_get_name(c), soup_cookie_get_value(c),
                               soup_cookie_get_domain(c), soup_cookie_get_path(c), expires ? (long long)g_date_time_to_unix(expires) : -1LL,
                               soup_cookie_get_secure(c) ? 1 : 0, soup_cookie_get_http_only(c) ? 1 : 0, same);
    }
    g_list_free_full(all, (GDestroyNotify)soup_cookie_free);
    push(e, EV_COOKIES, 0, 0, 0, 0, lines->str, NULL);
    g_string_free(lines, TRUE);
}
#endif

void spatimesh_cookies_ask(Engine *e) {
#if WEBKIT_CHECK_VERSION(2, 42, 0)
    webkit_cookie_manager_get_all_cookies(e->cookies, NULL, got_cookies, e);
#else
    (void)e;
#endif
}

/* A cookie from a line of `Cookie::line`. */
static SoupCookie *cookie_of(const char *line) {
    char **f = g_strsplit(line, "\t", 9);
    SoupCookie *c = NULL;
    if (g_strv_length(f) >= 7 && f[0][0] && f[2][0]) {
        c = soup_cookie_new(f[0], f[1], f[2], f[3][0] ? f[3] : "/", -1);
        long long expires = g_ascii_strtoll(f[4], NULL, 10);
        if (expires >= 0) {
            GDateTime *when = g_date_time_new_from_unix_utc(expires);
            soup_cookie_set_expires(c, when);
            g_date_time_unref(when);
        }
        soup_cookie_set_secure(c, strcmp(f[5], "1") == 0);
        soup_cookie_set_http_only(c, strcmp(f[6], "1") == 0);
        if (f[7]) {
            if (strcmp(f[7], "Strict") == 0)
                soup_cookie_set_same_site_policy(c, SOUP_SAME_SITE_POLICY_STRICT);
            else if (strcmp(f[7], "Lax") == 0)
                soup_cookie_set_same_site_policy(c, SOUP_SAME_SITE_POLICY_LAX);
            else if (strcmp(f[7], "None") == 0)
                soup_cookie_set_same_site_policy(c, SOUP_SAME_SITE_POLICY_NONE);
        }
    }
    g_strfreev(f);
    return c;
}

static void each_cookie(Engine *e, const char *lines, int add) {
    char **all = g_strsplit(lines, "\n", -1);
    for (char **line = all; *line; line++) {
        SoupCookie *c = cookie_of(*line);
        if (!c)
            continue;
        if (add)
            webkit_cookie_manager_add_cookie(e->cookies, c, NULL, NULL, NULL);
        else
            webkit_cookie_manager_delete_cookie(e->cookies, c, NULL, NULL, NULL);
        soup_cookie_free(c);
    }
    g_strfreev(all);
}

void spatimesh_cookies_set(Engine *e, const char *lines) {
    each_cookie(e, lines, 1);
}

void spatimesh_cookies_remove(Engine *e, const char *lines) {
    each_cookie(e, lines, 0);
}

/* WPE has no way to be told of links visited elsewhere: they look visited once visited here. */
void spatimesh_visited(Engine *e, const char *urls) {
    (void)e;
    (void)urls;
}
