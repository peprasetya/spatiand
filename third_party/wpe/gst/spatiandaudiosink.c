/*
 * spatiandaudiosink: pages' sound handed to the Spatiand session, which places it at SpatiMesh's
 * window in the room through its HRTF (and plays it plain in 2D mode), as Spatiand's own
 * applications' sound is: the session's socket for it (crates/spatiand/src/own.rs, `sound`),
 * under the window process's application id (SPATIAND_APP_ID). Its mute and volume are the
 * window's, on its bar.
 *
 * Ranked above the platform's own sink, so that autoaudiosink (WebKit's) takes it; where there is
 * no session to hand sound to, it does not open, and the next sink plays it as before.
 *
 * The session keeps no more than a tenth of a second of a feed waiting, so this paces itself:
 * it writes as fast as the sound plays and no faster, and says how far ahead it is (its delay).
 */

#ifdef HAVE_CONFIG_H
#include "config.h"
#endif

#include <gst/audio/gstaudiosink.h>
#include <gst/gst.h>

#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

#define SPATIAND_TYPE_AUDIO_SINK (spatiand_audio_sink_get_type())
G_DECLARE_FINAL_TYPE(SpatiandAudioSink, spatiand_audio_sink, SPATIAND, AUDIO_SINK, GstAudioSink)

struct _SpatiandAudioSink {
    GstAudioSink parent;
    int fd;
    int channels, rate, bytes_per_frame;
    /* When the first frame written plays, and how many have been written since. */
    gint64 started;
    guint64 frames;
};

G_DEFINE_TYPE(SpatiandAudioSink, spatiand_audio_sink, GST_TYPE_AUDIO_SINK)

/* 16-bit, interleaved, at the session's rate; as many channels as the session places (a 7.1
 * film's eight). */
static GstStaticPadTemplate sink_template = GST_STATIC_PAD_TEMPLATE("sink", GST_PAD_SINK, GST_PAD_ALWAYS,
    GST_STATIC_CAPS("audio/x-raw, format = (string) S16LE, layout = (string) interleaved, rate = (int) 48000, channels = (int) [ 1, 8 ]"));

static gint64 now_us(void) {
    return g_get_monotonic_time();
}

/* The session's socket for sound, if there is a session to hand sound to. */
static gboolean socket_path(struct sockaddr_un *where) {
    const char *dir = g_getenv("XDG_RUNTIME_DIR");
    /* Only inside a Spatiand session (the application id says so): elsewhere the next sink. */
    if (!dir || !g_getenv("SPATIAND_APP_ID") || g_getenv("SPATIMESH_SOUND_OFF"))
        return FALSE;
    *where = (struct sockaddr_un) { .sun_family = AF_UNIX };
    g_snprintf(where->sun_path, sizeof where->sun_path, "%s/spatiand-sound", dir);
    return g_file_test(where->sun_path, G_FILE_TEST_EXISTS);
}

/* Opened only to see whether there is a session (autoaudiosink opens each sink it tries, and
 * closes the ones it does not take): one is there if its socket answers -- the file outlives a
 * session that has ended (the glasses unplugged), and sound then goes to the next sink. The
 * connection sound goes over is made when it is to be played. */
static gboolean spatiand_open(GstAudioSink *base) {
    struct sockaddr_un where;
    if (!socket_path(&where))
        return FALSE;
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0)
        return FALSE;
    gboolean answers = connect(fd, (struct sockaddr *)&where, sizeof where) == 0;
    close(fd);
    return answers;
}

static gboolean spatiand_connect(SpatiandAudioSink *self) {
    struct sockaddr_un where;
    if (!socket_path(&where))
        return FALSE;
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0)
        return FALSE;
    if (connect(fd, (struct sockaddr *)&where, sizeof where) != 0) {
        close(fd);
        return FALSE;
    }
#ifdef SO_NOSIGPIPE
    int on = 1;
    setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, sizeof on);
#endif
    self->fd = fd;
    return TRUE;
}

