/* WebKitGTK behind the few calls SpatiMesh's engine host makes: the C interface every engine's
 * native half provides (crates/spatimesh-shim).
 *
 * Every tab is a WebKitWebView in a GtkOffscreenWindow: drawn by WebKit as on a screen, shown on
 * none. Its picture is read from the offscreen window's surface after each draw, only where it
 * changed (the window's damage); the wearer's input is given to it as GDK events made here.
 * Whatever WebKitGTK would otherwise answer with a window of its own -- a <select>'s list, a
 * script's alert, a file chooser, a context menu -- is answered here, or handed to SpatiMesh's
 * window to draw, so that no stray window ever appears in the room.
 *
 * What happens to pages is queued as events, popped by the Rust side after each pump. */

#include <gtk/gtk.h>
#include <glib-unix.h>
#ifdef GDK_WINDOWING_X11
#include <gdk/gdkx.h>
#endif
#include <webkit2/webkit2.h>
#include <libsoup/soup.h>
#include <stdint.h>
#include <string.h>

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
    /* 17 is WPE's sound name, which WebKitGTK has none of. */
    EV_CONTEXT_MENU = 18,
    EV_HOVERED,
    EV_FOUND,
    EV_CHOOSE_FILES,
    EV_DOWNLOAD,
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

typedef struct {
    uint64_t id;
    struct Engine *engine;
    GtkWidget *window;
    WebKitWebView *view;
    int width, height;
    /* What changed since the picture was last read, as one rectangle. */
    int damaged, dx0, dy0, dx1, dy1;
    /* Changed since the last pump: a reason for the pump not to wait. */
    int fresh;
    WebKitOptionMenu *menu;
    WebKitScriptDialog *dialog;
    int entered;
    guint buttons;
    int cursor;
    int loading;
    /* The extensions' worlds a message handler is registered in, for this tab's view. */
    GPtrArray *worlds;
    int main_handler;
    /* The media script's way out is registered once a tab has one. */
    int media_handler;
    /* A page's <input type=file>, waiting for the window's answer (spatimesh_files). */
    WebKitFileChooserRequest *files;
    /* What is being found in the page, and the link under the pointer as last said. */
    char *finding;
    char *hovered;
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
    /* Downloads by their number (Fetch), and the number the next one the engine begins by
     * itself gets: from 1 << 30, below which are the window's own (spatimesh_download). */
    GHashTable *downloads;
    int download_next;
    struct Fetch *asking;
} Engine;

/* A download, as the window knows it. */
typedef struct Fetch {
    Engine *engine;
    int id;
    uint64_t tab;
    /* The file the window asked for it to be written to, or NULL for the wearer's Downloads. */
    char *to;
    char *file;
    WebKitDownload *download;
    gint64 told_at;
    int done;
} Fetch;

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
    on_progress(view, NULL, t);
    if (event == WEBKIT_LOAD_COMMITTED || event == WEBKIT_LOAD_FINISHED)
        history(t);
}

static void on_audio(WebKitWebView *view, GParamSpec *p, Tab *t) {
    push(t->engine, EV_SOUND, t->id, 0, webkit_web_view_is_playing_audio(view), 0, NULL, NULL);
}

static void on_target(WebKitWebView *view, WebKitHitTestResult *hit, guint modifiers, Tab *t) {
    /* Cursors by what is under the pointer: WebKitGTK sets the cursor on a window that is on
     * no screen, where nobody can read it back by name. */
    int cursor = 0;
    if (webkit_hit_test_result_context_is_link(hit))
        cursor = 1;
    else if (webkit_hit_test_result_context_is_editable(hit))
        cursor = 2;
    if (cursor != t->cursor) {
        t->cursor = cursor;
        push(t->engine, EV_CURSOR, t->id, 0, cursor, 0, NULL, NULL);
    }
    /* The link under the pointer, said at the foot of the page. */
    const char *link = webkit_hit_test_result_context_is_link(hit) ? webkit_hit_test_result_get_link_uri(hit) : NULL;
    if (g_strcmp0(link ? link : "", t->hovered ? t->hovered : "") != 0) {
        g_free(t->hovered);
        t->hovered = g_strdup(link ? link : "");
        push(t->engine, EV_HOVERED, t->id, 0, 0, 0, t->hovered, NULL);
    }
}

