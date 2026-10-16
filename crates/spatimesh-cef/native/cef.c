/* Steam's Chromium behind the few calls SpatiMesh's engine host makes: the C interface every
 * engine's native half provides (crates/spatimesh-shim). See docs/spatimesh.md.
 *
 * On SteamOS the Steam client carries a Chromium of its own, CEF (the Chromium Embedded
 * Framework), for steamwebhelper: libcef.so and what it reads, in
 * ~/.local/share/Steam/ubuntu12_64. This program loads that library when it starts -- nothing of
 * Steam's is copied or changed -- and uses CEF's C interface, compiled against the headers of
 * the CEF version Steam ships; the library's interface hash is checked against them before
 * anything else is called, and a Steam that has moved on is said so plainly.
 *
 * Every tab is a windowless CEF browser: Chromium draws its page into memory and hands each
 * frame over with what changed (on_paint), and the wearer's input is given to it as CEF's own
 * events. Each tab's page is in Chromium's own renderer processes, which are this program again,
 * started with --type=renderer. Whatever Chromium would otherwise show in a window of its own
 * -- a script's alert, a context menu, a new window -- is answered here or handed to SpatiMesh's
 * window; a <select>'s list is asked of SpatiMesh by a script in the page.
 *
 * What happens to pages is queued as events, popped by the Rust side after each pump. */

#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <ftw.h>
#include <libgen.h>
#include <math.h>
#include <poll.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/eventfd.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#include "include/cef_api_hash.h"
#include "include/cef_version.h"
#include "include/capi/cef_app_capi.h"
#include "include/capi/cef_browser_capi.h"
#include "include/capi/cef_browser_process_handler_capi.h"
#include "include/capi/cef_client_capi.h"
#include "include/capi/cef_command_line_capi.h"
#include "include/capi/cef_context_menu_handler_capi.h"
#include "include/capi/cef_dialog_handler_capi.h"
#include "include/capi/cef_display_handler_capi.h"
#include "include/capi/cef_download_handler_capi.h"
#include "include/capi/cef_download_item_capi.h"
#include "include/capi/cef_frame_capi.h"
#include "include/capi/cef_jsdialog_handler_capi.h"
#include "include/capi/cef_life_span_handler_capi.h"
#include "include/capi/cef_load_handler_capi.h"
#include "include/capi/cef_render_handler_capi.h"
#include "include/capi/cef_request_handler_capi.h"
#include "include/capi/cef_values_capi.h"
#include "include/capi/cef_process_message_capi.h"
#include "include/capi/cef_render_process_handler_capi.h"
#include "include/capi/cef_request_capi.h"
#include "include/capi/cef_resource_handler_capi.h"
#include "include/capi/cef_resource_request_handler_capi.h"
#include "include/capi/cef_response_capi.h"
#include "include/capi/cef_scheme_capi.h"
#include "include/capi/cef_v8_capi.h"

/* ---- the library, loaded at run time ------------------------------------------------------ */

/* Every function of libcef's this calls, typed by CEF's own headers. */
#define CEF_FUNCTIONS(X)                                                                          \
    X(cef_api_hash)                                                                               \
    X(cef_version_info)                                                                           \
    X(cef_execute_process)                                                                        \
    X(cef_initialize)                                                                             \
    X(cef_shutdown)                                                                               \
    X(cef_do_message_loop_work)                                                                   \
    X(cef_browser_host_create_browser_sync)                                                       \
    X(cef_string_utf8_to_utf16)                                                                   \
    X(cef_string_utf16_to_utf8)                                                                   \
    X(cef_string_utf16_clear)                                                                     \
    X(cef_string_utf8_clear)                                                                      \
    X(cef_string_userfree_utf16_free)                                                             \
    X(cef_process_message_create)                                                                 \
    X(cef_list_value_create)                                                                      \
    X(cef_dictionary_value_create)                                                                \
    X(cef_v8value_create_function)                                                                \
    X(cef_v8value_create_string)                                                                  \
    X(cef_v8value_create_object)                                                                  \
    X(cef_v8value_create_undefined)                                                               \
    X(cef_v8context_get_current_context)                                                          \
    X(cef_string_list_alloc)                                                                      \
    X(cef_string_list_size)                                                                       \
    X(cef_string_list_value)                                                                      \
    X(cef_string_list_free)

#define DECLARE(name) static __typeof__(name) *p_##name;
CEF_FUNCTIONS(DECLARE)

static void *library;
static char error_text[9000];
static char cef_folder[4096];
static int argc_;
static char **argv_;

static int has_libcef(const char *dir) {
    char path[4200];
    snprintf(path, sizeof path, "%s/libcef.so", dir);
    return access(path, R_OK) == 0;
}

/* Where Steam keeps its Chromium, or where SPATIMESH_CEF_DIR says another is. */
static const char *find_cef(void) {
    if (cef_folder[0])
        return cef_folder;
    const char *given = getenv("SPATIMESH_CEF_DIR");
    if (given && *given) {
        snprintf(cef_folder, sizeof cef_folder, "%s", given);
        return has_libcef(cef_folder) ? cef_folder : (cef_folder[0] = 0, NULL);
    }
    const char *home = getenv("HOME");
    const char *places[] = {"/.local/share/Steam/ubuntu12_64", "/.steam/steam/ubuntu12_64", "/.steam/root/ubuntu12_64"};
    for (size_t i = 0; home && i < sizeof places / sizeof *places; i++) {
        snprintf(cef_folder, sizeof cef_folder, "%s%s", home, places[i]);
        if (has_libcef(cef_folder))
            return cef_folder;
    }
    cef_folder[0] = 0;
    return NULL;
}

static int load_cef(void) {
    if (library)
        return 1;
    const char *dir = find_cef();
    if (!dir) {
        snprintf(error_text, sizeof error_text,
                 "Steam's Chromium was not found: no libcef.so in ~/.local/share/Steam/ubuntu12_64 (is the Steam client "
                 "installed?) or in SPATIMESH_CEF_DIR");
        return 0;
    }
    char path[4200];
    snprintf(path, sizeof path, "%s/libcef.so", dir);
    void *lib = dlopen(path, RTLD_NOW | RTLD_LOCAL);
    if (!lib) {
        snprintf(error_text, sizeof error_text, "Steam's Chromium would not load: %s", dlerror());
        return 0;
    }
#define LOOKUP(name)                                                                              \
    p_##name = (__typeof__(name) *)dlsym(lib, #name);                                             \
    if (!p_##name) {                                                                              \
        snprintf(error_text, sizeof error_text, "%s has no %s: not a CEF this can use", path, #name); \
        return 0;                                                                                 \
    }
    CEF_FUNCTIONS(LOOKUP)
    /* The interface this was compiled against, or nothing is called: a struct laid out
     * differently would be read wrongly, not refused. */
    const char *hash = p_cef_api_hash(0);
    if (!hash || strcmp(hash, CEF_API_HASH_PLATFORM) != 0) {
        snprintf(error_text, sizeof error_text,
                 "Steam's Chromium is CEF %d.%d.%d (Chromium %d.%d.%d.%d), and SpatiMesh was built for CEF %s: it has to be "
                 "rebuilt against the headers of the one Steam has now",
                 p_cef_version_info(0), p_cef_version_info(1), p_cef_version_info(2), p_cef_version_info(4),
                 p_cef_version_info(5), p_cef_version_info(6), p_cef_version_info(7), CEF_VERSION);
        return 0;
    }
    library = lib;
    return 1;
}

const char *spatimesh_cef_error(void) {
    return error_text;
}

/* The program's arguments as the system laid them out, handed to constructors by glibc:
 * Chromium reads them, and titles each of its processes by writing over them (what `ps` shows,
 * --type=renderer and the like). */
static void __attribute__((constructor)) keep_arguments(int argc, char **argv, char **envp) {
    argc_ = argc;
    argv_ = argv;
}

/* The arguments, where the system did not hand them over (not glibc): copies, which Chromium
 * reads all the same. */
void spatimesh_cef_args(int argc, char **argv) {
    if (!argv_) {
        argc_ = argc;
        argv_ = argv;
    }
}

/* ---- strings ------------------------------------------------------------------------------- */

static void set_text(cef_string_t *s, const char *utf8) {
    p_cef_string_utf8_to_utf16(utf8 ? utf8 : "", utf8 ? strlen(utf8) : 0, s);
}

static void clear_text(cef_string_t *s) {
    p_cef_string_utf16_clear(s);
}

/* A CEF string as UTF-8, to be freed. */
static char *utf8_of(const cef_string_t *s) {
    if (!s || !s->str || !s->length)
        return strdup("");
    cef_string_utf8_t out;
    memset(&out, 0, sizeof out);
    p_cef_string_utf16_to_utf8(s->str, s->length, &out);
    char *copy = strndup(out.str ? out.str : "", out.length);
    p_cef_string_utf8_clear(&out);
    return copy;
}

/* Structures CEF hands over -- returned, or given to a handler -- come with a reference for
 * whoever receives them. One handed to CEF as an argument takes a reference along with it (CEF
 * releases it: libcef_dll's CppToC::Unwrap), so it is not released after the call; one still to
 * be used afterwards is given a reference of its own first (SHARE). */
#define RELEASE(p)                                                                                \
    do {                                                                                          \
        if (p)                                                                                    \
            (p)->base.release(&(p)->base);                                                        \
    } while (0)
#define SHARE(p) ((p)->base.add_ref(&(p)->base), (p))

/* ---- tabs and what happened to them -------------------------------------------------------- */

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

typedef struct Queued {
    MeshEvent ev;
    struct Queued *next;
} Queued;

typedef struct Tab {
    uint64_t id;
    int browser_id;
    cef_browser_t *browser;
    /* The picture's size, in pixels, and the page's zoom: Chromium draws at that scale. */
    int width, height;
    double zoom;
    /* Chromium's newest frame of the page, and its size. */
    unsigned char *pixels;
    int pw, ph;
    /* A <select>'s list or the like that Chromium draws itself, over the page: where (in
     * pixels), and its picture. */
    int popup_shown;
    cef_rect_t popup;
    unsigned char *popup_pixels;
    int popw, poph;
    /* What changed since the picture was last read, as one rectangle. */
    int damaged, dx0, dy0, dx1, dy1;
    int loading;
    cef_jsdialog_callback_t *dialog;
    /* The open <select>'s lines. */
    int choices;
    char **labels;
    int *flags;
    /* Mouse buttons held, as CEF's flags, and what makes a double click. */
    uint32_t buttons;
    int clicks, last_button, last_x, last_y, mouse_x, mouse_y;
    int64_t last_click;
    uint32_t window;
    struct Tab *next;
} Tab;

typedef struct Engine {
    int unused;
} Engine;

static Engine the_engine;
static int started;
static Tab *tabs;
/* The tab whose browser is being made: Chromium asks about it before saying who it is. */
static Tab *creating;
static int live_browsers;
static Queued *first, *last;
static MeshEvent current;
static char *cache_folder;
static char *private_folder;
static int watch_fd = -1, wake_fd = -1;
static _Atomic int64_t work_due = INT64_MAX;

static int64_t now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

static Queued *queue(int kind, uint64_t tab, double number, int a, int b, const char *text, const char *text2) {
    Queued *q = calloc(1, sizeof *q);
    q->ev.kind = kind;
    q->ev.tab = tab;
    q->ev.number = number;
    q->ev.a = a;
    q->ev.b = b;
    q->ev.text = text ? strdup(text) : NULL;
    q->ev.text2 = text2 ? strdup(text2) : NULL;
    if (last)
        last->next = q;
    else
        first = q;
    last = q;
    return q;
}

static void push(int kind, uint64_t tab, double number, int a, int b, const char *text, const char *text2) {
    queue(kind, tab, number, a, b, text, text2);
}

static Tab *find(uint64_t id) {
    for (Tab *t = tabs; t; t = t->next)
        if (t->id == id)
            return t;
    return NULL;
}

static Tab *tab_of(cef_browser_t *browser) {
    if (!browser)
        return NULL;
    int id = browser->get_identifier(browser);
    for (Tab *t = tabs; t; t = t->next)
        if (t->browser_id == id)
            return t;
    if (creating && !creating->browser_id) {
        creating->browser_id = id;
        return creating;
    }
    return NULL;
}

static int clampi(int v, int lo, int hi) {
    return v < lo ? lo : v > hi ? hi : v;
}

static void damage(Tab *t, int x0, int y0, int x1, int y1) {
    x0 = clampi(x0, 0, t->width);
    x1 = clampi(x1, 0, t->width);
    y0 = clampi(y0, 0, t->height);
    y1 = clampi(y1, 0, t->height);
    if (x1 <= x0 || y1 <= y0)
        return;
    if (!t->damaged) {
        t->damaged = 1;
        t->dx0 = x0;
        t->dy0 = y0;
        t->dx1 = x1;
        t->dy1 = y1;
    } else {
        t->dx0 = x0 < t->dx0 ? x0 : t->dx0;
        t->dy0 = y0 < t->dy0 ? y0 : t->dy0;
        t->dx1 = x1 > t->dx1 ? x1 : t->dx1;
        t->dy1 = y1 > t->dy1 ? y1 : t->dy1;
    }
}

static void forget_choices(Tab *t) {
    for (int i = 0; i < t->choices; i++)
        free(t->labels[i]);
    free(t->labels);
    free(t->flags);
    t->labels = NULL;
    t->flags = NULL;
    t->choices = 0;
}

/* A dialog still waiting is answered no: one at a time. */
static void answer_dialog(Tab *t, int accepted, const char *text) {
    if (!t->dialog)
        return;
    cef_string_t s;
    memset(&s, 0, sizeof s);
    set_text(&s, text ? text : "");
    t->dialog->cont(t->dialog, accepted, &s);
    clear_text(&s);
    RELEASE(t->dialog);
    t->dialog = NULL;
}

static void run_script(Tab *t, const char *script) {
    if (!t->browser)
        return;
    cef_frame_t *frame = t->browser->get_main_frame(t->browser);
    if (!frame)
        return;
    cef_string_t code, url;
    memset(&code, 0, sizeof code);
    memset(&url, 0, sizeof url);
    set_text(&code, script);
    frame->execute_java_script(frame, &code, &url, 0);
    clear_text(&code);
    RELEASE(frame);
}

/* ---- handlers -----------------------------------------------------------------------------
 * One of each, for every tab: they live as long as the process, so their reference counts are
 * not kept. */

static void CEF_CALLBACK kept_add_ref(cef_base_ref_counted_t *self) {}
static int CEF_CALLBACK kept_release(cef_base_ref_counted_t *self) {
    return 0;
}
static int CEF_CALLBACK kept_has_one_ref(cef_base_ref_counted_t *self) {
    return 1;
}
static int CEF_CALLBACK kept_has_at_least_one_ref(cef_base_ref_counted_t *self) {
    return 1;
}

static void kept(cef_base_ref_counted_t *base, size_t size) {
    base->size = size;
    base->add_ref = kept_add_ref;
    base->release = kept_release;
    base->has_one_ref = kept_has_one_ref;
    base->has_at_least_one_ref = kept_has_at_least_one_ref;
}

static cef_app_t app;
static cef_browser_process_handler_t process_handler;
static cef_client_t client;
static cef_render_handler_t render_handler;
static cef_life_span_handler_t life_span_handler;
static cef_load_handler_t load_handler;
static cef_display_handler_t display_handler;
static cef_jsdialog_handler_t jsdialog_handler;
static cef_request_handler_t request_handler;
static cef_context_menu_handler_t context_menu_handler;
static cef_dialog_handler_t dialog_handler;
static cef_download_handler_t download_handler;

/* -- the program -- */

static void add_switch(cef_command_line_t *line, const char *name, const char *value) {
    cef_string_t n, v;
    memset(&n, 0, sizeof n);
    memset(&v, 0, sizeof v);
    set_text(&n, name);
    if (value) {
        set_text(&v, value);
        line->append_switch_with_value(line, &n, &v);
        clear_text(&v);
    } else {
        line->append_switch(line, &n);
    }
    clear_text(&n);
}

static void CEF_CALLBACK on_command_line(cef_app_t *self, const cef_string_t *process_type, cef_command_line_t *line) {
    if (!process_type || !process_type->length) {
        /* Drawn by the processor into memory, which is all SpatiMesh reads (on_paint). */
        add_switch(line, "disable-gpu", NULL);
        add_switch(line, "disable-gpu-compositing", NULL);
        /* Cookies' key kept by Chromium, not in a desktop's keyring that Game Mode has not got. */
        add_switch(line, "password-store", "basic");
        /* The cache in the profile's cache folder, never with what is kept. */
        if (cache_folder)
            add_switch(line, "disk-cache-dir", cache_folder);
        const char *ozone = getenv("SPATIMESH_CEF_OZONE");
        if (ozone && *ozone)
            add_switch(line, "ozone-platform", ozone);
        /* What goes wrong said where the host's own messages go (Spatiand's applications'
         * log), not in a file of Chromium's beside the program. */
        add_switch(line, "enable-logging", "stderr");
    }
    RELEASE(line);
}

static cef_browser_process_handler_t *CEF_CALLBACK get_process_handler(cef_app_t *self) {
    return &process_handler;
}

/* Chromium asks for its work to be done, from any of its threads: when, and a wake-up. */
static void CEF_CALLBACK on_schedule_work(cef_browser_process_handler_t *self, int64_t delay_ms) {
    int64_t at = now_ms() + (delay_ms > 0 ? delay_ms : 0);
    int64_t was = atomic_load(&work_due);
    while (at < was && !atomic_compare_exchange_weak(&work_due, &was, at)) {
    }
    if (wake_fd >= 0) {
        uint64_t one = 1;
        ssize_t r = write(wake_fd, &one, sizeof one);
        (void)r;
    }
}

/* -- the client -- */

static cef_render_handler_t *CEF_CALLBACK get_render_handler(cef_client_t *self) {
    return &render_handler;
}
static cef_life_span_handler_t *CEF_CALLBACK get_life_span_handler(cef_client_t *self) {
    return &life_span_handler;
}
static cef_load_handler_t *CEF_CALLBACK get_load_handler(cef_client_t *self) {
    return &load_handler;
}
static cef_display_handler_t *CEF_CALLBACK get_display_handler(cef_client_t *self) {
    return &display_handler;
}
static cef_jsdialog_handler_t *CEF_CALLBACK get_jsdialog_handler(cef_client_t *self) {
    return &jsdialog_handler;
}
static cef_request_handler_t *CEF_CALLBACK get_request_handler(cef_client_t *self) {
    return &request_handler;
}
static cef_context_menu_handler_t *CEF_CALLBACK get_context_menu_handler(cef_client_t *self) {
    return &context_menu_handler;
}
static cef_dialog_handler_t *CEF_CALLBACK get_dialog_handler(cef_client_t *self) {
    return &dialog_handler;
}
static cef_download_handler_t *CEF_CALLBACK get_download_handler(cef_client_t *self) {
    return &download_handler;
}

/* -- drawing -- */

static void CEF_CALLBACK get_view_rect(cef_render_handler_t *self, cef_browser_t *browser, cef_rect_t *rect) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    double zoom = t ? t->zoom : 1.0;
    rect->x = 0;
    rect->y = 0;
    rect->width = t ? (int)lround(t->width / zoom) : 800;
    rect->height = t ? (int)lround(t->height / zoom) : 600;
    if (rect->width < 1)
        rect->width = 1;
    if (rect->height < 1)
        rect->height = 1;
}