static gboolean spatiand_prepare(GstAudioSink *base, GstAudioRingBufferSpec *spec) {
    SpatiandAudioSink *self = SPATIAND_AUDIO_SINK(base);
    /* One connection for each feed: a change of format is a new one. */
    if (!spatiand_connect(self)) {
        GST_ELEMENT_ERROR(self, RESOURCE, OPEN_WRITE, ("No Spatiand session to take sound"), ("%s", g_strerror(errno)));
        return FALSE;
    }
    self->channels = GST_AUDIO_INFO_CHANNELS(&spec->info);
    self->rate = GST_AUDIO_INFO_RATE(&spec->info);
    self->bytes_per_frame = GST_AUDIO_INFO_BPF(&spec->info);
    /* SPSN, channels (u16), rate (u32), the application id (u16 length, then its bytes). */
    /* Each page's sound under a name of its own (its web process says which page it is
     * showing: WebKit's WebPage sets SPATIAND_PAGE), which SpatiMesh's window routes to the
     * place of the window the tab is in; or the application's own, where there is none. */
    const char *page = g_getenv("SPATIAND_PAGE");
    char *app = page && *page ? g_strdup_printf("%s/page%s", g_getenv("SPATIAND_APP_ID"), page) : g_strdup(g_getenv("SPATIAND_APP_ID"));
    guint16 channels = (guint16)self->channels, length = (guint16)strlen(app);
    guint32 rate = (guint32)self->rate;
    unsigned char head[12];
    memcpy(head, "SPSN", 4);
    memcpy(head + 4, &channels, 2);
    memcpy(head + 6, &rate, 4);
    memcpy(head + 10, &length, 2);
    gboolean told = write(self->fd, head, sizeof head) == sizeof head && write(self->fd, app, length) == length;
    g_free(app);
    if (!told) {
        GST_ELEMENT_ERROR(self, RESOURCE, WRITE, ("The Spatiand session took no sound"), ("%s", g_strerror(errno)));
        return FALSE;
    }
    self->started = 0;
    self->frames = 0;
    return TRUE;
}

static gboolean spatiand_unprepare(GstAudioSink *base) {
    SpatiandAudioSink *self = SPATIAND_AUDIO_SINK(base);
    if (self->fd >= 0)
        close(self->fd);
    self->fd = -1;
    return TRUE;
}

static gboolean spatiand_close(GstAudioSink *base) {
    return TRUE;
}

/* How far ahead of the sound being heard what was written is, in microseconds. */
static gint64 ahead_us(SpatiandAudioSink *self) {
    if (!self->started || !self->rate)
        return 0;
    gint64 written_until = self->started + (gint64)(self->frames * G_USEC_PER_SEC / (guint64)self->rate);
    return MAX(written_until - now_us(), 0);
}

static gint spatiand_write(GstAudioSink *base, gpointer data, guint length) {
    SpatiandAudioSink *self = SPATIAND_AUDIO_SINK(base);
    if (!self->started)
        self->started = now_us();
    /* No further ahead than the session keeps: wait until most of it has played. */
    gint64 ahead = ahead_us(self);
    if (ahead > 60000)
        g_usleep((gulong)(ahead - 40000));
    const char *bytes = data;
    guint left = length;
    while (left) {
        ssize_t n = write(self->fd, bytes, left);
        if (n < 0) {
            if (errno == EINTR)
                continue;
            /* The session went: what is left is not heard. */
            break;
        }
        bytes += n;
        left -= (guint)n;
    }
    if (self->bytes_per_frame)
        self->frames += length / (guint)self->bytes_per_frame;
    return (gint)length;
}

static guint spatiand_delay(GstAudioSink *base) {
    SpatiandAudioSink *self = SPATIAND_AUDIO_SINK(base);
    return (guint)(ahead_us(self) * self->rate / G_USEC_PER_SEC);
}

/* A seek or a pause: what was written is dropped by the session soon enough; time starts again. */
static void spatiand_reset(GstAudioSink *base) {
    SpatiandAudioSink *self = SPATIAND_AUDIO_SINK(base);
    self->started = 0;
    self->frames = 0;
}

static void spatiand_audio_sink_class_init(SpatiandAudioSinkClass *klass) {
    GstElementClass *element = GST_ELEMENT_CLASS(klass);
    GstAudioSinkClass *sink = GST_AUDIO_SINK_CLASS(klass);
    gst_element_class_set_static_metadata(element, "Spatiand audio sink", "Sink/Audio",
        "Sound placed in the Spatiand room, at the window that plays it", "Spatiand");
    gst_element_class_add_static_pad_template(element, &sink_template);
    sink->open = spatiand_open;
    sink->prepare = spatiand_prepare;
    sink->unprepare = spatiand_unprepare;
    sink->close = spatiand_close;
    sink->write = spatiand_write;
    sink->delay = spatiand_delay;
    sink->reset = spatiand_reset;
}

static void spatiand_audio_sink_init(SpatiandAudioSink *self) {
    self->fd = -1;
}

static gboolean plugin_init(GstPlugin *plugin) {
    /* Above the platform's own sinks (PRIMARY), so that autoaudiosink tries this first. */
    return gst_element_register(plugin, "spatiandaudiosink", GST_RANK_PRIMARY + 10, SPATIAND_TYPE_AUDIO_SINK);
}

#define PACKAGE "spatiand"
GST_PLUGIN_DEFINE(GST_VERSION_MAJOR, GST_VERSION_MINOR, spatiand, "Sound into the Spatiand room", plugin_init, "1.0", "LGPL", "Spatiand", "https://prasetya.id")
