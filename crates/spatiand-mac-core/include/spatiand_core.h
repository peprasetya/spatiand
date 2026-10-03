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

#ifdef __cplusplus
}
#endif

#endif