static int CEF_CALLBACK get_screen_info(cef_render_handler_t *self, cef_browser_t *browser, cef_screen_info_t *info) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    if (!t)
        return 0;
    /* The page's zoom as the screen's scale: pages drawn sharp at SpatiMesh's size. */
    info->device_scale_factor = (float)t->zoom;
    info->depth = 24;
    info->depth_per_component = 8;
    info->is_monochrome = 0;
    info->rect.x = 0;
    info->rect.y = 0;
    info->rect.width = (int)lround(t->width / t->zoom);
    info->rect.height = (int)lround(t->height / t->zoom);
    info->available_rect = info->rect;
    return 1;
}

static void CEF_CALLBACK on_popup_show(cef_render_handler_t *self, cef_browser_t *browser, int show) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    if (!t)
        return;
    if (!show) {
        damage(t, t->popup.x, t->popup.y, t->popup.x + t->popup.width, t->popup.y + t->popup.height);
        free(t->popup_pixels);
        t->popup_pixels = NULL;
        t->popw = t->poph = 0;
    }
    t->popup_shown = show;
}

static void CEF_CALLBACK on_popup_size(cef_render_handler_t *self, cef_browser_t *browser, const cef_rect_t *rect) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    if (!t)
        return;
    damage(t, t->popup.x, t->popup.y, t->popup.x + t->popup.width, t->popup.y + t->popup.height);
    t->popup.x = (int)lround(rect->x * t->zoom);
    t->popup.y = (int)lround(rect->y * t->zoom);
    t->popup.width = (int)lround(rect->width * t->zoom);
    t->popup.height = (int)lround(rect->height * t->zoom);
}

static void CEF_CALLBACK on_paint(cef_render_handler_t *self, cef_browser_t *browser, cef_paint_element_type_t type,
                                  size_t count, const cef_rect_t *rects, const void *buffer, int width, int height) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    if (!t || width <= 0 || height <= 0 || !buffer)
        return;
    const unsigned char *from = buffer;
    size_t size = (size_t)width * height * 4;
    if (type == PET_POPUP) {
        if (t->popw != width || t->poph != height) {
            free(t->popup_pixels);
            t->popup_pixels = malloc(size);
            t->popw = width;
            t->poph = height;
        }
        if (t->popup_pixels)
            memcpy(t->popup_pixels, from, size);
        damage(t, t->popup.x, t->popup.y, t->popup.x + width, t->popup.y + height);
        return;
    }
    if (!t->pixels || t->pw != width || t->ph != height) {
        free(t->pixels);
        t->pixels = malloc(size);
        t->pw = width;
        t->ph = height;
        if (t->pixels)
            memcpy(t->pixels, from, size);
        damage(t, 0, 0, t->width, t->height);
        return;
    }
    for (size_t i = 0; i < count; i++) {
        int x0 = clampi(rects[i].x, 0, width), x1 = clampi(rects[i].x + rects[i].width, 0, width);
        int y0 = clampi(rects[i].y, 0, height), y1 = clampi(rects[i].y + rects[i].height, 0, height);
        for (int y = y0; y < y1; y++)
            memcpy(t->pixels + ((size_t)y * width + x0) * 4, from + ((size_t)y * width + x0) * 4, (size_t)(x1 - x0) * 4);
        damage(t, x0, y0, x1, y1);
    }
}

/* -- tabs opening and closing -- */

static int CEF_CALLBACK on_before_popup(cef_life_span_handler_t *self, cef_browser_t *browser, cef_frame_t *frame,
                                        const cef_string_t *target_url, const cef_string_t *target_frame_name,
                                        cef_window_open_disposition_t disposition, int user_gesture,
                                        const cef_popup_features_t *features, cef_window_info_t *window_info,
                                        cef_client_t **popup_client, cef_browser_settings_t *settings,
                                        cef_dictionary_value_t **extra_info, int *no_javascript_access) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    RELEASE(frame);
    if (t) {
        char *url = utf8_of(target_url);
        /* A new window is SpatiMesh's to open, as a tab (4: a background tab). */
        push(EV_OPEN_TAB, t->id, 0, (int)disposition == 4, 0, url, NULL);
        free(url);
    }
    return 1;
}

static void CEF_CALLBACK on_after_created(cef_life_span_handler_t *self, cef_browser_t *browser) {
    live_browsers++;
    RELEASE(browser);
}

static int CEF_CALLBACK do_close(cef_life_span_handler_t *self, cef_browser_t *browser) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    /* A page closing itself (window.close()): SpatiMesh closes the tab, which comes back here
     * when the tab is gone. */
    if (t) {
        push(EV_CLOSE_TAB, t->id, 0, 0, 0, NULL, NULL);
        return 1;
    }
    return 0;
}

static void CEF_CALLBACK on_before_close(cef_life_span_handler_t *self, cef_browser_t *browser) {
    live_browsers--;
    RELEASE(browser);
}

/* -- loading -- */

static void CEF_CALLBACK on_loading_state_change(cef_load_handler_t *self, cef_browser_t *browser, int loading, int back,
                                                 int forward) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    if (!t)
        return;
    t->loading = loading;
    push(EV_PROGRESS, t->id, loading ? 0.1 : 1.0, loading, 0, NULL, NULL);
    push(EV_HISTORY, t->id, 0, back, forward, NULL, NULL);
}

/* A <select>'s list, asked of SpatiMesh: the page's script says so in a console message only
 * this reads, as left,top,width,height then for each line 1 enabled + 2 selected and its label,
 * each after a \x01. */
