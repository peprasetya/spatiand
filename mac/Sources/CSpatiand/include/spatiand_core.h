// The link to a Spatiand host, for the Mac app. See crates/spatiand-mac-core.
//
// Everything crosses as JSON text except pictures and sound, which cross as bytes. Callbacks
// arrive on threads of the core's own: copy what you need and return, and never call back into
// the core from inside one with anything that waits.
#ifndef SPATIAND_CORE_H
#define SPATIAND_CORE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct sp_core sp_core;

// One JSON object per call. Either {"host": <what the host said>} (a HostMessage as serde writes
// it, with each application's icon as a base64 string) or {"core": {...}} for what the core
// itself has to say: "connected", "disconnected", "compare", "paired", "pair_failed".
typedef void (*sp_event_fn)(void *user, const char *json);

// One whole picture for one window. `codec`: 0 H.264, 1 HEVC, 2 AV1. The bytes are an
// Annex B access unit, parameter sets included on a keyframe.
typedef void (*sp_video_fn)(void *user, uint16_t window, int32_t codec, int32_t keyframe,
                            uint64_t captured_us, const uint8_t *data, size_t length);

// Sound from one application: signed 16-bit little-endian, interleaved, 48 kHz.
typedef void (*sp_audio_fn)(void *user, const char *app, uint16_t channels, const uint8_t *pcm,
                            size_t length);

// Start the core. `identity_dir` holds this computer's certificate and key, made if absent.
sp_core *sp_start(const char *identity_dir, void *user, sp_event_fn on_event,
                  sp_video_fn on_video, sp_audio_fn on_audio);

// This computer's fingerprint, in full. Free with sp_free_string.
char *sp_fingerprint(sp_core *core);

// Pair with a host that is waiting to be paired with. Reports through events: "compare" with the
// code to read out, then "paired" or "pair_failed".
void sp_pair(sp_core *core, const char *address);

// Open a session with a paired host. `fingerprint` is the host's, in full hex.
void sp_connect(sp_core *core, const char *address, const char *fingerprint);

// Say something to the host: a ClientMessage as JSON. Ignored with no session.
void sp_say(sp_core *core, const char *json);

// Leave the session, keeping the host's applications running.
void sp_disconnect(sp_core *core);

void sp_free_string(char *string);
void sp_stop(sp_core *core);

// ---- the room: the glasses' world. See crates/spatiand-mac-core/src/room.rs. ----

typedef struct RoomHandle sp_room;

typedef struct {
    int32_t window;      // the window's id, or -1 for none
    double x, y;         // where in that window, in the host's pixels
    double point[3];     // where in the room: +X forward, +Y left, +Z up
    int32_t title;       // 1 when it is the window's title bar (1024 by 46 pixels)
    int32_t corner;      // 1 when it is a window's bottom right corner, where a press resizes it
} sp_aim;

typedef struct {
    uint32_t window;     // 0xFFFF is the cursor; 0x10000 added is a window's title bar
    uint32_t first;      // first vertex
    uint32_t count;      // vertices
    uint32_t flags;      // bit 0 focused, bit 1 aimed at, bit 2 pinned to the glass
} sp_draw;

sp_room *sp_room_new(void);
void sp_room_free(sp_room *room);

// One sample from the glasses: degrees a second, g, gauss.
void sp_room_imu(sp_room *room, uint64_t timestamp_ns, const double *gyro, const double *accel,
                 const double *mag);
void sp_room_recentre(sp_room *room);
int32_t sp_room_has_head(sp_room *room);
// For a preview with no sensors: hold the head at this heading and pitch, in degrees.
void sp_room_set_head(sp_room *room, int32_t enable, double yaw_deg, double pitch_deg);
void sp_room_set_per_eye(sp_room *room, uint32_t width, uint32_t height);
// The head's heading, pitch and roll, degrees.
void sp_room_head_euler(sp_room *room, double *out);

void sp_room_set_window(sp_room *room, uint32_t id, uint32_t width, uint32_t height);
// A panel of Spatiand's own (ids 0xFFF0 and up): where the wearer looks, no keyboard focus.
void sp_room_set_panel(sp_room *room, uint32_t id, uint32_t width_px, uint32_t height_px,
                       double width_m, double radius_m);
// Which application a window belongs to, so its sound is put where the window is.
void sp_room_set_app(sp_room *room, uint32_t id, const char *app);
// An application's sound (signed 16-bit little-endian, interleaved) placed in the room and folded
// to two ears: interleaved stereo floats into `out`. Returns how many; 0 if it would not fit.
size_t sp_room_audio(sp_room *room, const char *app, uint16_t channels, const uint8_t *pcm,
                     size_t length, float *out, size_t capacity);
