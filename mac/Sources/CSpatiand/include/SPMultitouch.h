//
//  SPMultitouch.h — a trackpad's fingers as the trackpad reports them, and a thread made real-time.
//
//  macOS keeps every trackpad to its own driver, a Bluetooth Magic Trackpad as much as a built-in
//  one: a program may open the device "seized" and is told it has it, and then gets nothing while
//  the cursor goes on moving (tried on an M6 Mac mini, macOS 27.2, with Input Monitoring allowed).
//  What it does give is the fingers themselves, frame by frame, through MultitouchSupport -- the
//  private framework every tool that reads a trackpad's fingers uses. Loaded when asked for, so
//  that a macOS without it leaves Spatiand on the gestures macOS makes (RoomTap).
//

#import <stdbool.h>
#import <stdint.h>

/// One finger, or the start or end of one. `x` and `y` from 0 to 1 across the pad, `y` upwards,
/// as AppKit's touches have them.
typedef struct {
    int32_t id;
    /// 1 coming near, 2 hovering, 3 landing, 4 on the pad, 5 lifting, 6 lingering, 7 gone.
    int32_t state;
    float x, y;
    /// How much of the finger is on the pad, and how long and wide its mark is (mm).
    float size, major, minor;
} SPFinger;

/// Called on the framework's own thread, once for every frame of every trackpad, with the
/// fingers of that frame (none, once they have all gone) and the pad's size in millimetres.
typedef void (*SPFingersCallback)(const SPFinger *fingers, int32_t count, float width_mm, float height_mm, void *context);

/// Start hearing every trackpad there is. False when the framework is not there, or has no
/// trackpad. Safe to call again: the trackpads are looked for anew.
bool sp_multitouch_start(SPFingersCallback callback, void *context);
/// Stop hearing them.
void sp_multitouch_stop(void);

/// The calling thread, real-time: given `computation_ns` of every `period_ns`, done within
/// `constraint_ns` of being woken, ahead of every thread of ordinary priority -- the window
/// server's way of keeping a pointer smooth. False if the kernel refused it.
bool sp_thread_realtime(uint64_t period_ns, uint64_t computation_ns, uint64_t constraint_ns);