#define SELECT_MARK "\x01spatimesh-select\x01"
static const char SELECT_SCRIPT[] =
    "(function(){if(window.__spatimesh)return;window.__spatimesh=1;var log=console.log.bind(console);"
    "document.addEventListener('mousedown',function(ev){var s=ev.target.closest&&ev.target.closest('select');"
    "if(!s||s.multiple||s.size>1||s.disabled||ev.button!==0)return;ev.preventDefault();s.focus();"
    "window.__spatimeshSelect=s;var r=s.getBoundingClientRect(),m=[Math.round(r.left),Math.round(r.top),"
    "Math.round(r.width),Math.round(r.height)].join(',');for(var i=0;i<s.options.length;i++){var o=s.options[i];"
    "m+='\\u0001'+((o.disabled?0:1)|(o.selected?2:0))+String(o.label||o.text).replace(/\\u0001/g,' ');}"
    "log('\\u0001spatimesh-select\\u0001'+m);},true);"
    "window.__spatimeshChoose=function(i){var s=window.__spatimeshSelect;window.__spatimeshSelect=null;"
    "if(!s||i<0||i>=s.options.length)return;if(s.selectedIndex!==i){s.selectedIndex=i;"
    "s.dispatchEvent(new Event('input',{bubbles:true}));s.dispatchEvent(new Event('change',{bubbles:true}));}};})();";

static void CEF_CALLBACK on_load_end(cef_load_handler_t *self, cef_browser_t *browser, cef_frame_t *frame, int status) {
    Tab *t = tab_of(browser);
    int main_frame = frame && frame->is_main(frame);
    RELEASE(browser);
    RELEASE(frame);
    if (t && main_frame)
        run_script(t, SELECT_SCRIPT);
}

static void CEF_CALLBACK on_load_error(cef_load_handler_t *self, cef_browser_t *browser, cef_frame_t *frame,
                                       cef_errorcode_t code, const cef_string_t *text, const cef_string_t *failed_url) {
    Tab *t = tab_of(browser);
    int main_frame = frame && frame->is_main(frame);
    RELEASE(browser);
    RELEASE(frame);
    /* A load given up for another is not a failure. */
    if (!t || !main_frame || code == ERR_ABORTED)
        return;
    char *url = utf8_of(failed_url), *why = utf8_of(text);
    push(EV_FAILED, t->id, 0, 0, 0, url, why);
    free(url);
    free(why);
}

/* -- what the page shows -- */

static void CEF_CALLBACK on_address_change(cef_display_handler_t *self, cef_browser_t *browser, cef_frame_t *frame,
                                          const cef_string_t *address) {
    Tab *t = tab_of(browser);
    int main_frame = frame && frame->is_main(frame);
    RELEASE(browser);
    RELEASE(frame);
    if (!t || !main_frame)
        return;
    char *url = utf8_of(address);
    push(EV_URL, t->id, 0, 0, 0, url, NULL);
    free(url);
}

static void CEF_CALLBACK on_title_change(cef_display_handler_t *self, cef_browser_t *browser, const cef_string_t *title) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    if (!t)
        return;
    char *text = utf8_of(title);
    push(EV_TITLE, t->id, 0, 0, 0, text, NULL);
    free(text);
}

static int CEF_CALLBACK on_tooltip(cef_display_handler_t *self, cef_browser_t *browser, cef_string_t *text) {
    RELEASE(browser);
    return 1; /* never a window of Chromium's own */
}

static void select_list(Tab *t, const char *said) {
    int left = 0, top = 0, width = 0, height = 0;
    if (sscanf(said, "%d,%d,%d,%d", &left, &top, &width, &height) != 4)
        return;
    forget_choices(t);
    int n = 0;
    for (const char *p = said; (p = strchr(p, '\x01')); p++)
        n++;
    t->labels = calloc(n ? n : 1, sizeof *t->labels);
    t->flags = calloc(n ? n : 1, sizeof *t->flags);
    const char *p = strchr(said, '\x01');
    while (p && t->choices < n) {
        const char *line = p + 1;
        const char *end = strchr(line, '\x01');
        size_t length = end ? (size_t)(end - line) : strlen(line);
        t->flags[t->choices] = length ? line[0] - '0' : 1;
        t->labels[t->choices] = strndup(length ? line + 1 : "", length ? length - 1 : 0);
        t->choices++;
        p = end;
    }
    Queued *q = queue(EV_CHOOSE, t->id, 0, t->choices, 0, NULL, NULL);
    q->ev.x = (int)lround(left * t->zoom);
    q->ev.y = (int)lround(top * t->zoom);
    q->ev.w = (int)lround(width * t->zoom);
    q->ev.h = (int)lround(height * t->zoom);
}

static int CEF_CALLBACK on_console_message(cef_display_handler_t *self, cef_browser_t *browser, cef_log_severity_t level,
                                           const cef_string_t *message, const cef_string_t *source, int line) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    if (!t || !message)
        return 0;
    char *text = utf8_of(message);
    int ours = strncmp(text, SELECT_MARK, strlen(SELECT_MARK)) == 0;
    if (ours) {
        select_list(t, text + strlen(SELECT_MARK));
    } else if (getenv("SPATIMESH_CONSOLE")) {
        /* What pages say on their consoles, where asked for (tests). */
        char *where = source ? utf8_of(source) : strdup("");
        fprintf(stderr, "spatimesh-cef: console of tab %llu: %s (%s:%d)\n", (unsigned long long)t->id, text, where, line);
        free(where);
    }
    free(text);
    return ours;
}

static void CEF_CALLBACK on_progress(cef_display_handler_t *self, cef_browser_t *browser, double progress) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    if (t)
        push(EV_PROGRESS, t->id, progress, t->loading, 0, NULL, NULL);
}

static int CEF_CALLBACK on_cursor_change(cef_display_handler_t *self, cef_browser_t *browser, cef_cursor_handle_t cursor,
                                         cef_cursor_type_t type, const cef_cursor_info_t *custom) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    if (!t)
        return 1;
    /* SpatiMesh's numbering: crates/spatimesh/src/wire.rs. */
    int n = 0;
    switch (type) {
    case CT_HAND: n = 1; break;
    case CT_IBEAM: n = 2; break;
    case CT_WAIT:
    case CT_PROGRESS: n = 3; break;
    case CT_CROSS: n = 4; break;
    case CT_MOVE: n = 5; break;
    case CT_EASTWESTRESIZE:
    case CT_COLUMNRESIZE: n = 6; break;
    case CT_NORTHSOUTHRESIZE:
    case CT_ROWRESIZE: n = 7; break;
    case CT_NOTALLOWED:
    case CT_NODROP: n = 8; break;
    case CT_GRAB: n = 9; break;
    case CT_GRABBING: n = 10; break;
    case CT_NONE: n = 11; break;
    default: n = 0; break;
    }
    push(EV_CURSOR, t->id, 0, n, 0, NULL, NULL);
    return 1;
}

/* -- script dialogs, SpatiMesh's to show -- */

static int CEF_CALLBACK on_jsdialog(cef_jsdialog_handler_t *self, cef_browser_t *browser, const cef_string_t *origin,
                                    cef_jsdialog_type_t type, const cef_string_t *message, const cef_string_t *prompt,
                                    cef_jsdialog_callback_t *callback, int *suppress) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    if (!t) {
        RELEASE(callback);
        *suppress = 1;
        return 0;
    }
    answer_dialog(t, 0, NULL);
    t->dialog = callback;
    char *m = utf8_of(message), *p = utf8_of(prompt);
    int kind = type == JSDIALOGTYPE_CONFIRM ? 1 : type == JSDIALOGTYPE_PROMPT ? 2 : 0;
    push(EV_DIALOG, t->id, 0, kind, 0, m, kind == 2 ? p : "");
    free(m);
    free(p);
    return 1;
}

static int CEF_CALLBACK on_before_unload(cef_jsdialog_handler_t *self, cef_browser_t *browser, const cef_string_t *message,
                                         int is_reload, cef_jsdialog_callback_t *callback) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    if (!t) {
        RELEASE(callback);
        return 0;
    }
    answer_dialog(t, 0, NULL);
    t->dialog = callback;
    char *m = utf8_of(message);
    push(EV_DIALOG, t->id, 0, 3, 0, m, "");
    free(m);
    return 1;
}

static void CEF_CALLBACK on_reset_dialogs(cef_jsdialog_handler_t *self, cef_browser_t *browser) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    /* Chromium has given up on the dialog itself (the page went). */
    if (t && t->dialog) {
        RELEASE(t->dialog);
        t->dialog = NULL;
    }
}

/* -- links that want a tab, pages' processes that end -- */

static int CEF_CALLBACK on_open_url_from_tab(cef_request_handler_t *self, cef_browser_t *browser, cef_frame_t *frame,
                                             const cef_string_t *target_url, cef_window_open_disposition_t disposition,
                                             int user_gesture) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    RELEASE(frame);
    /* 3 a tab in front, 4 behind, 5 a popup, 6 a window: all SpatiMesh's tabs. */
    int d = (int)disposition;
    if (!t || d < 3 || d > 6)
        return 0;
    char *url = utf8_of(target_url);
    push(EV_OPEN_TAB, t->id, 0, d == 4, 0, url, NULL);
    free(url);
    return 1;
}

static void CEF_CALLBACK on_renderer_gone(cef_request_handler_t *self, cef_browser_t *browser, cef_termination_status_t status,
                                          int error_code, const cef_string_t *error_string) {
    Tab *t = tab_of(browser);
    RELEASE(browser);
    if (!t)
        return;
    if (t->dialog) {
        RELEASE(t->dialog);
        t->dialog = NULL;
    }
    const char *why = "The page's process ended";
    switch ((int)status) {
    case 0: why = "The page's process ended abnormally"; break;
    case 1: why = "The page's process was killed"; break;
    case 2: why = "The page's process crashed"; break;
    case 3: why = "The page's process ran out of memory"; break;
    default: break;
    }
    push(EV_CRASHED, t->id, 0, 0, 0, why, NULL);
}

static void CEF_CALLBACK on_before_context_menu(cef_context_menu_handler_t *self, cef_browser_t *browser, cef_frame_t *frame,
                                                cef_context_menu_params_t *params, cef_menu_model_t *model) {
    /* No menu of Chromium's own in the room. */
    if (model)
        model->clear(model);
    RELEASE(browser);
    RELEASE(frame);
    RELEASE(params);
    RELEASE(model);
}

/* -- choosing files: not yet SpatiMesh's to show, and never a window of Chromium's own -- */

static int CEF_CALLBACK on_file_dialog(cef_dialog_handler_t *self, cef_browser_t *browser, cef_file_dialog_mode_t mode,
                                       const cef_string_t *title, const cef_string_t *default_file_path,
                                       cef_string_list_t accept_filters, cef_string_list_t accept_extensions,
                                       cef_string_list_t accept_descriptions, cef_file_dialog_callback_t *callback) {
    if (callback)
        callback->cancel(callback);
    RELEASE(callback);
    RELEASE(browser);
    return 1;
}

/* -- downloads: in the cache while they come, in Downloads once they have -- */

static void make_folders(char *path) {
    for (char *p = path + 1; *p; p++) {
        if (*p == '/') {
            *p = 0;
            mkdir(path, 0700);
            *p = '/';
        }
    }
    mkdir(path, 0700);
}

/* Where downloads are while they come: the profile's cache, thrown away freely. */
static const char *downloads_coming(void) {
    static char dir[4400];
    if (!dir[0]) {
        const char *xdg = getenv("XDG_CACHE_HOME"), *home = getenv("HOME");
        if (cache_folder)
            snprintf(dir, sizeof dir, "%s/downloads", cache_folder);
        else if (xdg && xdg[0] == '/')
            snprintf(dir, sizeof dir, "%s/spatiand/spatimesh/downloads", xdg);
        else
            snprintf(dir, sizeof dir, "%s/.cache/spatiand/spatimesh/downloads", home ? home : "/tmp");
        make_folders(dir);
    }
    return dir;
}

static int copy_file(const char *from, const char *to) {
    int in = open(from, O_RDONLY | O_CLOEXEC);
    if (in < 0)
        return -1;
    int out = open(to, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0644);
    if (out < 0) {
        close(in);
        return -1;
    }
    char buffer[65536];
    ssize_t n;
    int ok = 1;
    while ((n = read(in, buffer, sizeof buffer)) > 0)
        if (write(out, buffer, (size_t)n) != n) {
            ok = 0;
            break;
        }
    close(in);
    close(out);
    if (!ok || n < 0) {
        unlink(to);
        return -1;
    }
    unlink(from);
    return 0;
}

/* A finished download into ~/Downloads, under its own name or the first free one like it. */
static void move_to_downloads(const char *from) {
    const char *home = getenv("HOME");
    char dir[4300], to[4600], copy[4300];
    snprintf(dir, sizeof dir, "%s/Downloads", home ? home : "/tmp");
    make_folders(dir);
    snprintf(copy, sizeof copy, "%s", from);
    const char *name = basename(copy);
    /* Its name here begins with the download's number, to keep two of one name apart. */
    const char *dash = strchr(name, '-');
    if (dash && dash[1])
        name = dash + 1;
    const char *dot = strrchr(name, '.');
    int stem = dot && dot != name ? (int)(dot - name) : (int)strlen(name);
    snprintf(to, sizeof to, "%s/%s", dir, name);
    for (int n = 2; access(to, F_OK) == 0 && n < 1000; n++)
        snprintf(to, sizeof to, "%s/%.*s (%d)%s", dir, stem, name, n, dot && dot != name ? dot : "");
    if (rename(from, to) != 0 && errno == EXDEV)
        copy_file(from, to);
}