static gboolean on_policy(WebKitWebView *view, WebKitPolicyDecision *decision, WebKitPolicyDecisionType type, Tab *t) {
    if (type == WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION || type == WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION) {
        WebKitNavigationAction *action = webkit_navigation_policy_decision_get_navigation_action(WEBKIT_NAVIGATION_POLICY_DECISION(decision));
        guint button = webkit_navigation_action_get_mouse_button(action);
        guint modifiers = webkit_navigation_action_get_modifiers(action);
        gboolean link = webkit_navigation_action_get_navigation_type(action) == WEBKIT_NAVIGATION_TYPE_LINK_CLICKED;
        gboolean background = button == 2 || (modifiers & (GDK_CONTROL_MASK | GDK_SUPER_MASK | GDK_META_MASK));
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
static GtkWidget *on_create(WebKitWebView *view, WebKitNavigationAction *action, Tab *t) {
    Engine *e = t->engine;
    GtkWidget *opened = webkit_web_view_new_with_related_view(view);
    webkit_web_view_set_settings(WEBKIT_WEB_VIEW(opened), e->settings);
    unsigned number = ++e->opened_next;
    g_hash_table_insert(e->opened, GUINT_TO_POINTER(number), g_object_ref_sink(opened));
    char *address = g_strdup_printf(OPENED_PREFIX "%u", number);
    push(e, EV_OPEN_TAB, t->id, 0, 0, 0, address, NULL);
    g_free(address);
    return opened;
}

static void on_close(WebKitWebView *view, Tab *t) {
    push(t->engine, EV_CLOSE_TAB, t->id, 0, 0, 0, NULL, NULL);
}

static void on_terminated(WebKitWebView *view, WebKitWebProcessTerminationReason reason, Tab *t) {
    const char *why = reason == WEBKIT_WEB_PROCESS_CRASHED ? "The page's process crashed"
        : reason == WEBKIT_WEB_PROCESS_EXCEEDED_MEMORY_LIMIT ? "The page used too much memory"
        : "The page's process was ended";
    push(t->engine, EV_CRASHED, t->id, 0, 0, 0, why, NULL);
}

static gboolean on_failed(WebKitWebView *view, WebKitLoadEvent event, const char *uri, GError *error, Tab *t) {
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

static gboolean on_option_menu(WebKitWebView *view, WebKitOptionMenu *menu, GdkEvent *event, GdkRectangle *rect, Tab *t) {
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

/* A page's <input type=file>: the window asks the wearer, with a list of its own, and answers
 * with spatimesh_files. */
static gboolean on_file_chooser(WebKitWebView *view, WebKitFileChooserRequest *request, Tab *t) {
    if (t->files) {
        webkit_file_chooser_request_cancel(t->files);
        g_object_unref(t->files);
    }
    t->files = g_object_ref(request);
    const gchar *const *types = webkit_file_chooser_request_get_mime_types(request);
    char *accept = types ? g_strjoinv("\n", (gchar **)types) : g_strdup("");
    push(t->engine, EV_CHOOSE_FILES, t->id, 0, webkit_file_chooser_request_get_select_multiple(request), 0, accept, NULL);
    g_free(accept);
    return TRUE;
}

/* One line of a hit's description, with nothing in it that would end the line. */
static void append_line(GString *lines, const char *text) {
    for (const char *c = text ? text : ""; *c; c++)
        g_string_append_c(lines, *c == '\n' || *c == '\r' ? ' ' : *c);
    g_string_append_c(lines, '\n');
}

/* What is under the pointer, for the window's own menu: flags, and the link, its words, the
 * image and the media, a line each (spatimesh::engine::Hit::from_native). */
static void push_hit(Tab *t, WebKitHitTestResult *hit) {
    int flags = 0;
    if (webkit_hit_test_result_context_is_link(hit))
        flags |= 1;
    if (webkit_hit_test_result_context_is_image(hit))
        flags |= 2;
    if (webkit_hit_test_result_context_is_media(hit))
        flags |= 4;
    if (webkit_hit_test_result_context_is_editable(hit))
        flags |= 8;
    if (webkit_hit_test_result_context_is_selection(hit))
        flags |= 16;
    GString *lines = g_string_new(NULL);
    append_line(lines, webkit_hit_test_result_get_link_uri(hit));
    const char *label = webkit_hit_test_result_get_link_label(hit);
    append_line(lines, label ? label : webkit_hit_test_result_get_link_title(hit));
    append_line(lines, webkit_hit_test_result_get_image_uri(hit));
    append_line(lines, webkit_hit_test_result_get_media_uri(hit));
    push(t->engine, EV_CONTEXT_MENU, t->id, 0, flags, 0, lines->str, NULL);
    g_string_free(lines, TRUE);
}

static gboolean on_colour_chooser(WebKitWebView *view, WebKitColorChooserRequest *request, Tab *t) {
    webkit_color_chooser_request_cancel(request);
    return TRUE;
}

/* The page's menu is the window's to draw: WebKitGTK's own would be a window of its own. */
static gboolean on_context_menu(WebKitWebView *view, WebKitContextMenu *menu, GdkEvent *event, WebKitHitTestResult *hit, Tab *t) {
    push_hit(t, hit);
    return TRUE;
}

static void on_found(WebKitFindController *finder, guint matches, Tab *t) {
    push(t->engine, EV_FOUND, t->id, 0, (int)MIN(matches, G_MAXINT), 0, NULL, NULL);
}

static void on_not_found(WebKitFindController *finder, Tab *t) {
    push(t->engine, EV_FOUND, t->id, 0, 0, 0, NULL, NULL);
}

static gboolean on_permission(WebKitWebView *view, WebKitPermissionRequest *request, Tab *t) {
    webkit_permission_request_deny(request);
    return TRUE;
}

static gboolean on_swallow(WebKitWebView *view, gpointer what, Tab *t) {
    return TRUE;
}

static gboolean on_damage(GtkWidget *widget, GdkEventExpose *event, Tab *t) {
    GdkRectangle r = event->area;
    if (!t->damaged) {
        t->dx0 = r.x;
        t->dy0 = r.y;
        t->dx1 = r.x + r.width;
        t->dy1 = r.y + r.height;
    } else {
        t->dx0 = MIN(t->dx0, r.x);
        t->dy0 = MIN(t->dy0, r.y);
        t->dx1 = MAX(t->dx1, r.x + r.width);
        t->dy1 = MAX(t->dy1, r.y + r.height);
    }
    t->damaged = 1;
    t->fresh = 1;
    return FALSE;
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
    if (t->files) {
        webkit_file_chooser_request_cancel(t->files);
        g_object_unref(t->files);
    }
    g_free(t->finding);
    g_free(t->hovered);
    gtk_widget_destroy(t->window);
    if (t->worlds)
        g_ptr_array_unref(t->worlds);
    g_free(t);
}

const char *spatimesh_version(void) {
    static char version[64];
    g_snprintf(version, sizeof version, "WebKitGTK %u.%u.%u", webkit_get_major_version(), webkit_get_minor_version(), webkit_get_micro_version());
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
static void on_media_message(WebKitUserContentManager *manager, WebKitJavascriptResult *result, Tab *t) {
    JSCValue *value = webkit_javascript_result_get_js_value(result);
    char *text = jsc_value_to_string(value);
    push(t->engine, EV_MEDIA, t->id, 0, 0, 0, text, NULL);
    g_free(text);
}

static void on_ext_message(WebKitUserContentManager *manager, WebKitJavascriptResult *result, Tab *t) {
    JSCValue *value = webkit_javascript_result_get_js_value(result);
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
            t->media_handler = webkit_user_content_manager_register_script_message_handler(m, "spatimeshMedia");
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
        webkit_user_content_manager_unregister_script_message_handler_in_world(m, "spatimeshExt", g_ptr_array_index(t->worlds, i));
    g_ptr_array_set_size(t->worlds, 0);
    if (t->main_handler) {
        webkit_user_content_manager_unregister_script_message_handler(m, "spatimeshExt");
        t->main_handler = 0;
    }
    if (!e->exts || e->exts->len == 0)
        return;
    t->main_handler = webkit_user_content_manager_register_script_message_handler(m, "spatimeshExt");
    for (guint i = 0; i < e->exts->len; i++) {
        Ext *x = g_ptr_array_index(e->exts, i);
        if (webkit_user_content_manager_register_script_message_handler_in_world(m, "spatimeshExt", x->world))
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
    g_free(e->page_script);
    e->page_script = code && *code ? g_strdup(code) : NULL;
    GHashTableIter it;
    gpointer key, value;
    g_hash_table_iter_init(&it, e->tabs);
    while (g_hash_table_iter_next(&it, &key, &value))
        apply_extensions(value);
}

/* Code run in a tab's top frame, in `world` ("" the page's own), the extension's API set up
 * there first if `boot`. Only the top frame here: WebKitGTK runs scripts from outside the
 * page in no other (`all_frames` is for engines that can). */
void spatimesh_run_script(Engine *e, uint64_t id, const char *world, int all_frames, const char *code, uint64_t request, int boot) {
    Tab *t = find(e, id);
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

/* ---- downloads ---------------------------------------------------------------------------- */

static uint64_t tab_of_view(Engine *e, WebKitWebView *view) {
    GHashTableIter it;
    gpointer key, value;
    g_hash_table_iter_init(&it, e->tabs);
    while (g_hash_table_iter_next(&it, &key, &value))
        if (((Tab *)value)->view == view)
            return ((Tab *)value)->id;
    return 0;
}

/* The wearer's Downloads folder, and a name in it nothing has yet: "name (2).ext". */
static char *free_name_in_downloads(const char *suggested) {
    const char *dir = g_get_user_special_dir(G_USER_DIRECTORY_DOWNLOAD);
    char *fallback = g_build_filename(g_get_home_dir(), "Downloads", NULL);
    if (!dir || !*dir)
        dir = fallback;
    g_mkdir_with_parents(dir, 0755);
    char *base = g_path_get_basename(suggested && *suggested ? suggested : "download");
    if (!*base || strcmp(base, ".") == 0 || strcmp(base, "/") == 0) {
        g_free(base);
        base = g_strdup("download");
    }
    char *path = g_build_filename(dir, base, NULL);
    const char *dot = strrchr(base, '.');
    size_t stem = dot && dot != base ? (size_t)(dot - base) : strlen(base);
    for (int n = 2; g_file_test(path, G_FILE_TEST_EXISTS) && n < 10000; n++) {
        g_free(path);
        char *name = g_strdup_printf("%.*s (%d)%s", (int)stem, base, n, base + stem);
        path = g_build_filename(dir, name, NULL);
        g_free(name);
    }
    g_free(base);
    g_free(fallback);
    return path;
}

static gboolean on_download_destination(WebKitDownload *download, const char *suggested, Fetch *f) {
    char *path = f->to && *f->to ? g_strdup(f->to) : free_name_in_downloads(suggested);
    char *folder = g_path_get_dirname(path);
    g_mkdir_with_parents(folder, 0755);
    g_free(folder);
    g_free(f->file);
    f->file = path;
    char *uri = g_filename_to_uri(path, NULL, NULL);
    webkit_download_set_allow_overwrite(download, f->to && *f->to);
    webkit_download_set_destination(download, uri);
    g_free(uri);
    WebKitURIRequest *request = webkit_download_get_request(download);
    const char *from = request ? webkit_uri_request_get_uri(request) : "";
    MeshEvent *ev = g_new0(MeshEvent, 1);
    ev->kind = EV_DOWNLOAD;
    ev->tab = f->tab;
    ev->x = f->id;
    ev->a = 0;
    ev->text = g_strdup(from ? from : "");
    ev->text2 = g_strdup(f->file);
    g_queue_push_tail(f->engine->events, ev);
    return TRUE;
}

static void push_download(Fetch *f, int stage, double number, const char *text) {
    MeshEvent *ev = g_new0(MeshEvent, 1);
    ev->kind = EV_DOWNLOAD;
    ev->tab = f->tab;
    ev->x = f->id;
    ev->a = stage;
    ev->number = number;
    ev->text = g_strdup(text ? text : "");
    ev->text2 = g_strdup(f->file ? f->file : "");
    g_queue_push_tail(f->engine->events, ev);
}

static void on_download_data(WebKitDownload *download, guint64 length, Fetch *f) {
    /* Four times a second is enough to draw it by. */
    gint64 now = g_get_monotonic_time();
    if (now - f->told_at < 250000)
        return;
    f->told_at = now;
    WebKitURIResponse *response = webkit_download_get_response(download);
    char *total = g_strdup_printf("%" G_GUINT64_FORMAT, response ? webkit_uri_response_get_content_length(response) : 0);
    push_download(f, 1, (double)webkit_download_get_received_data_length(download), total);
    g_free(total);
}

static void forget_download(Fetch *f) {
    g_hash_table_remove(f->engine->downloads, GINT_TO_POINTER(f->id));
    g_signal_handlers_disconnect_by_data(f->download, f);
    g_object_unref(f->download);
    g_free(f->to);
    g_free(f->file);
    g_free(f);
}

static void on_download_failed(WebKitDownload *download, GError *error, Fetch *f) {
    if (f->done)
        return;
    f->done = 1;
    gboolean cancelled = g_error_matches(error, WEBKIT_DOWNLOAD_ERROR, WEBKIT_DOWNLOAD_ERROR_CANCELLED_BY_USER);
    push_download(f, 3, 0, cancelled ? "Cancelled" : error ? error->message : "it could not be downloaded");
}

/* After "failed" too, which said so already. */
static void on_download_finished(WebKitDownload *download, Fetch *f) {
    if (!f->done) {
        f->done = 1;
        push_download(f, 2, 0, NULL);
    }
    forget_download(f);
}

static void watch_download(Fetch *f, WebKitDownload *download) {
    f->download = g_object_ref(download);
    g_hash_table_insert(f->engine->downloads, GINT_TO_POINTER(f->id), f);
    g_signal_connect(download, "decide-destination", G_CALLBACK(on_download_destination), f);
    g_signal_connect(download, "received-data", G_CALLBACK(on_download_data), f);
    g_signal_connect(download, "failed", G_CALLBACK(on_download_failed), f);
    g_signal_connect(download, "finished", G_CALLBACK(on_download_finished), f);
}

/* Every download, the window's own (being asked for: e->asking) and those a page began. */
static void on_download_started(WebKitWebContext *context, WebKitDownload *download, Engine *e) {
    if (g_object_get_data(G_OBJECT(download), "spatimesh"))
        return;
    g_object_set_data(G_OBJECT(download), "spatimesh", GINT_TO_POINTER(1));
    Fetch *f = e->asking;
    e->asking = NULL;
    if (!f) {
        f = g_new0(Fetch, 1);
        f->engine = e;
        f->id = e->download_next++;
        WebKitWebView *view = webkit_download_get_web_view(download);
        f->tab = view ? tab_of_view(e, view) : 0;
    }
    watch_download(f, download);
}

/* Download `uri` as the tab would ask for it, into `to` (empty: the wearer's Downloads). */
void spatimesh_download(Engine *e, uint64_t id, int download, const char *uri, const char *to) {
    Tab *t = find(e, id);
    Fetch *f = g_new0(Fetch, 1);
    f->engine = e;
    f->id = download;
    f->tab = id;
    f->to = to && *to ? g_strdup(to) : NULL;
    e->asking = f;
    WebKitDownload *d = t ? webkit_web_view_download_uri(t->view, uri) : webkit_web_context_download_uri(e->context, uri);
    if (e->asking == f) {
        /* Not said to have started yet: it is this one. */
        e->asking = NULL;
        g_object_set_data(G_OBJECT(d), "spatimesh", GINT_TO_POINTER(1));
        watch_download(f, d);
    }
    g_object_unref(d);
}

void spatimesh_download_cancel(Engine *e, int download) {
    Fetch *f = g_hash_table_lookup(e->downloads, GINT_TO_POINTER(download));
    if (f)
        webkit_download_cancel(f->download);
}

/* ---- finding in a page, and files for it ------------------------------------------------ */

void spatimesh_find(Engine *e, uint64_t id, const char *text, int forward) {
    Tab *t = find(e, id);
    if (!t)
        return;
    WebKitFindController *finder = webkit_web_view_get_find_controller(t->view);
    if (!text || !*text) {
        webkit_find_controller_search_finish(finder);
        g_clear_pointer(&t->finding, g_free);
        return;
    }
    if (t->finding && strcmp(t->finding, text) == 0) {
        if (forward)
            webkit_find_controller_search_next(finder);
        else
            webkit_find_controller_search_previous(finder);
        return;
    }
    g_free(t->finding);
    t->finding = g_strdup(text);
    guint32 options = WEBKIT_FIND_OPTIONS_CASE_INSENSITIVE | WEBKIT_FIND_OPTIONS_WRAP_AROUND | (forward ? 0 : WEBKIT_FIND_OPTIONS_BACKWARDS);
    webkit_find_controller_search(finder, text, options, G_MAXUINT);
}

/* The files the wearer chose for the page's <input type=file>, a path a line; NULL for none. */
void spatimesh_files(Engine *e, uint64_t id, const char *paths) {
    Tab *t = find(e, id);
    if (!t || !t->files)
        return;
    if (paths && *paths) {
        char **files = g_strsplit(paths, "\n", -1);
        webkit_file_chooser_request_select_files(t->files, (const gchar *const *)files);
        g_strfreev(files);
    } else
        webkit_file_chooser_request_cancel(t->files);
    g_clear_object(&t->files);
}

/* A profile's engine: `data` and `cache` are its folders, or NULL for the private profile,
 * which keeps everything in memory. NULL if GTK cannot start (no display at all). */
Engine *spatimesh_new(const char *data, const char *cache) {
    if (!gtk_init_check(NULL, NULL))
        return NULL;
    Engine *e = g_new0(Engine, 1);
    WebKitWebsiteDataManager *manager;
    if (data && cache) {
        manager = webkit_website_data_manager_new("base-data-directory", data, "base-cache-directory", cache, NULL);
    } else {
        manager = webkit_website_data_manager_new_ephemeral();
    }
    e->context = webkit_web_context_new_with_website_data_manager(manager);
    WebKitCookieManager *cookies = webkit_website_data_manager_get_cookie_manager(manager);
    if (data) {
        char *file = g_build_filename(data, "cookies.sqlite", NULL);
        webkit_cookie_manager_set_persistent_storage(cookies, file, WEBKIT_COOKIE_PERSISTENT_STORAGE_SQLITE);
        g_free(file);
    }
    webkit_cookie_manager_set_accept_policy(cookies, WEBKIT_COOKIE_POLICY_ACCEPT_NO_THIRD_PARTY);
    e->cookies = g_object_ref(cookies);
    g_object_unref(manager);
    webkit_web_context_set_cache_model(e->context, WEBKIT_CACHE_MODEL_WEB_BROWSER);
    e->settings = webkit_settings_new();
    webkit_settings_set_enable_smooth_scrolling(e->settings, TRUE);
    webkit_settings_set_enable_back_forward_navigation_gestures(e->settings, FALSE);
    webkit_settings_set_javascript_can_open_windows_automatically(e->settings, FALSE);
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
    e->downloads = g_hash_table_new(g_direct_hash, g_direct_equal);
    e->download_next = 1 << 30;
    g_signal_connect(e->context, "download-started", G_CALLBACK(on_download_started), e);
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
    if (find(e, id))
        return 0;
    Tab *t = g_new0(Tab, 1);
    t->id = id;
    t->engine = e;
    t->width = MAX(width, 1);
    t->height = MAX(height, 1);
    t->window = gtk_offscreen_window_new();
    gtk_window_set_default_size(GTK_WINDOW(t->window), t->width, t->height);
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
    t->view = opened ? opened : WEBKIT_WEB_VIEW(g_object_new(WEBKIT_TYPE_WEB_VIEW, "web-context", e->context, "settings", e->settings, NULL));
    gtk_widget_set_size_request(GTK_WIDGET(t->view), t->width, t->height);
    gtk_container_add(GTK_CONTAINER(t->window), GTK_WIDGET(t->view));
    webkit_web_view_set_zoom_level(t->view, zoom > 0.1 ? zoom : 1.0);
    g_signal_connect(t->window, "damage-event", G_CALLBACK(on_damage), t);
    g_signal_connect(t->view, "notify::title", G_CALLBACK(on_title), t);
    g_signal_connect(t->view, "notify::uri", G_CALLBACK(on_uri), t);
    g_signal_connect(t->view, "notify::estimated-load-progress", G_CALLBACK(on_progress), t);
    g_signal_connect(t->view, "load-changed", G_CALLBACK(on_load_changed), t);
    g_signal_connect(t->view, "notify::is-playing-audio", G_CALLBACK(on_audio), t);
    g_signal_connect(t->view, "mouse-target-changed", G_CALLBACK(on_target), t);
    g_signal_connect(t->view, "decide-policy", G_CALLBACK(on_policy), t);
    g_signal_connect(t->view, "create", G_CALLBACK(on_create), t);
    g_signal_connect(t->view, "close", G_CALLBACK(on_close), t);
    g_signal_connect(t->view, "web-process-terminated", G_CALLBACK(on_terminated), t);
    g_signal_connect(t->view, "load-failed", G_CALLBACK(on_failed), t);
    g_signal_connect(t->view, "load-failed-with-tls-errors", G_CALLBACK(on_tls), t);
    g_signal_connect(t->view, "show-option-menu", G_CALLBACK(on_option_menu), t);
    g_signal_connect(t->view, "script-dialog", G_CALLBACK(on_dialog), t);
    g_signal_connect(t->view, "run-file-chooser", G_CALLBACK(on_file_chooser), t);
    g_signal_connect(t->view, "run-color-chooser", G_CALLBACK(on_colour_chooser), t);
    g_signal_connect(t->view, "context-menu", G_CALLBACK(on_context_menu), t);
    g_signal_connect(t->view, "permission-request", G_CALLBACK(on_permission), t);
    g_signal_connect(t->view, "show-notification", G_CALLBACK(on_swallow), t);
    g_signal_connect(t->view, "print", G_CALLBACK(on_swallow), t);
    WebKitFindController *finder = webkit_web_view_get_find_controller(t->view);
    g_signal_connect(finder, "found-text", G_CALLBACK(on_found), t);
    g_signal_connect(finder, "failed-to-find-text", G_CALLBACK(on_not_found), t);
    gtk_widget_show_all(t->window);
    gtk_widget_grab_focus(GTK_WIDGET(t->view));
    uint64_t *key = g_new(uint64_t, 1);
    *key = id;
    g_hash_table_insert(e->tabs, key, t);
    apply_extensions(t);
    apply_filters(t);
    if (opened) {
        /* Loading already, as its opener asked: the window is told where it is. */
        on_uri(t->view, NULL, t);
        on_title(t->view, NULL, t);
        /* Ours, now that the window holds it. */
        g_object_unref(opened);
    } else
        webkit_web_view_load_uri(t->view, uri && *uri ? uri : "about:blank");
    return 1;
}

void spatimesh_close(Engine *e, uint64_t id) {
    g_hash_table_remove(e->tabs, &id);
}

void spatimesh_load(Engine *e, uint64_t id, const char *uri) {
    Tab *t = find(e, id);
    if (t)
        webkit_web_view_load_uri(t->view, uri);
}

void spatimesh_back(Engine *e, uint64_t id) {
    Tab *t = find(e, id);
    if (t)
        webkit_web_view_go_back(t->view);
}

void spatimesh_forward(Engine *e, uint64_t id) {
    Tab *t = find(e, id);
    if (t)
        webkit_web_view_go_forward(t->view);
}

void spatimesh_reload(Engine *e, uint64_t id) {
    Tab *t = find(e, id);
    if (t)
        webkit_web_view_reload(t->view);
}

void spatimesh_stop(Engine *e, uint64_t id) {
    Tab *t = find(e, id);
    if (t)
        webkit_web_view_stop_loading(t->view);
}

void spatimesh_resize(Engine *e, uint64_t id, int width, int height, double zoom) {
    Tab *t = find(e, id);
    if (!t)
        return;
    t->width = MAX(width, 1);
    t->height = MAX(height, 1);
    gtk_widget_set_size_request(GTK_WIDGET(t->view), t->width, t->height);
    gtk_window_resize(GTK_WINDOW(t->window), t->width, t->height);
    if (zoom > 0.1 && webkit_web_view_get_zoom_level(t->view) != zoom)
        webkit_web_view_set_zoom_level(t->view, zoom);
}

void spatimesh_mute(Engine *e, uint64_t id, int muted) {
    Tab *t = find(e, id);
    if (t)
        webkit_web_view_set_is_muted(t->view, muted);
}

/* ---- input ---------------------------------------------------------------------------------- */

static guint state_of(int modifiers) {
    guint state = 0;
    if (modifiers & 1) state |= GDK_SHIFT_MASK;
    if (modifiers & 2) state |= GDK_CONTROL_MASK;
    if (modifiers & 4) state |= GDK_MOD1_MASK;
    if (modifiers & 8) state |= GDK_SUPER_MASK | GDK_MOD4_MASK;
    return state;
}

static GdkDevice *device(gboolean keyboard) {
    GdkSeat *seat = gdk_display_get_default_seat(gdk_display_get_default());
    if (!seat)
        return NULL;
    return keyboard ? gdk_seat_get_keyboard(seat) : gdk_seat_get_pointer(seat);
}

static GdkWindow *page_window(Tab *t) {
    GdkWindow *w = gtk_widget_get_window(GTK_WIDGET(t->view));
    return w ? w : gtk_widget_get_window(t->window);
}

static void deliver(GdkEvent *ev, gboolean keyboard) {
    GdkDevice *d = device(keyboard);
    if (d) {
        gdk_event_set_device(ev, d);
        gdk_event_set_source_device(ev, d);
    }
#ifdef GDK_WINDOWING_X11
    /* On X, GTK answers some events by asking the server about the window they came in -- the
     * keys held, where the pointer is -- and an offscreen window has no X window to ask about.
     * The server's "no such window" would end the program; the answer is not needed. */
    GdkDisplay *display = gdk_display_get_default();
    gboolean x11 = GDK_IS_X11_DISPLAY(display);
    if (x11)
        gdk_x11_display_error_trap_push(display);
#endif
    gtk_main_do_event(ev);
    gdk_event_free(ev);
#ifdef GDK_WINDOWING_X11
    if (x11)
        gdk_x11_display_error_trap_pop_ignored(display);
#endif
}

static void cross(Tab *t, double x, double y, gboolean in) {
    GdkWindow *w = page_window(t);
    if (!w)
        return;
    GdkEvent *ev = gdk_event_new(in ? GDK_ENTER_NOTIFY : GDK_LEAVE_NOTIFY);
    ev->crossing.window = g_object_ref(w);
    ev->crossing.send_event = TRUE;
    ev->crossing.time = now_ms();
    ev->crossing.x = x;
    ev->crossing.y = y;
    ev->crossing.x_root = x;
    ev->crossing.y_root = y;
    ev->crossing.mode = GDK_CROSSING_NORMAL;
    ev->crossing.detail = GDK_NOTIFY_ANCESTOR;
    ev->crossing.focus = TRUE;
    deliver(ev, FALSE);
}

static guint button_mask(int button) {
    switch (button) {
    case 1: return GDK_BUTTON1_MASK;
    case 2: return GDK_BUTTON2_MASK;
    case 3: return GDK_BUTTON3_MASK;
    default: return 0;
    }
}

/* `button`: 0 none (a move), 1 left, 2 middle, 3 right, 4 back, 5 forward. */
void spatimesh_pointer(Engine *e, uint64_t id, double x, double y, int button, int pressed, int modifiers) {
    Tab *t = find(e, id);
    if (!t)
        return;
    GdkWindow *w = page_window(t);
    if (!w)
        return;
    if (!t->entered) {
        cross(t, x, y, TRUE);
        t->entered = 1;
    }
    if (button == 0) {
        GdkEvent *ev = gdk_event_new(GDK_MOTION_NOTIFY);
        ev->motion.window = g_object_ref(w);
        ev->motion.send_event = TRUE;
        ev->motion.time = now_ms();
        ev->motion.x = x;
        ev->motion.y = y;
        ev->motion.x_root = x;
        ev->motion.y_root = y;
        ev->motion.state = state_of(modifiers) | t->buttons;
        deliver(ev, FALSE);
        return;
    }
    /* GDK numbers back and forward 8 and 9. */
    guint gdk_button = button == 4 ? 8 : button == 5 ? 9 : (guint)button;
    GdkEvent *ev = gdk_event_new(pressed ? GDK_BUTTON_PRESS : GDK_BUTTON_RELEASE);
    ev->button.window = g_object_ref(w);
    ev->button.send_event = TRUE;
    ev->button.time = now_ms();
    ev->button.x = x;
    ev->button.y = y;
    ev->button.x_root = x;
    ev->button.y_root = y;
    ev->button.button = gdk_button;
    /* A press is told with the buttons held before it; a release with them still held. */
    ev->button.state = state_of(modifiers) | t->buttons;
    if (pressed)
        t->buttons |= button_mask(button);
    else
        t->buttons &= ~button_mask(button);
    deliver(ev, FALSE);
}

void spatimesh_leave(Engine *e, uint64_t id) {
    Tab *t = find(e, id);
    if (t && t->entered) {
        cross(t, -1, -1, FALSE);
        t->entered = 0;
        t->buttons = 0;
    }
}

void spatimesh_scroll(Engine *e, uint64_t id, double x, double y, double dx, double dy, int modifiers) {
    Tab *t = find(e, id);
    if (!t)
        return;
    GdkWindow *w = page_window(t);
    if (!w)
        return;
    if (!t->entered) {
        cross(t, x, y, TRUE);
        t->entered = 1;
    }
    GdkEvent *ev = gdk_event_new(GDK_SCROLL);
    ev->scroll.window = g_object_ref(w);
    ev->scroll.send_event = TRUE;
    ev->scroll.time = now_ms();
    ev->scroll.x = x;
    ev->scroll.y = y;
    ev->scroll.x_root = x;
    ev->scroll.y_root = y;
    ev->scroll.state = state_of(modifiers);
    ev->scroll.direction = GDK_SCROLL_SMOOTH;
    /* WebKit moves a page 40 pixels for each unit of a smooth scroll. */
    ev->scroll.delta_x = dx / 40.0;
    ev->scroll.delta_y = dy / 40.0;
    deliver(ev, FALSE);
}

/* A key's GDK name from SpatiMesh's: `key` is the wire's number (1 Enter ... 15 a character,
 * `ch`), `code` Linux's number for the key, for the ones the wire has no name for. */
static guint keyval_of(int key, guint32 ch, int code) {
    switch (key) {
    case 1: return GDK_KEY_Return;
    case 2: return GDK_KEY_Escape;
    case 3: return GDK_KEY_BackSpace;
    case 4: return GDK_KEY_Delete;
    case 5: return GDK_KEY_Tab;
    case 6: return GDK_KEY_space;
    case 7: return GDK_KEY_Left;
    case 8: return GDK_KEY_Right;
    case 9: return GDK_KEY_Up;
    case 10: return GDK_KEY_Down;
    case 11: return GDK_KEY_Home;
    case 12: return GDK_KEY_End;
    case 13: return GDK_KEY_Page_Up;
    case 14: return GDK_KEY_Page_Down;
    case 15: return gdk_unicode_to_keyval(ch);
    default: break;
    }
    if (code >= 59 && code <= 68)
        return GDK_KEY_F1 + (code - 59);
    switch (code) {
    case 87: return GDK_KEY_F11;
    case 88: return GDK_KEY_F12;
    case 110: return GDK_KEY_Insert;
    case 42: return GDK_KEY_Shift_L;
    case 54: return GDK_KEY_Shift_R;
    case 29: return GDK_KEY_Control_L;
    case 97: return GDK_KEY_Control_R;
    case 56: return GDK_KEY_Alt_L;
    case 100: return GDK_KEY_Alt_R;
    case 125: return GDK_KEY_Super_L;
    case 126: return GDK_KEY_Super_R;
    case 127: return GDK_KEY_Menu;
    default: return GDK_KEY_VoidSymbol;
    }
}

void spatimesh_key(Engine *e, uint64_t id, int key, guint32 ch, int code, int pressed, int modifiers, const char *text) {
    Tab *t = find(e, id);
    if (!t)
        return;
    GdkWindow *w = gtk_widget_get_window(t->window);
    if (!w)
        return;
    guint keyval = keyval_of(key, ch, code);
    GdkEvent *ev = gdk_event_new(pressed ? GDK_KEY_PRESS : GDK_KEY_RELEASE);
    ev->key.window = g_object_ref(w);
    ev->key.send_event = TRUE;
    ev->key.time = now_ms();
    ev->key.state = state_of(modifiers);
    ev->key.keyval = keyval;
    ev->key.hardware_keycode = (guint16)(code + 8);
    ev->key.string = g_strdup(text ? text : "");
    ev->key.length = (gint)strlen(ev->key.string);
    ev->key.is_modifier = (code == 42 || code == 54 || code == 29 || code == 97 || code == 56 || code == 100 || code == 125 || code == 126);
    deliver(ev, TRUE);
}

void spatimesh_focus(Engine *e, uint64_t id, int focused) {
    Tab *t = find(e, id);
    if (!t)
        return;
    GdkWindow *w = gtk_widget_get_window(t->window);
    if (!w)
        return;
    if (focused)
        gtk_widget_grab_focus(GTK_WIDGET(t->view));
    GdkEvent *ev = gdk_event_new(GDK_FOCUS_CHANGE);
    ev->focus_change.window = g_object_ref(w);
    ev->focus_change.send_event = TRUE;
    ev->focus_change.in = focused ? TRUE : FALSE;
    deliver(ev, TRUE);
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

/* ---- the loop and the pictures ----------------------------------------------------------- */

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
    Tab *t = find(e, id);
    if (!t || !t->damaged)
        return 0;
    *x = MAX(t->dx0, 0);
    *y = MAX(t->dy0, 0);
    *w = MIN(t->dx1, t->width) - *x;
    *h = MIN(t->dy1, t->height) - *y;
    if (*w <= 0 || *h <= 0) {
        t->damaged = 0;
        return 0;
    }
    return 1;
}

/* The tab's picture, where it changed, into `pixels` (`width` by `height`, four bytes a pixel,
 * cairo's order), and the change forgotten. */
/* Pictures on the GPU, by number: not from this engine, whose pictures are copied. */
int spatimesh_surface(Engine *e, uint64_t id, uint32_t *surface, int *width, int *height) {
    (void)e; (void)id; (void)surface; (void)width; (void)height;
    return 0;
}

void spatimesh_release_surface(Engine *e, uint64_t id, uint32_t surface) {
    (void)e; (void)id; (void)surface;
}

int spatimesh_copy(Engine *e, uint64_t id, unsigned char *pixels, int width, int height) {
    Tab *t = find(e, id);
    if (!t || !t->damaged)
        return 0;
    cairo_surface_t *source = gtk_offscreen_window_get_surface(GTK_OFFSCREEN_WINDOW(t->window));
    if (!source)
        return 0;
    cairo_surface_t *target = cairo_image_surface_create_for_data(pixels, CAIRO_FORMAT_ARGB32, width, height, width * 4);
    cairo_t *cr = cairo_create(target);
    cairo_rectangle(cr, t->dx0, t->dy0, t->dx1 - t->dx0, t->dy1 - t->dy0);
    cairo_clip(cr);
    cairo_set_operator(cr, CAIRO_OPERATOR_SOURCE);
    cairo_set_source_surface(cr, source, 0, 0);
    cairo_paint(cr);
    cairo_destroy(cr);
    cairo_surface_flush(target);
    cairo_surface_destroy(target);
    t->damaged = 0;
    return 1;
}

/* For tests: a script run in the tab's page. */
void spatimesh_run(Engine *e, uint64_t id, const char *script) {
    Tab *t = find(e, id);
    if (t)
        webkit_web_view_evaluate_javascript(t->view, script, -1, NULL, NULL, NULL, NULL, NULL);
}

/* The window a tab is in: where its sound is to come from. Its page's sound plays from its
 * own web process, through PipeWire; placing it at the window needs the compositor's sink
 * per window (docs/spatimesh.md, "Sound"). Until then a tab sounds where SpatiMesh does. */
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

/* Pages visited, from SpatiMesh's history (this machine's and the others'): links to them look
 * visited in every tab. With Spatiand's WebKit (third_party/wpe/patches/webkit/0002) the
 * profile's tabs share one store of visited links, which WebKit adds to as any of them
 * navigates and which is told these; WebKit without it keeps each web view's visited links to
 * itself, and is told nothing (docs/spatimesh.md, "Visited links"). */
void spatimesh_visited(Engine *e, const char *urls) {
#ifdef WEBKIT_WEB_CONTEXT_HAS_VISITED_LINKS
    char **each = g_strsplit(urls, "\n", -1);
    webkit_web_context_add_visited_links(e->context, (const gchar *const *)each);
    g_strfreev(each);
#else
    (void)e;
    (void)urls;
#endif
}

/* Every visited link forgotten: pages were taken out of the history, and what is left of it is
 * told again (spatimesh_visited). */
void spatimesh_visited_forget(Engine *e) {
#ifdef WEBKIT_WEB_CONTEXT_HAS_VISITED_LINKS
    webkit_web_context_clear_visited_links(e->context);
#else
    (void)e;
#endif
}

/* The wearer is about to load `url` in the tab: with Spatiand's WebKit (patches/webkit/0005) a
 * connection made to its server ahead of it -- name, TCP, TLS -- as <link rel=preconnect> does;
 * otherwise its name looked up. */
void spatimesh_preconnect(Engine *e, uint64_t id, const char *url) {
#ifdef WEBKIT_WEB_VIEW_HAS_PRECONNECT
    Tab *t = find(e, id);
    if (t) {
        webkit_web_view_preconnect(t->view, url);
        return;
    }
#else
    (void)id;
#endif
    GUri *uri = g_uri_parse(url, G_URI_FLAGS_NONE, NULL);
    if (!uri)
        return;
    const char *host = g_uri_get_host(uri);
    if (host && *host)
        webkit_web_context_prefetch_dns(e->context, host);
    g_uri_unref(uri);
}

/* The developer tools: not yet here. WebKitGTK opens them as a GTK window of their own, which
 * SpatiMesh's offscreen windows do not show; SpatiMesh on WPE has them as a tab. */
int spatimesh_inspect(Engine *e, uint64_t id, uint64_t inspector, int width, int height) {
    (void)e;
    (void)id;
    (void)inspector;
    (void)width;
    (void)height;
    return 0;
}
