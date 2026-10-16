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
    X(cef_string_userfree_utf16_free)

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

/* Structures CEF hands over come with a reference for whoever receives them. */
#define RELEASE(p)                                                                                \
    do {                                                                                          \
        if (p)                                                                                    \
            (p)->base.release(&(p)->base);                                                        \
    } while (0)

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
    if (ours)
        select_list(t, text + strlen(SELECT_MARK));
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

static void make_handlers(void) {
    static int made;
    if (made)
        return;
    made = 1;
    kept(&app.base, sizeof app);
    app.on_before_command_line_processing = on_command_line;
    app.get_browser_process_handler = get_process_handler;

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
    t->browser = p_cef_browser_host_create_browser_sync(&info, &client, &url, &browser_settings, NULL, NULL);
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