static int CEF_CALLBACK on_before_download(cef_download_handler_t *self, cef_browser_t *browser, cef_download_item_t *item,
                                           const cef_string_t *suggested_name, cef_before_download_callback_t *callback) {
    char *name = utf8_of(suggested_name);
    for (char *c = name; *c; c++)
        if (*c == '/')
            *c = '_';
    char path[4700];
    snprintf(path, sizeof path, "%s/%u-%s", downloads_coming(), item ? item->get_id(item) : 0u,
             name[0] && strcmp(name, ".") && strcmp(name, "..") ? name : "download");
    free(name);
    cef_string_t target;
    memset(&target, 0, sizeof target);
    set_text(&target, path);
    callback->cont(callback, &target, 0);
    clear_text(&target);
    RELEASE(callback);
    RELEASE(item);
    RELEASE(browser);
    return 1;
}

static void CEF_CALLBACK on_download_updated(cef_download_handler_t *self, cef_browser_t *browser, cef_download_item_t *item,
                                             cef_download_item_callback_t *callback) {
    if (item && item->is_complete(item)) {
        cef_string_userfree_t full = item->get_full_path(item);
        char *from = utf8_of(full);
        if (full)
            p_cef_string_userfree_utf16_free(full);
        if (from[0] && access(from, F_OK) == 0)
            move_to_downloads(from);
        free(from);
    }
    RELEASE(callback);
    RELEASE(item);
    RELEASE(browser);
}

/* ---- extensions (docs/spatimesh.md, "Extensions") ---------------------------------------- *
 *
 * The window's runtime answers every extension API; here is only the bridge. Chromium as CEF
 * has it gives no world of an extension's own, so content scripts run in the page's world, in a
 * closure: each extension's API is made by a function (crates/spatimesh-shim) that is given a
 * native function of this file's to speak to the window with, and returns the API, which this
 * keeps -- the page never sees either. The extension's own pages get their API as globals. The
 * set reaches each page's process with its browser (`extra_info`) and as a message when it
 * changes; calls come back as process messages; code is run in a frame when the window asks.
 * chrome-extension:// and moz-extension:// are served from the extension's folder by the shim. */

typedef int (*ServeFn)(const char *url, const char *initiator, const char **mime, unsigned char **bytes, size_t *len);
typedef void (*ServeFreeFn)(unsigned char *bytes, size_t len);
static ServeFn serve_fn;
static ServeFreeFn serve_free_fn;

void spatimesh_ext_server(ServeFn serve, ServeFreeFn free_bytes) {
    serve_fn = serve;
    serve_free_fn = free_bytes;
}

/* The bridge's steps, with the process they are in, said where pages' consoles are asked for (tests). */
static void trace(const char *format, ...) {
    if (!getenv("SPATIMESH_CONSOLE"))
        return;
    va_list args;
    va_start(args, format);
    fprintf(stderr, "spatimesh-cef: process %d: ", (int)getpid());
    vfprintf(stderr, format, args);
    fputc('\n', stderr);
    va_end(args);
}

/* No worlds of the extensions' own: the API comes as functions (spatimesh-shim). */
const char *spatimesh_ext_bridge(void) {
    return "";
}

/* The set, as the pages' processes are given it:
 *   [[id, its pages' prefix, content boot, page boot, [[run_at, all_frames, main_world, guard, js, css], ...]], ...] */
static cef_list_value_t *ext_set;
static size_t last_ext;

static void list_set_text(cef_list_value_t *list, size_t index, const char *utf8) {
    cef_string_t s;
    memset(&s, 0, sizeof s);
    set_text(&s, utf8);
    list->set_string(list, index, &s);
    clear_text(&s);
}

static char *list_text(cef_list_value_t *list, size_t index) {
    cef_string_userfree_t s = list->get_string(list, index);
    char *out = utf8_of(s);
    if (s)
        p_cef_string_userfree_utf16_free(s);
    return out;
}

static cef_process_message_t *message_named(const char *name) {
    cef_string_t n;
    memset(&n, 0, sizeof n);
    set_text(&n, name);
    cef_process_message_t *m = p_cef_process_message_create(&n);
    clear_text(&n);
    return m;
}

static int message_is(cef_process_message_t *m, const char *name) {
    cef_string_userfree_t n = m->get_name(m);
    char *text = utf8_of(n);
    if (n)
        p_cef_string_userfree_utf16_free(n);
    int same = strcmp(text, name) == 0;
    free(text);
    return same;
}

void spatimesh_ext_clear(Engine *e) {
    RELEASE(ext_set);
    ext_set = p_cef_list_value_create();
}

void spatimesh_ext_add(Engine *e, const char *id, const char *scheme, const char *world, const char *content_boot, const char *page_boot) {
    if (!ext_set)
        spatimesh_ext_clear(e);
    cef_list_value_t *x = p_cef_list_value_create();
    char prefix[600];
    snprintf(prefix, sizeof prefix, "%s://%s/", scheme, id);
    list_set_text(x, 0, id);
    list_set_text(x, 1, prefix);
    list_set_text(x, 2, content_boot);
    list_set_text(x, 3, page_boot);
    x->set_list(x, 4, p_cef_list_value_create());
    ext_set->set_list(ext_set, ext_set->get_size(ext_set), x);
}

void spatimesh_ext_entry(Engine *e, const char *id, int run_at, int all_frames, int main_world, const char *allow, const char *block, const char *guard, int exact) {
    if (!ext_set)
        return;
    size_t n = ext_set->get_size(ext_set);
    for (size_t i = 0; i < n; i++) {
        cef_list_value_t *x = ext_set->get_list(ext_set, i);
        char *xid = list_text(x, 0);
        if (strcmp(xid, id) == 0) {
            cef_list_value_t *entries = x->get_list(x, 4);
            cef_list_value_t *entry = p_cef_list_value_create();
            entry->set_int(entry, 0, run_at);
            entry->set_bool(entry, 1, all_frames);
            entry->set_bool(entry, 2, main_world);
            /* Which pages: the guard, always (Chromium's own patterns are not reachable here). */
            list_set_text(entry, 3, guard && *guard ? guard : "true");
            list_set_text(entry, 4, "");
            list_set_text(entry, 5, "");
            entries->set_list(entries, entries->get_size(entries), entry);
            RELEASE(entries);
            last_ext = i;
        }
        free(xid);
        RELEASE(x);
    }
}

/* Code added to the last entry's JavaScript (index 4) or style sheet (index 5). */
static void ext_append(size_t index, const char *code) {
    if (!ext_set || last_ext >= ext_set->get_size(ext_set))
        return;
    cef_list_value_t *x = ext_set->get_list(ext_set, last_ext);
    cef_list_value_t *entries = x->get_list(x, 4);
    size_t count = entries->get_size(entries);
    if (count) {
        cef_list_value_t *entry = entries->get_list(entries, count - 1);
        char *was = list_text(entry, index);
        size_t len = strlen(was) + strlen(code) + 4;
        char *both = malloc(len);
        snprintf(both, len, "%s%s%s", was, *was ? "\n;\n" : "", code);
        list_set_text(entry, index, both);
        free(both);
        free(was);
        RELEASE(entry);
    }
    RELEASE(entries);
    RELEASE(x);
}

void spatimesh_ext_js(Engine *e, const char *code) {
    ext_append(4, code);
}

void spatimesh_ext_css(Engine *e, const char *code) {
    ext_append(5, code);
}

/* The set to a browser's pages' process. */
static void send_set(cef_browser_t *browser) {
    if (!ext_set || !browser)
        return;
    cef_frame_t *frame = browser->get_main_frame(browser);
    if (!frame)
        return;
    cef_process_message_t *m = message_named("spatimesh-ext");
    cef_list_value_t *args = m->get_argument_list(m);
    args->set_list(args, 0, ext_set->copy(ext_set));
    RELEASE(args);
    frame->send_process_message(frame, PID_RENDERER, m);
    RELEASE(frame);
}

/* The set is whole: every tab's pages have it from their next load (new tabs with their browser). */
void spatimesh_ext_done(Engine *e) {
    for (Tab *t = tabs; t; t = t->next)
        send_set(t->browser);
}

/* What a new browser's process is given: the set. */
static cef_dictionary_value_t *extra_for_browser(void) {
    if (!ext_set)
        return NULL;
    cef_dictionary_value_t *extra = p_cef_dictionary_value_create();
    cef_string_t key;
    memset(&key, 0, sizeof key);
    set_text(&key, "ext");
    extra->set_list(extra, &key, ext_set->copy(ext_set));
    clear_text(&key);
    return extra;
}

static void send_run(cef_frame_t *frame, const char *world, const char *code, uint64_t request, int boot, int reply) {
    cef_process_message_t *m = message_named("spatimesh-run");
    cef_list_value_t *args = m->get_argument_list(m);
    list_set_text(args, 0, world ? world : "");
    list_set_text(args, 1, code ? code : "");
    args->set_double(args, 2, (double)request);
    args->set_bool(args, 3, boot);
    args->set_bool(args, 4, reply);
    RELEASE(args);
    frame->send_process_message(frame, PID_RENDERER, m);
}

/* Code run in a tab's top frame (and its other frames), in `world` ("" the page's own; an
 * extension's: in its closure, its API set up first if `boot`); the top frame's result told. */
void spatimesh_run_script(Engine *e, uint64_t id, const char *world, int all_frames, const char *code, uint64_t request, int boot) {
    Tab *t = find(id);
    cef_frame_t *main = t && t->browser ? t->browser->get_main_frame(t->browser) : NULL;
    if (!main) {
        if (request)
            push(EV_SCRIPT_RESULT, id, (double)request, 0, 0, "No such tab", NULL);
        return;
    }
    send_run(main, world, code, request, boot, request != 0);
    if (all_frames) {
        cef_string_list_t ids = p_cef_string_list_alloc();
        t->browser->get_frame_identifiers(t->browser, ids);
        size_t n = p_cef_string_list_size(ids);
        cef_string_userfree_t main_id = main->get_identifier(main);
        char *main_text = utf8_of(main_id);
        if (main_id)
            p_cef_string_userfree_utf16_free(main_id);
        for (size_t i = 0; i < n; i++) {
            cef_string_t one;
            memset(&one, 0, sizeof one);
            if (!p_cef_string_list_value(ids, i, &one))
                continue;
            char *text = utf8_of(&one);
            if (strcmp(text, main_text) != 0) {
                cef_frame_t *frame = t->browser->get_frame_by_identifier(t->browser, &one);
                if (frame) {
                    send_run(frame, world, code, 0, boot, 0);
                    RELEASE(frame);
                }
            }
            free(text);
            clear_text(&one);
        }
        free(main_text);
        p_cef_string_list_free(ids);
    }
    RELEASE(main);
}

/* -- the browser's side: what pages' processes say -- */

static int CEF_CALLBACK on_process_message(cef_client_t *self, cef_browser_t *browser, cef_frame_t *frame, cef_process_id_t source,
                                           cef_process_message_t *message) {
    Tab *t = tab_of(browser);
    int handled = 0;
    cef_list_value_t *args = message->get_argument_list(message);
    if (t && message_is(message, "spatimesh-call")) {
        char *json = list_text(args, 0);
        push(EV_EXTENSION, t->id, 0, 0, 0, json, NULL);
        free(json);
        handled = 1;
    } else if (t && message_is(message, "spatimesh-result")) {
        char *text = list_text(args, 2);
        push(EV_SCRIPT_RESULT, t->id, args->get_double(args, 0), args->get_bool(args, 1), 0, text, NULL);
        free(text);
        handled = 1;
    }
    RELEASE(args);
    RELEASE(message);
    RELEASE(frame);
    RELEASE(browser);
    return handled;
}

/* -- the extensions' files: chrome-extension:// and moz-extension://, on Chromium's IO thread -- */

static int is_extension_url(const char *url) {
    return strncmp(url, "chrome-extension://", 19) == 0 || strncmp(url, "moz-extension://", 16) == 0;
}

/* One file being given to Chromium. */
typedef struct {
    cef_resource_handler_t handler;
    atomic_int refs;
    unsigned char *bytes;
    size_t len, offset;
    int status;
    char mime[96];
    char charset[32];
} Served;

/* One request for one: who asked. */
typedef struct {
    cef_resource_request_handler_t handler;
    atomic_int refs;
    char *initiator;
} Asked;

