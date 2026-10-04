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
} sp_aim;

typedef struct {
    uint32_t window;     // 0xFFFF is the cursor
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
void sp_room_show(sp_room *room, uint32_t id);
void sp_room_remove(sp_room *room, uint32_t id);
void sp_room_clear(sp_room *room);
void sp_room_focus(sp_room *room, int32_t id);
int32_t sp_room_focused(sp_room *room);

void sp_room_move_pointer(sp_room *room, double dx, double dy);
void sp_room_centre_pointer(sp_room *room);
void sp_room_aim(sp_room *room, sp_aim *out);
int32_t sp_room_aim_at(sp_room *room, uint32_t id, double *x, double *y);

void sp_room_begin_grab(sp_room *room, uint32_t id);
void sp_room_drag(sp_room *room);
void sp_room_end_grab(sp_room *room);
int32_t sp_room_grabbed(sp_room *room);
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

#ifdef __cplusplus
}
#endif

#endif