// What the pointer looks like: its picture size and hot spot in pixels; all zero is the arrow.
void sp_room_set_cursor_shape(sp_room *room, double hot_x, double hot_y, double width, double height);
void sp_room_show(sp_room *room, uint32_t id);
void sp_room_remove(sp_room *room, uint32_t id);
void sp_room_clear(sp_room *room);
void sp_room_focus(sp_room *room, int32_t id);
int32_t sp_room_focused(sp_room *room);

void sp_room_move_pointer(sp_room *room, double dx, double dy);
void sp_room_centre_pointer(sp_room *room);
void sp_room_aim(sp_room *room, sp_aim *out);
int32_t sp_room_aim_at(sp_room *room, uint32_t id, double *x, double *y);
// The same, off the window's edge as well as on it.
int32_t sp_room_aim_free(sp_room *room, uint32_t id, double *x, double *y);
// The picture size at the density windows start with, at the width the window has now.
int32_t sp_room_native_size(sp_room *room, uint32_t id, uint32_t *w, uint32_t *h);
// The host is being asked for this size; writes it as limited, and the width follows the answer.
int32_t sp_room_request_size(sp_room *room, uint32_t id, double width, double height, uint32_t *w, uint32_t *h);

void sp_room_begin_grab(sp_room *room, uint32_t id);
void sp_room_drag(sp_room *room);
void sp_room_end_grab(sp_room *room);
int32_t sp_room_grabbed(sp_room *room);
// Move a window round the room: degrees to the left and up.
void sp_room_nudge(sp_room *room, uint32_t id, double yaw_deg, double pitch_deg);
// Move the keyboard to the next window round the room (+1 left, -1 right); its id, or -1.
int32_t sp_room_focus_step(sp_room *room, int32_t step);
// Gather the windows side by side in front of the wearer; `spread` is the room between them.
void sp_room_arrange(sp_room *room, double spread);
void sp_room_scale(sp_room *room, uint32_t id, double factor);
void sp_room_bring_here(sp_room *room, uint32_t id);
void sp_room_set_pinned(sp_room *room, uint32_t id, int32_t pinned);
int32_t sp_room_is_pinned(sp_room *room, uint32_t id);
void sp_room_next_corner(sp_room *room);
void sp_room_toggle_size(sp_room *room);

// What to draw. `matrices`: 32 floats, the left eye's view-projection then the right's,
// column-major. Vertices are x y z u v. Returns the floats written (0 if they would not fit).
uint32_t sp_room_frame(sp_room *room, float *matrices, float *vertices, uint32_t vertex_capacity,
                       sp_draw *draws, uint32_t draw_capacity, uint32_t *draw_count);

// ---- the Mac as a host. See crates/spatiand-mac-core/src/host.rs. ----

typedef struct HostCore sp_host;

// What the app hears, as one JSON object per call: {"joined": {who, short, address, code, known}},
// {"said": <a ClientMessage as serde writes it, bytes as base64>}, {"left": {refused}}.
// Start listening on `port` with the identity in `identity_dir`. NULL if it could not.
sp_host *sp_host_start(const char *identity_dir, uint16_t port, void *user, sp_event_fn on_event);
void sp_host_stop(sp_host *host);
// This host's fingerprint, in full. Free with sp_free_string.
char *sp_host_fingerprint(sp_host *host);
// Say something to the session: a HostMessage as JSON (bytes as base64).
void sp_host_say(sp_host *host, const char *json);
// One picture of one window: an Annex B access unit, parameter sets first on a keyframe.
void sp_host_video(sp_host *host, uint16_t window, int32_t keyframe, uint64_t captured_us,
                   const uint8_t *data, size_t length);
// A little of one application's sound: signed 16-bit little-endian, interleaved, 48 kHz.
void sp_host_audio(sp_host *host, const char *app, uint16_t channels, const uint8_t *data, size_t length);
// Let a device that has never been here pair, or stop.
void sp_host_open_pairing(sp_host *host, int32_t open);
// The owner's answer about the device waiting to be let in.
void sp_host_decide(sp_host *host, int32_t admit);
void sp_host_trust(sp_host *host, const char *fingerprint);
int32_t sp_host_paired_count(sp_host *host);
void sp_host_forget_all(sp_host *host);

#ifdef __cplusplus
}
#endif

#endif