#define REFCOUNTED(Type, field, freeing)                                                          \
    static void CEF_CALLBACK Type##_add_ref(cef_base_ref_counted_t *b) {                          \
        atomic_fetch_add(&((Type *)b)->refs, 1);                                                  \
    }                                                                                             \
    static int CEF_CALLBACK Type##_release(cef_base_ref_counted_t *b) {                           \
        Type *field = (Type *)b;                                                                  \
        if (atomic_fetch_sub(&field->refs, 1) == 1) {                                             \
            freeing;                                                                              \
            return 1;                                                                             \
        }                                                                                         \
        return 0;                                                                                 \
    }                                                                                             \
    static int CEF_CALLBACK Type##_has_one_ref(cef_base_ref_counted_t *b) {                       \
        return atomic_load(&((Type *)b)->refs) == 1;                                              \
    }                                                                                             \
    static int CEF_CALLBACK Type##_has_at_least_one_ref(cef_base_ref_counted_t *b) {              \
        return atomic_load(&((Type *)b)->refs) >= 1;                                              \
    }

REFCOUNTED(Served, s, (free(s->bytes), free(s)))
REFCOUNTED(Asked, a, (free(a->initiator), free(a)))

#define COUNTED(object, Type, struct_size)                                                        \
    do {                                                                                          \
        (object)->handler.base.size = (struct_size);                                              \
        (object)->handler.base.add_ref = Type##_add_ref;                                          \
        (object)->handler.base.release = Type##_release;                                          \
        (object)->handler.base.has_one_ref = Type##_has_one_ref;                                  \
        (object)->handler.base.has_at_least_one_ref = Type##_has_at_least_one_ref;                \
        atomic_init(&(object)->refs, 1);                                                          \
    } while (0)

static int CEF_CALLBACK served_open(cef_resource_handler_t *self, cef_request_t *request, int *handle_request, cef_callback_t *callback) {
    *handle_request = 1;
    RELEASE(request);
    RELEASE(callback);
    return 1;
}

static void CEF_CALLBACK served_headers(cef_resource_handler_t *self, cef_response_t *response, int64_t *length, cef_string_t *redirect) {
    Served *s = (Served *)self;
    response->set_status(response, s->status);
    cef_string_t text, name, value;
    memset(&text, 0, sizeof text);
    memset(&name, 0, sizeof name);
    memset(&value, 0, sizeof value);
    set_text(&text, s->mime);
    response->set_mime_type(response, &text);
    clear_text(&text);
    if (s->charset[0]) {
        set_text(&text, s->charset);
        response->set_charset(response, &text);
        clear_text(&text);
    }
    /* Who may read it was decided when it was served (only what is web accessible, to pages
     * not the extension's own). */
    set_text(&name, "Access-Control-Allow-Origin");
    set_text(&value, "*");
    response->set_header_by_name(response, &name, &value, 1);
    clear_text(&name);
    clear_text(&value);
    *length = (int64_t)s->len;
    RELEASE(response);
}

static int CEF_CALLBACK served_read(cef_resource_handler_t *self, void *out, int want, int *got, cef_resource_read_callback_t *callback) {
    Served *s = (Served *)self;
    size_t left = s->len - s->offset;
    size_t n = left < (size_t)(want > 0 ? want : 0) ? left : (size_t)(want > 0 ? want : 0);
    if (n)
        memcpy(out, s->bytes + s->offset, n);
    s->offset += n;
    *got = (int)n;
    RELEASE(callback);
    return n > 0;
}

static int CEF_CALLBACK served_skip(cef_resource_handler_t *self, int64_t want, int64_t *skipped, cef_resource_skip_callback_t *callback) {
    Served *s = (Served *)self;
    size_t left = s->len - s->offset;
    size_t n = want > 0 && (size_t)want < left ? (size_t)want : left;
    s->offset += n;
    *skipped = (int64_t)n;
    RELEASE(callback);
    return 1;
}

static void CEF_CALLBACK served_cancel(cef_resource_handler_t *self) {}

static cef_resource_handler_t *CEF_CALLBACK asked_get_resource_handler(cef_resource_request_handler_t *self, cef_browser_t *browser,
                                                                       cef_frame_t *frame, cef_request_t *request) {
    Asked *a = (Asked *)self;
    cef_string_userfree_t u = request->get_url(request);
    char *url = utf8_of(u);
    if (u)
        p_cef_string_userfree_utf16_free(u);
    Served *s = calloc(1, sizeof *s);
    COUNTED(s, Served, sizeof(cef_resource_handler_t));
    s->handler.open = served_open;
    s->handler.get_response_headers = served_headers;
    s->handler.read = served_read;
    s->handler.skip = served_skip;
    s->handler.cancel = served_cancel;
    const char *mime = "text/plain";
    unsigned char *bytes = NULL;
    size_t len = 0;
    s->status = serve_fn ? serve_fn(url, a->initiator ? a->initiator : "", &mime, &bytes, &len) : 404;
    if (len) {
        s->bytes = malloc(len);
        memcpy(s->bytes, bytes, len);
        s->len = len;
    }
    if (serve_free_fn)
        serve_free_fn(bytes, len);
    trace("served %s to %s: %d, %zu bytes", url, a->initiator ? a->initiator : "the browser", s->status, len);
    /* "text/html; charset=utf-8": the type, and its charset apart. */
    snprintf(s->mime, sizeof s->mime, "%s", mime);
    char *semi = strchr(s->mime, ';');
    if (semi) {
        const char *cs = strstr(semi, "charset=");
        if (cs)
            snprintf(s->charset, sizeof s->charset, "%s", cs + 8);
        *semi = 0;
    }
    free(url);
    RELEASE(request);
    RELEASE(frame);
    RELEASE(browser);
    return &s->handler;
}

static cef_resource_request_handler_t *CEF_CALLBACK get_resource_request_handler(
    cef_request_handler_t *self, cef_browser_t *browser, cef_frame_t *frame, cef_request_t *request, int is_navigation, int is_download,
    const cef_string_t *request_initiator, int *disable_default_handling) {
    cef_resource_request_handler_t *out = NULL;
    cef_string_userfree_t u = request->get_url(request);
    char *url = utf8_of(u);
    if (u)
        p_cef_string_userfree_utf16_free(u);
    if (is_extension_url(url)) {
        Asked *a = calloc(1, sizeof *a);
        COUNTED(a, Asked, sizeof(cef_resource_request_handler_t));
        a->handler.get_resource_handler = asked_get_resource_handler;
        a->initiator = request_initiator && request_initiator->length ? utf8_of(request_initiator) : NULL;
        out = &a->handler;
    }
    free(url);
    RELEASE(request);
    RELEASE(frame);
    RELEASE(browser);
    return out;
}

/* Firefox's extensions' scheme, known to every process as a standard one (Chromium knows
 * chrome-extension:// itself). */
static void CEF_CALLBACK on_register_schemes(cef_app_t *self, cef_scheme_registrar_t *registrar) {
    cef_string_t name;
    memset(&name, 0, sizeof name);
    set_text(&name, "moz-extension");
    registrar->add_custom_scheme(registrar, &name,
                                 CEF_SCHEME_OPTION_STANDARD | CEF_SCHEME_OPTION_SECURE | CEF_SCHEME_OPTION_CORS_ENABLED |
                                     CEF_SCHEME_OPTION_FETCH_ENABLED);
    clear_text(&name);
}

/* -- the pages' processes -- */

/* The set this process was given. */
static cef_list_value_t *renderer_set;

/* An extension's API as made for a frame's page world, kept for code to be run in its name. */
typedef struct Made {
    char *ext;
    cef_v8context_t *context;
    cef_v8value_t *api;
    /* {__spatimeshExt: api}: what such code is given as `globalThis`. */
    cef_v8value_t *holder;
    struct Made *next;
} Made;
static Made *made;

/* Content scripts waiting for their page to be parsed. */
typedef struct Later {
    cef_v8context_t *context;
    char *ext;
    char *js;
    char *css;
    int main_world;
    struct Later *next;
} Later;
static Later *later;

static cef_v8handler_t v8_handler;
static cef_render_process_handler_t render_process_handler;

static cef_v8value_t *v8_text(const char *utf8) {
    cef_string_t s;
    memset(&s, 0, sizeof s);
    set_text(&s, utf8);
    cef_v8value_t *v = p_cef_v8value_create_string(&s);
    clear_text(&s);
    return v;
}

static cef_v8value_t *v8_key(cef_v8value_t *object, const char *key) {
    cef_string_t k;
    memset(&k, 0, sizeof k);
    set_text(&k, key);
    cef_v8value_t *v = object->get_value_bykey(object, &k);
    clear_text(&k);
    return v;
}

/* JavaScript evaluated in a context (entered): its value, or NULL (what went wrong logged). */
static cef_v8value_t *v8_eval(cef_v8context_t *context, const char *code, const char *where, char **error) {
    cef_string_t c, u;
    memset(&c, 0, sizeof c);
    memset(&u, 0, sizeof u);
    set_text(&c, code);
    set_text(&u, where ? where : "");
    cef_v8value_t *value = NULL;
    cef_v8exception_t *exception = NULL;
    context->eval(context, &c, &u, 1, &value, &exception);
    if (exception) {
        cef_string_userfree_t m = exception->get_message(exception);
        char *text = utf8_of(m);
        if (m)
            p_cef_string_userfree_utf16_free(m);
        if (error)
            *error = text;
        else {
            fprintf(stderr, "spatimesh-cef: %s: %s\n", where ? where : "a script", text);
            free(text);
        }
        RELEASE(exception);
    }
    clear_text(&c);
    clear_text(&u);
    return value;
}

/* fn(args) in the entered context: its value, or NULL with what it threw said. */
static cef_v8value_t *v8_call(cef_v8value_t *fn, size_t argc, cef_v8value_t *const *args, const char *what) {
    cef_v8value_t *value = fn->execute_function(fn, NULL, argc, args);
    if (!value && fn->has_exception(fn)) {
        cef_v8exception_t *exception = fn->get_exception(fn);
        if (exception) {
            cef_string_userfree_t m = exception->get_message(exception);
            char *text = utf8_of(m);
            if (m)
                p_cef_string_userfree_utf16_free(m);
            fprintf(stderr, "spatimesh-cef: %s: %s (line %d)\n", what, text, exception->get_line_number(exception));
            free(text);
            RELEASE(exception);
        }
        fn->clear_exception(fn);
    }
    return value;
}

static int same_context(cef_v8context_t *a, cef_v8context_t *b) {
    return a && b && a->is_same(a, SHARE(b));
}

/* `v`, or undefined where there is none (CEF takes no NULL among a function's arguments). */
static cef_v8value_t *defined(cef_v8value_t *v) {
    if (v && !v->is_null(v))
        return v;
    RELEASE(v);
    return p_cef_v8value_create_undefined();
}

static Made *made_for(cef_v8context_t *context, const char *ext) {
    for (Made *m = made; m; m = m->next)
        if (strcmp(m->ext, ext) == 0 && same_context(m->context, context))
            return m;
    return NULL;
}

/* The extension's entry in the set, by its id (a reference, or NULL). */
static cef_list_value_t *ext_entry(const char *id) {
    if (!renderer_set)
        return NULL;
    size_t n = renderer_set->get_size(renderer_set);
    for (size_t i = 0; i < n; i++) {
        cef_list_value_t *x = renderer_set->get_list(renderer_set, i);
        char *xid = list_text(x, 0);
        int same = strcmp(xid, id) == 0;
        free(xid);
        if (same)
            return x;
        RELEASE(x);
    }
    return NULL;
}

/* An extension's API in a frame's page world (entered): made by its boot function, handed the
 * native function its calls go through; kept for content scripts (`closed`), or set up as the
 * page's globals for its own pages. */
static Made *boot_api(cef_v8context_t *context, const char *ext, const char *boot, int keep) {
    Made *m = keep ? made_for(context, ext) : NULL;
    if (m)
        return m;
    cef_v8value_t *fn = v8_eval(context, boot, "spatimesh-extension-api", NULL);
    if (!fn)
        return NULL;
    char name[600];
    snprintf(name, sizeof name, "spatimeshPost:%s", ext);
    cef_string_t n;
    memset(&n, 0, sizeof n);
    set_text(&n, name);
    cef_v8value_t *args[1] = {p_cef_v8value_create_function(&n, &v8_handler)};
    clear_text(&n);
    cef_v8value_t *api = v8_call(fn, 1, args, "the extension's API");
    RELEASE(fn);
    trace("%s's API %s", ext, api ? "set up" : "not set up");
    if (!api)
        return NULL;
    if (!keep) {
        RELEASE(api);
        return NULL;
    }
    m = calloc(1, sizeof *m);
    m->ext = strdup(ext);
    context->base.add_ref(&context->base);
    m->context = context;
    m->api = api;
    m->holder = p_cef_v8value_create_object(NULL, NULL);
    cef_string_t k;
    memset(&k, 0, sizeof k);
    set_text(&k, "__spatimeshExt");
    m->holder->set_value_bykey(m->holder, &k, SHARE(api), V8_PROPERTY_ATTRIBUTE_NONE);
    clear_text(&k);
    m->next = made;
    made = m;
    return m;
}

