// The link to a Spatiand host, for the Mac app's windows on this Mac when there are no glasses,
// and this Mac as a host. See crates/spatiand-mac-core. The room in the glasses is the Deck's
// compositor: spatiand_mac.h.
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