/* One entry's content scripts and style sheet, now, in a frame's page world (entered). */
static void inject(cef_v8context_t *context, const char *ext, const char *js, const char *css, int main_world) {
    if (css && *css) {
        cef_v8value_t *add = v8_eval(context,
                                     "(function(css){try{var s=new CSSStyleSheet();s.replaceSync(css);"
                                     "document.adoptedStyleSheets=document.adoptedStyleSheets.concat([s]);}catch(e){}})",
                                     "spatimesh-content-style", NULL);
        if (add) {
            cef_v8value_t *args[1] = {v8_text(css)};
            cef_v8value_t *r = v8_call(add, 1, args, "a content script's style sheet");
            RELEASE(r);
            RELEASE(add);
        }
    }
    if (!js || !*js)
        return;
    if (main_world) {
        cef_v8value_t *r = v8_eval(context, js, "spatimesh-content-script", NULL);
        RELEASE(r);
        return;
    }
    cef_list_value_t *x = ext_entry(ext);
    if (!x)
        return;
    char *boot = list_text(x, 2);
    RELEASE(x);
    Made *m = boot_api(context, ext, boot, 1);
    free(boot);
    if (!m)
        return;
    /* The extension's files in one closure: chrome and browser its own, globalThis the page's
     * with them. */
    size_t len = strlen(js) + 96;
    char *code = malloc(len);
    snprintf(code, len, "(function(chrome,browser,globalThis){\n%s\n})", js);
    cef_v8value_t *fn = v8_eval(context, code, "spatimesh-content-script", NULL);
    free(code);
    if (!fn)
        return;
    cef_v8value_t *args[3] = {defined(v8_key(m->api, "chrome")), defined(v8_key(m->api, "browser")), defined(v8_key(m->api, "global"))};
    trace("%s's content scripts run", ext);
    cef_v8value_t *r = v8_call(fn, 3, args, "a content script");
    RELEASE(r);
    RELEASE(fn);
}

/* The page's DOM is parsed: what waited for it, in this context. */
static void run_later(cef_v8context_t *context) {
    Later **at = &later;
    while (*at) {
        Later *l = *at;
        if (same_context(l->context, context)) {
            *at = l->next;
            inject(context, l->ext, l->js, l->css, l->main_world);
            RELEASE(l->context);
            free(l->ext);
            free(l->js);
            free(l->css);
            free(l);
        } else {
            at = &l->next;
        }
    }
}

static int CEF_CALLBACK v8_execute(cef_v8handler_t *self, const cef_string_t *name, cef_v8value_t *object, size_t argc,
                                   cef_v8value_t *const *argv, cef_v8value_t **retval, cef_string_t *exception) {
    char *fn = utf8_of(name);
    cef_v8context_t *context = p_cef_v8context_get_current_context();
    if (strncmp(fn, "spatimeshPost:", 14) == 0 && argc >= 1 && argv[0] && argv[0]->is_string(argv[0])) {
        cef_string_userfree_t json = argv[0]->get_string_value(argv[0]);
        cef_frame_t *frame = context ? context->get_frame(context) : NULL;
        if (frame) {
            cef_process_message_t *m = message_named("spatimesh-call");
            cef_list_value_t *args = m->get_argument_list(m);
            args->set_string(args, 0, json);
            RELEASE(args);
            frame->send_process_message(frame, PID_BROWSER, m);
            RELEASE(frame);
        }
        if (json)
            p_cef_string_userfree_utf16_free(json);
    } else if (strcmp(fn, "spatimeshLater") == 0 && context) {
        run_later(context);
    }
    RELEASE(context);
    for (size_t i = 0; i < argc; i++)
        RELEASE(argv[i]);
    RELEASE(object);
    free(fn);
    return 1;
}

static void CEF_CALLBACK on_browser_created(cef_render_process_handler_t *self, cef_browser_t *browser, cef_dictionary_value_t *extra) {
    if (extra) {
        cef_string_t key;
        memset(&key, 0, sizeof key);
        set_text(&key, "ext");
        if (extra->has_key(extra, &key)) {
            cef_list_value_t *l = extra->get_list(extra, &key);
            if (l) {
                RELEASE(renderer_set);
                renderer_set = l->copy(l);
                RELEASE(l);
            }
        }
        clear_text(&key);
    }
    trace("browser %d made, with %d extensions", browser->get_identifier(browser), renderer_set ? (int)renderer_set->get_size(renderer_set) : -1);
    RELEASE(extra);
    RELEASE(browser);
}

/* A frame's page world is made: the extensions' API for their own pages, and the content
 * scripts that want this page, now or once it is parsed. */
static void CEF_CALLBACK on_context_created(cef_render_process_handler_t *self, cef_browser_t *browser, cef_frame_t *frame,
                                            cef_v8context_t *context) {
    if (renderer_set && renderer_set->get_size(renderer_set)) {
        cef_string_userfree_t u = frame->get_url(frame);
        char *url = utf8_of(u);
        if (u)
            p_cef_string_userfree_utf16_free(u);
        int top = frame->is_main(frame);
        trace("a page world made for %s (%s frame)", url, top ? "the top" : "another");
        context->enter(context);
        int waiting = 0;
        size_t n = renderer_set->get_size(renderer_set);
        for (size_t i = 0; i < n; i++) {
            cef_list_value_t *x = renderer_set->get_list(renderer_set, i);
            char *id = list_text(x, 0);
            char *prefix = list_text(x, 1);
            if (strncmp(url, prefix, strlen(prefix)) == 0) {
                char *page = list_text(x, 3);
                boot_api(context, id, page, 0);
                free(page);
            } else if (strncmp(url, "http", 4) == 0 || strncmp(url, "file:", 5) == 0 || strncmp(url, "ftp:", 4) == 0) {
                cef_list_value_t *entries = x->get_list(x, 4);
                size_t count = entries->get_size(entries);
                for (size_t j = 0; j < count; j++) {
                    cef_list_value_t *entry = entries->get_list(entries, j);
                    int run_at = entry->get_int(entry, 0);
                    int all_frames = entry->get_bool(entry, 1);
                    int main_world = entry->get_bool(entry, 2);
                    char *guard = list_text(entry, 3);
                    cef_v8value_t *wanted = (top || all_frames) ? v8_eval(context, guard, "spatimesh-content-guard", NULL) : NULL;
                    int yes = wanted && wanted->is_bool(wanted) && wanted->get_bool_value(wanted);
                    trace("%s's content scripts (%zu, at %d) %s here", id, j, run_at, yes ? "are wanted" : "are not wanted");
                    if (yes) {
                        char *js = list_text(entry, 4);
                        char *css = list_text(entry, 5);
                        /* Style sheets before anything is shown, as Chrome puts them in whenever
                         * the scripts go. */
                        inject(context, id, NULL, css, main_world);
                        if (run_at == 0) {
                            inject(context, id, js, NULL, main_world);
                            free(js);
                        } else {
                            Later *l = calloc(1, sizeof *l);
                            context->base.add_ref(&context->base);
                            l->context = context;
                            l->ext = strdup(id);
                            l->js = js;
                            l->css = NULL;
                            l->main_world = main_world;
                            l->next = later;
                            later = l;
                            waiting = 1;
                        }
                        free(css);
                    }
                    RELEASE(wanted);
                    free(guard);
                    RELEASE(entry);
                }
                RELEASE(entries);
            }
            free(id);
            free(prefix);
            RELEASE(x);
        }
        if (waiting) {
            cef_v8value_t *when = v8_eval(context,
                                          "(function(cb){if(document.readyState==='loading')"
                                          "document.addEventListener('DOMContentLoaded',function(){cb();},{once:true});else cb();})",
                                          "spatimesh-content-later", NULL);
            if (when) {
                cef_string_t name;
                memset(&name, 0, sizeof name);
                set_text(&name, "spatimeshLater");
                cef_v8value_t *args[1] = {p_cef_v8value_create_function(&name, &v8_handler)};
                clear_text(&name);
                cef_v8value_t *r = v8_call(when, 1, args, "waiting for the page to be parsed");
                RELEASE(r);
                RELEASE(when);
            }
        }
        context->exit(context);
        free(url);
    }
    RELEASE(context);
    RELEASE(frame);
    RELEASE(browser);
}

static void CEF_CALLBACK on_context_released(cef_render_process_handler_t *self, cef_browser_t *browser, cef_frame_t *frame,
                                             cef_v8context_t *context) {
    Made **at = &made;
    while (*at) {
        Made *m = *at;
        if (same_context(m->context, context)) {
            *at = m->next;
            RELEASE(m->holder);
            RELEASE(m->api);
            RELEASE(m->context);
            free(m->ext);
            free(m);
        } else {
            at = &m->next;
        }
    }
    Later **wait = &later;
    while (*wait) {
        Later *l = *wait;
        if (same_context(l->context, context)) {
            *wait = l->next;
            RELEASE(l->context);
            free(l->ext);
            free(l->js);
            free(l->css);
            free(l);
        } else {
            wait = &l->next;
        }
    }
    RELEASE(context);
    RELEASE(frame);
    RELEASE(browser);
}

/* Code the window asked to be run in this frame; the result told back if asked. */
static void run_in_frame(cef_frame_t *frame, const char *world, const char *code, double request, int boot, int reply) {
    cef_v8context_t *context = frame->get_v8context(frame);
    char *error = NULL;
    char *answer = NULL;
    if (!context) {
        error = strdup("The frame has no page");
    } else {
        context->enter(context);
        cef_v8value_t *result = NULL;
        if (!*world) {
            result = v8_eval(context, code, "spatimesh-script", &error);
        } else {
            const char *ext = strncmp(world, "spatimesh-", 10) == 0 ? world + 10 : world;
            Made *m = made_for(context, ext);
            if (!m && boot) {
                cef_list_value_t *x = ext_entry(ext);
                if (x) {
                    char *b = list_text(x, 2);
                    m = boot_api(context, ext, b, 1);
                    free(b);
                    RELEASE(x);
                }
            }
            if (!m) {
                if (reply)
                    error = strdup("The extension has no API in that frame");
            } else {
                size_t len = strlen(code) + 64;
                char *wrapped = malloc(len);
                snprintf(wrapped, len, "(function(globalThis){%s\n})", code);
                cef_v8value_t *fn = v8_eval(context, wrapped, "spatimesh-script", &error);
                free(wrapped);
                if (fn) {
                    cef_v8value_t *args[1] = {SHARE(m->holder)};
                    result = v8_call(fn, 1, args, "code run for an extension");
                    RELEASE(fn);
                }
            }
        }
        if (reply && !error) {
            cef_v8value_t *json = v8_eval(context, "(function(v){try{var s=JSON.stringify(v);return s===undefined?'null':s;}catch(e){return 'null';}})",
                                          "spatimesh-result", NULL);
            if (json) {
                /* The result goes with the call. */
                cef_v8value_t *args[1] = {result ? result : p_cef_v8value_create_undefined()};
                result = NULL;
                cef_v8value_t *text = v8_call(json, 1, args, "a result as JSON");
                if (text && text->is_string(text)) {
                    cef_string_userfree_t s = text->get_string_value(text);
                    answer = utf8_of(s);
                    if (s)
                        p_cef_string_userfree_utf16_free(s);
                }
                RELEASE(text);
                RELEASE(json);
            }
        }
        RELEASE(result);
        context->exit(context);
        RELEASE(context);
    }
    if (reply) {
        cef_process_message_t *m = message_named("spatimesh-result");
        cef_list_value_t *args = m->get_argument_list(m);
        args->set_double(args, 0, request);
        args->set_bool(args, 1, error == NULL);
        list_set_text(args, 2, error ? error : answer ? answer : "null");
        RELEASE(args);
        frame->send_process_message(frame, PID_BROWSER, m);
    }
    free(error);
    free(answer);
}

static int CEF_CALLBACK on_renderer_message(cef_render_process_handler_t *self, cef_browser_t *browser, cef_frame_t *frame,
                                            cef_process_id_t source, cef_process_message_t *message) {
    int handled = 0;
    cef_list_value_t *args = message->get_argument_list(message);
    if (message_is(message, "spatimesh-ext")) {
        cef_list_value_t *l = args->get_list(args, 0);
        if (l) {
            RELEASE(renderer_set);
            renderer_set = l->copy(l);
            RELEASE(l);
        }
        handled = 1;
        trace("the set changed: %d extensions", renderer_set ? (int)renderer_set->get_size(renderer_set) : -1);
    } else if (message_is(message, "spatimesh-run") && frame) {
        char *world = list_text(args, 0);
        char *code = list_text(args, 1);
        run_in_frame(frame, world, code, args->get_double(args, 2), args->get_bool(args, 3), args->get_bool(args, 4));
        free(world);
        free(code);
        handled = 1;
    }
    RELEASE(args);
    RELEASE(message);
    RELEASE(frame);
    RELEASE(browser);
    return handled;
}

static cef_render_process_handler_t *CEF_CALLBACK get_render_process_handler(cef_app_t *self) {
    return &render_process_handler;
}

static void make_handlers(void) {
    static int made;
    if (made)
        return;
    made = 1;
    kept(&app.base, sizeof app);
    app.on_before_command_line_processing = on_command_line;
    app.get_browser_process_handler = get_process_handler;
    app.on_register_custom_schemes = on_register_schemes;
    app.get_render_process_handler = get_render_process_handler;

    kept(&render_process_handler.base, sizeof render_process_handler);
    render_process_handler.on_browser_created = on_browser_created;
    render_process_handler.on_context_created = on_context_created;
    render_process_handler.on_context_released = on_context_released;
    render_process_handler.on_process_message_received = on_renderer_message;

    kept(&v8_handler.base, sizeof v8_handler);
    v8_handler.execute = v8_execute;

    kept(&process_handler.base, sizeof process_handler);
    process_handler.on_schedule_message_pump_work = on_schedule_work;

    kept(&client.base, sizeof client);
    client.get_render_handler = get_render_handler;
    client.get_life_span_handler = get_life_span_handler;
    client.get_load_handler = get_load_handler;
    client.get_display_handler = get_display_handler;
    client.get_jsdialog_handler = get_jsdialog_handler;
    client.get_request_handler = get_request_handler;
    client.get_context_menu_handler = get_context_menu_handler;
    client.get_dialog_handler = get_dialog_handler;
    client.on_process_message_received = on_process_message;
    client.get_download_handler = get_download_handler;

    kept(&render_handler.base, sizeof render_handler);
    render_handler.get_view_rect = get_view_rect;
    render_handler.get_screen_info = get_screen_info;
    render_handler.on_popup_show = on_popup_show;
    render_handler.on_popup_size = on_popup_size;
    render_handler.on_paint = on_paint;

    kept(&life_span_handler.base, sizeof life_span_handler);
    life_span_handler.on_before_popup = on_before_popup;
    life_span_handler.on_after_created = on_after_created;
    life_span_handler.do_close = do_close;
    life_span_handler.on_before_close = on_before_close;

    kept(&load_handler.base, sizeof load_handler);
    load_handler.on_loading_state_change = on_loading_state_change;
    load_handler.on_load_end = on_load_end;
    load_handler.on_load_error = on_load_error;

    kept(&display_handler.base, sizeof display_handler);
    display_handler.on_address_change = on_address_change;
    display_handler.on_title_change = on_title_change;
    display_handler.on_tooltip = on_tooltip;
    display_handler.on_console_message = on_console_message;
    display_handler.on_loading_progress_change = on_progress;
    display_handler.on_cursor_change = on_cursor_change;

    kept(&jsdialog_handler.base, sizeof jsdialog_handler);
    jsdialog_handler.on_jsdialog = on_jsdialog;
    jsdialog_handler.on_before_unload_dialog = on_before_unload;
    jsdialog_handler.on_reset_dialog_state = on_reset_dialogs;

    kept(&request_handler.base, sizeof request_handler);
    request_handler.on_open_urlfrom_tab = on_open_url_from_tab;
    request_handler.on_render_process_terminated = on_renderer_gone;
    request_handler.get_resource_request_handler = get_resource_request_handler;

    kept(&context_menu_handler.base, sizeof context_menu_handler);
    context_menu_handler.on_before_context_menu = on_before_context_menu;

    kept(&dialog_handler.base, sizeof dialog_handler);
    dialog_handler.on_file_dialog = on_file_dialog;

    kept(&download_handler.base, sizeof download_handler);
    download_handler.on_before_download = on_before_download;
    download_handler.on_download_updated = on_download_updated;
}

/* ---- the interface ------------------------------------------------------------------------- */

/* One of Chromium's own processes (a page's, the GPU's, ...): this program started again by
 * Chromium with --type=...; it returns what the process ends with. */
int spatimesh_cef_subprocess(void) {
    if (!load_cef()) {
        fprintf(stderr, "spatimesh-cef: %s\n", error_text);
        return 1;
    }
    make_handlers();
    cef_main_args_t args = {argc_, argv_};
    int code = p_cef_execute_process(&args, &app, NULL);
    return code < 0 ? 0 : code;
}

const char *spatimesh_version(void) {
    static char version[4400];
    if (!library)
        return "Steam's Chromium (not loaded)";
    snprintf(version, sizeof version, "Chromium %d.%d.%d.%d (CEF %d.%d.%d, %s)", p_cef_version_info(4), p_cef_version_info(5),
             p_cef_version_info(6), p_cef_version_info(7), p_cef_version_info(0), p_cef_version_info(1), p_cef_version_info(2),
             strstr(cef_folder, "Steam") ? "Steam's" : cef_folder);
    return version;
}

static int remove_one(const char *path, const struct stat *st, int flag, struct FTW *ftw) {
    return remove(path);
}

/* A profile's engine: `data` and `cache` are its folders, or NULL for the private profile,
 * which keeps everything in memory. NULL if Chromium cannot start; spatimesh_cef_error says
 * why. */
Engine *spatimesh_new(const char *data, const char *cache) {
    if (started)
        return &the_engine;
    if (!load_cef())
        return NULL;
    make_handlers();
    cef_main_args_t args = {argc_, argv_};
    if (p_cef_execute_process(&args, &app, NULL) >= 0) {
        snprintf(error_text, sizeof error_text, "started as one of Chromium's own processes");
        return NULL;
    }
    wake_fd = eventfd(0, EFD_NONBLOCK | EFD_CLOEXEC);

    cef_settings_t settings;
    memset(&settings, 0, sizeof settings);
    settings.size = sizeof settings;
    /* Chromium's sandbox for the pages' processes, unless asked not to (a machine that does
     * not allow it, as CI's: SPATIMESH_CEF_NO_SANDBOX=1). */
    settings.no_sandbox = getenv("SPATIMESH_CEF_NO_SANDBOX") ? 1 : 0;
    settings.multi_threaded_message_loop = 0;
    settings.external_message_pump = 1;
    settings.windowless_rendering_enabled = 1;
    settings.background_color = 0xFFFFFFFFu;
    settings.log_severity = getenv("SPATIMESH_CEF_LOG") ? LOGSEVERITY_INFO : LOGSEVERITY_ERROR;
    char locales[4300];
    snprintf(locales, sizeof locales, "%s/locales", cef_folder);
    set_text(&settings.resources_dir_path, cef_folder);
    set_text(&settings.locales_dir_path, locales);
    if (data && cache) {
        set_text(&settings.root_cache_path, data);
        set_text(&settings.cache_path, data);
        settings.persist_session_cookies = 1;
        cache_folder = strdup(cache);
    } else {
        /* Chromium wants a folder of its own all the same: a new one, in memory where the
         * system has a runtime folder, removed at the end; the pages' data is never in it. */
        const char *runtime = getenv("XDG_RUNTIME_DIR");
        char folder[4096];
        snprintf(folder, sizeof folder, "%s/spatimesh-private-XXXXXX", runtime && *runtime ? runtime : "/tmp");
        if (!mkdtemp(folder)) {
            snprintf(error_text, sizeof error_text, "no folder for the private profile: %s", folder);
            return NULL;
        }
        private_folder = strdup(folder);
        set_text(&settings.root_cache_path, private_folder);
    }
    int ok = p_cef_initialize(&args, &settings, &app, NULL);
    clear_text(&settings.resources_dir_path);
    clear_text(&settings.locales_dir_path);
    clear_text(&settings.root_cache_path);
    clear_text(&settings.cache_path);
    if (!ok) {
        snprintf(error_text, sizeof error_text, "Chromium would not start (what it said is in Spatiand's applications' log)");
        return NULL;
    }
    started = 1;
    return &the_engine;
}

void spatimesh_watch(Engine *e, int fd) {
    watch_fd = fd;
}

int spatimesh_open(Engine *e, uint64_t id, const char *uri, int width, int height, double zoom) {
    if (find(id))
        return 0;
    Tab *t = calloc(1, sizeof *t);
    t->id = id;
    t->width = width > 1 ? width : 1;
    t->height = height > 1 ? height : 1;
    t->zoom = zoom > 0.1 ? zoom : 1.0;
    t->next = tabs;
    tabs = t;

    cef_window_info_t info;
    memset(&info, 0, sizeof info);
    info.windowless_rendering_enabled = 1;
    info.bounds.width = (int)lround(t->width / t->zoom);
    info.bounds.height = (int)lround(t->height / t->zoom);
    cef_browser_settings_t browser_settings;
    memset(&browser_settings, 0, sizeof browser_settings);
    browser_settings.size = sizeof browser_settings;
    browser_settings.windowless_frame_rate = 60;
    browser_settings.background_color = 0xFFFFFFFFu;
    cef_string_t url;
    memset(&url, 0, sizeof url);
    set_text(&url, uri && *uri ? uri : "about:blank");
    creating = t;
    /* The profile's extensions, for the browser's pages' process from its first page on (the
     * set goes with the call). */
    t->browser = p_cef_browser_host_create_browser_sync(&info, &client, &url, &browser_settings, extra_for_browser(), NULL);
    creating = NULL;
    clear_text(&url);
    if (!t->browser) {
        push(EV_CRASHED, id, 0, 0, 0, "Chromium would not open the page", NULL);
        return 1;
    }
    t->browser_id = t->browser->get_identifier(t->browser);
    return 1;
}

void spatimesh_close(Engine *e, uint64_t id) {
    Tab **at = &tabs;
    while (*at && (*at)->id != id)
        at = &(*at)->next;
    Tab *t = *at;
    if (!t)
        return;
    *at = t->next;
    if (t->dialog) {
        RELEASE(t->dialog);
        t->dialog = NULL;
    }
    if (t->browser) {
        cef_browser_host_t *host = t->browser->get_host(t->browser);
        if (host) {
            host->close_browser(host, 1);
            RELEASE(host);
        }
        RELEASE(t->browser);
    }
    forget_choices(t);
    free(t->pixels);
    free(t->popup_pixels);
    free(t);
}

static cef_browser_host_t *host_of(Tab *t) {
    return t && t->browser ? t->browser->get_host(t->browser) : NULL;
}

void spatimesh_load(Engine *e, uint64_t id, const char *uri) {
    Tab *t = find(id);
    if (!t || !t->browser)
        return;
    cef_frame_t *frame = t->browser->get_main_frame(t->browser);
    if (!frame)
        return;
    cef_string_t url;
    memset(&url, 0, sizeof url);
    set_text(&url, uri);
    frame->load_url(frame, &url);
    clear_text(&url);
    RELEASE(frame);
}

void spatimesh_back(Engine *e, uint64_t id) {
    Tab *t = find(id);
    if (t && t->browser)
        t->browser->go_back(t->browser);
}

void spatimesh_forward(Engine *e, uint64_t id) {
    Tab *t = find(id);
    if (t && t->browser)
        t->browser->go_forward(t->browser);
}

void spatimesh_reload(Engine *e, uint64_t id) {
    Tab *t = find(id);
    if (t && t->browser)
        t->browser->reload(t->browser);
}

void spatimesh_stop(Engine *e, uint64_t id) {
    Tab *t = find(id);
    if (t && t->browser)
        t->browser->stop_load(t->browser);
}

void spatimesh_resize(Engine *e, uint64_t id, int width, int height, double zoom) {
    Tab *t = find(id);
    if (!t)
        return;
    int rescaled = zoom > 0.1 && zoom != t->zoom;
    t->width = width > 1 ? width : 1;
    t->height = height > 1 ? height : 1;
    if (zoom > 0.1)
        t->zoom = zoom;
    cef_browser_host_t *host = host_of(t);
    if (host) {
        if (rescaled)
            host->notify_screen_info_changed(host);
        host->was_resized(host);
        RELEASE(host);
    }
    damage(t, 0, 0, t->width, t->height);
}

void spatimesh_mute(Engine *e, uint64_t id, int muted) {
    cef_browser_host_t *host = host_of(find(id));
    if (host) {
        host->set_audio_muted(host, muted);
        RELEASE(host);
    }
}

/* The window a tab is in: where its sound is to come from (docs/spatimesh.md, Sound). */
void spatimesh_set_window(Engine *e, uint64_t id, uint32_t window) {
    Tab *t = find(id);
    if (t)
        t->window = window;
}

static uint32_t flags_of(int modifiers) {
    uint32_t f = 0;
    if (modifiers & 1)
        f |= EVENTFLAG_SHIFT_DOWN;
    if (modifiers & 2)
        f |= EVENTFLAG_CONTROL_DOWN;
    if (modifiers & 4)
        f |= EVENTFLAG_ALT_DOWN;
    return f;
}

void spatimesh_pointer(Engine *e, uint64_t id, double x, double y, int button, int pressed, int modifiers) {
    Tab *t = find(id);
    cef_browser_host_t *host = host_of(t);
    if (!host)
        return;
    cef_mouse_event_t ev;
    memset(&ev, 0, sizeof ev);
    ev.x = (int)lround(x / t->zoom);
    ev.y = (int)lround(y / t->zoom);
    t->mouse_x = ev.x;
    t->mouse_y = ev.y;
    if (button == 4 || button == 5) {
        if (pressed) {
            if (button == 4)
                t->browser->go_back(t->browser);
            else
                t->browser->go_forward(t->browser);
        }
    } else if (button == 0) {
        ev.modifiers = flags_of(modifiers) | t->buttons;
        host->send_mouse_move_event(host, &ev, 0);
    } else {
        cef_mouse_button_type_t type = button == 2 ? MBT_MIDDLE : button == 3 ? MBT_RIGHT : MBT_LEFT;
        uint32_t bit = button == 2 ? EVENTFLAG_MIDDLE_MOUSE_BUTTON : button == 3 ? EVENTFLAG_RIGHT_MOUSE_BUTTON : EVENTFLAG_LEFT_MOUSE_BUTTON;
        if (pressed) {
            int64_t now = now_ms();
            int again = button == t->last_button && now - t->last_click < 500 && abs(ev.x - t->last_x) < 5 && abs(ev.y - t->last_y) < 5;
            t->clicks = again ? t->clicks % 3 + 1 : 1;
            t->last_click = now;
            t->last_button = button;
            t->last_x = ev.x;
            t->last_y = ev.y;
            t->buttons |= bit;
        } else {
            t->buttons &= ~bit;
        }
        ev.modifiers = flags_of(modifiers) | t->buttons;
        host->send_mouse_click_event(host, &ev, type, !pressed, t->clicks ? t->clicks : 1);
    }
    RELEASE(host);
}

void spatimesh_leave(Engine *e, uint64_t id) {
    Tab *t = find(id);
    cef_browser_host_t *host = host_of(t);
    if (!host)
        return;
    cef_mouse_event_t ev;
    memset(&ev, 0, sizeof ev);
    ev.x = t->mouse_x;
    ev.y = t->mouse_y;
    host->send_mouse_move_event(host, &ev, 1);
    RELEASE(host);
}

void spatimesh_scroll(Engine *e, uint64_t id, double x, double y, double dx, double dy, int modifiers) {
    Tab *t = find(id);
    cef_browser_host_t *host = host_of(t);
    if (!host)
        return;
    cef_mouse_event_t ev;
    memset(&ev, 0, sizeof ev);
    ev.x = (int)lround(x / t->zoom);
    ev.y = (int)lround(y / t->zoom);
    ev.modifiers = flags_of(modifiers) | t->buttons;
    /* A wheel turns where the pointer is: one told elsewhere moves there first, as a real
     * pointer would have before its wheel turned. */
    if (ev.x != t->mouse_x || ev.y != t->mouse_y) {
        host->send_mouse_move_event(host, &ev, 0);
        t->mouse_x = ev.x;
        t->mouse_y = ev.y;
    }
    /* SpatiMesh's down is Chromium's negative. */
    host->send_mouse_wheel_event(host, &ev, (int)lround(-dx), (int)lround(-dy));
    RELEASE(host);
}

/* Windows' number for a key, which CEF's key events are told in, from Linux's. */
static int vk_of_code(int code) {
    static const unsigned char table[128] = {
        [1] = 0x1B, [2] = '1', [3] = '2', [4] = '3', [5] = '4', [6] = '5', [7] = '6', [8] = '7', [9] = '8', [10] = '9',
        [11] = '0', [12] = 0xBD, [13] = 0xBB, [14] = 0x08, [15] = 0x09, [16] = 'Q', [17] = 'W', [18] = 'E', [19] = 'R',
        [20] = 'T', [21] = 'Y', [22] = 'U', [23] = 'I', [24] = 'O', [25] = 'P', [26] = 0xDB, [27] = 0xDD, [28] = 0x0D,
        [29] = 0x11, [30] = 'A', [31] = 'S', [32] = 'D', [33] = 'F', [34] = 'G', [35] = 'H', [36] = 'J', [37] = 'K',
        [38] = 'L', [39] = 0xBA, [40] = 0xDE, [41] = 0xC0, [42] = 0x10, [43] = 0xDC, [44] = 'Z', [45] = 'X', [46] = 'C',
        [47] = 'V', [48] = 'B', [49] = 'N', [50] = 'M', [51] = 0xBC, [52] = 0xBE, [53] = 0xBF, [54] = 0x10, [56] = 0x12,
        [57] = 0x20, [58] = 0x14, [59] = 0x70, [60] = 0x71, [61] = 0x72, [62] = 0x73, [63] = 0x74, [64] = 0x75, [65] = 0x76,
        [66] = 0x77, [67] = 0x78, [68] = 0x79, [87] = 0x7A, [88] = 0x7B, [97] = 0x11, [100] = 0x12, [102] = 0x24,
        [103] = 0x26, [104] = 0x21, [105] = 0x25, [106] = 0x27, [107] = 0x23, [108] = 0x28, [109] = 0x22, [110] = 0x2D,
        [111] = 0x2E, [125] = 0x5B, [126] = 0x5C,
    };
    return code > 0 && code < 128 ? table[code] : 0;
}

/* `key` is the wire's number (1 Enter ... 15 a character, `ch`). */
static int vk_of(int key, uint32_t ch, int code) {
    static const unsigned char named[15] = {0, 0x0D, 0x1B, 0x08, 0x2E, 0x09, 0x20, 0x25, 0x27, 0x26, 0x28, 0x24, 0x23, 0x21, 0x22};
    if (key >= 1 && key <= 14)
        return named[key];
    int vk = vk_of_code(code);
    if (vk)
        return vk;
    if (key == 15) {
        if (ch >= 'a' && ch <= 'z')
            return (int)(ch - 'a' + 'A');
        if ((ch >= 'A' && ch <= 'Z') || (ch >= '0' && ch <= '9'))
            return (int)ch;
        if (ch == ' ')
            return 0x20;
    }
    return 0;
}

void spatimesh_key(Engine *e, uint64_t id, int key, uint32_t ch, int code, int pressed, int modifiers, const char *text) {
    Tab *t = find(id);
    cef_browser_host_t *host = host_of(t);
    if (!host)
        return;
    cef_key_event_t ev;
    memset(&ev, 0, sizeof ev);
    ev.type = pressed ? KEYEVENT_RAWKEYDOWN : KEYEVENT_KEYUP;
    ev.modifiers = flags_of(modifiers);
    ev.windows_key_code = vk_of(key, ch, code);
    ev.native_key_code = code > 0 ? code + 8 : 0;
    host->send_key_event(host, &ev);
    /* What the key typed, as characters Chromium puts in the page as they are. */
    if (pressed && text && *text) {
        cef_string_t typed;
        memset(&typed, 0, sizeof typed);
        set_text(&typed, text);
        for (size_t i = 0; i < typed.length; i++) {
            ev.type = KEYEVENT_CHAR;
            ev.character = typed.str[i];
            ev.unmodified_character = typed.str[i];
            host->send_key_event(host, &ev);
        }
        clear_text(&typed);
    }
    RELEASE(host);
}

void spatimesh_focus(Engine *e, uint64_t id, int focused) {
    cef_browser_host_t *host = host_of(find(id));
    if (host) {
        host->set_focus(host, focused);
        RELEASE(host);
    }
}

const char *spatimesh_choice(Engine *e, uint64_t id, int index, int *flags) {
    Tab *t = find(id);
    if (!t || index < 0 || index >= t->choices)
        return NULL;
    *flags = t->flags[index];
    return t->labels[index];
}

void spatimesh_choose(Engine *e, uint64_t id, int index) {
    Tab *t = find(id);
    if (!t)
        return;
    char script[96];
    snprintf(script, sizeof script, "window.__spatimeshChoose&&window.__spatimeshChoose(%d)", index);
    run_script(t, script);
    forget_choices(t);
}

void spatimesh_answer(Engine *e, uint64_t id, int accepted, const char *text) {
    Tab *t = find(id);
    if (t)
        answer_dialog(t, accepted, text);
}

static int any_damaged(void) {
    for (Tab *t = tabs; t; t = t->next)
        if (t->damaged)
            return 1;
    return 0;
}

/* Chromium's work, done when it asked for it and at least a frame's time apart (as CEF's own
 * samples pump it), waiting meanwhile for the window to say something. */
void spatimesh_pump(Engine *e, int timeout_ms) {
    int wait = timeout_ms < 33 ? timeout_ms : 33;
    int64_t due = atomic_load(&work_due);
    if (due != INT64_MAX) {
        int64_t until = due - now_ms();
        if (until < wait)
            wait = until > 0 ? (int)until : 0;
    }
    if (first || any_damaged())
        wait = 0;
    struct pollfd fds[2] = {{watch_fd, POLLIN, 0}, {wake_fd, POLLIN, 0}};
    if (wait > 0)
        poll(fds, 2, wait);
    if (wake_fd >= 0) {
        uint64_t n;
        ssize_t r = read(wake_fd, &n, sizeof n);
        (void)r;
    }
    atomic_store(&work_due, INT64_MAX);
    p_cef_do_message_loop_work();
}

/* The next thing that happened, into `out`; its strings live until the next call. */
int spatimesh_next_event(Engine *e, MeshEvent *out) {
    free(current.text);
    free(current.text2);
    memset(&current, 0, sizeof current);
    Queued *q = first;
    if (!q)
        return 0;
    first = q->next;
    if (!first)
        last = NULL;
    current = q->ev;
    free(q);
    *out = current;
    return 1;
}

/* Whether a tab's picture changed and has not been read, and what changed. */
int spatimesh_damage(Engine *e, uint64_t id, int *x, int *y, int *w, int *h) {
    Tab *t = find(id);
    if (!t || !t->damaged)
        return 0;
    *x = t->dx0;
    *y = t->dy0;
    *w = t->dx1 - t->dx0;
    *h = t->dy1 - t->dy0;
    if (*w <= 0 || *h <= 0) {
        t->damaged = 0;
        return 0;
    }
    return 1;
}

/* The tab's picture, where it changed, into `pixels` (`width` by `height`, four bytes a pixel,
 * blue green red alpha), with Chromium's own list over it where one is open; white where
 * Chromium has not drawn yet. The change is then forgotten. */
int spatimesh_copy(Engine *e, uint64_t id, unsigned char *pixels, int width, int height) {
    Tab *t = find(id);
    if (!t || !t->damaged)
        return 0;
    int x0 = t->dx0, x1 = t->dx1 < width ? t->dx1 : width;
    int y0 = t->dy0, y1 = t->dy1 < height ? t->dy1 : height;
    for (int y = y0; y < y1; y++) {
        unsigned char *row = pixels + (size_t)y * width * 4;
        int have = t->pixels && y < t->ph ? (x1 < t->pw ? x1 : t->pw) : x0;
        if (have > x0)
            memcpy(row + (size_t)x0 * 4, t->pixels + ((size_t)y * t->pw + x0) * 4, (size_t)(have - x0) * 4);
        if (have < x1)
            memset(row + (size_t)(have > x0 ? have : x0) * 4, 0xff, (size_t)(x1 - (have > x0 ? have : x0)) * 4);
        if (t->popup_shown && t->popup_pixels && y >= t->popup.y && y < t->popup.y + t->poph) {
            int px0 = x0 > t->popup.x ? x0 : t->popup.x;
            int px1 = x1 < t->popup.x + t->popw ? x1 : t->popup.x + t->popw;
            if (px1 > px0)
                memcpy(row + (size_t)px0 * 4, t->popup_pixels + ((size_t)(y - t->popup.y) * t->popw + (px0 - t->popup.x)) * 4,
                       (size_t)(px1 - px0) * 4);
        }
    }
    t->damaged = 0;
    return 1;
}

/* The end: every tab closed, Chromium given a moment to close them and write down what it
 * keeps, then stopped; the private profile's folder removed. */
void spatimesh_cef_shutdown(void) {
    if (!started)
        return;
    while (tabs)
        spatimesh_close(&the_engine, tabs->id);
    int64_t until = now_ms() + 5000;
    while (live_browsers > 0 && now_ms() < until) {
        p_cef_do_message_loop_work();
        usleep(10000);
    }
    p_cef_shutdown();
    started = 0;
    if (private_folder)
        nftw(private_folder, remove_one, 16, FTW_DEPTH | FTW_PHYS);
}
