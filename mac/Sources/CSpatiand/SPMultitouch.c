//
//  SPMultitouch.c — see include/SPMultitouch.h.
//
//  The framework's frame, as every reader of it has it (it has been this shape since Mac OS X 10.5).
//

#include "include/SPMultitouch.h"
#include <CoreFoundation/CoreFoundation.h>
#include <IOKit/IOKitLib.h>
#include <dispatch/dispatch.h>
#include <dlfcn.h>
#include <mach/mach.h>
#include <mach/mach_time.h>
#include <mach/thread_policy.h>
#include <pthread.h>

typedef struct { float x, y; } MTPoint;
typedef struct { MTPoint position, velocity; } MTVector;
typedef struct {
    int32_t frame;
    double timestamp;
    int32_t identifier, state, fingerID, handID;
    MTVector normalized;
    float size;
    int32_t zero1;
    float angle, majorAxis, minorAxis;
    MTVector absolute;
    int32_t zero2[2];
    float density;
} MTTouch;
typedef void *MTDeviceRef;
typedef int (*MTFrameCallback)(MTDeviceRef, MTTouch *, int32_t, double, int32_t);

static CFArrayRef (*MTDeviceCreateList)(void);
static void (*MTRegisterContactFrameCallback)(MTDeviceRef, MTFrameCallback);
static void (*MTUnregisterContactFrameCallback)(MTDeviceRef, MTFrameCallback);
static int32_t (*MTDeviceStart)(MTDeviceRef, int32_t);
static int32_t (*MTDeviceStop)(MTDeviceRef);
static int32_t (*MTDeviceGetSensorSurfaceDimensions)(MTDeviceRef, int32_t *, int32_t *);

static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static CFArrayRef devices;
static SPFingersCallback heard;
static void *heardContext;

static bool load(void) {
    static int loaded = -1;
    if (loaded >= 0) return loaded;
    void *mt = dlopen("/System/Library/PrivateFrameworks/MultitouchSupport.framework/MultitouchSupport", RTLD_NOW);
    if (mt) {
        MTDeviceCreateList = dlsym(mt, "MTDeviceCreateList");
        MTRegisterContactFrameCallback = dlsym(mt, "MTRegisterContactFrameCallback");
        MTUnregisterContactFrameCallback = dlsym(mt, "MTUnregisterContactFrameCallback");
        MTDeviceStart = dlsym(mt, "MTDeviceStart");
        MTDeviceStop = dlsym(mt, "MTDeviceStop");
        MTDeviceGetSensorSurfaceDimensions = dlsym(mt, "MTDeviceGetSensorSurfaceDimensions");
    }
    loaded = mt && MTDeviceCreateList && MTRegisterContactFrameCallback && MTDeviceStart && MTDeviceStop;
    return loaded;
}

static int frame(MTDeviceRef device, MTTouch *touches, int32_t count, double timestamp, int32_t number) {
    (void)timestamp; (void)number;
    // The framework's thread, real-time too: its frames are handed on at once, ahead of the rest.
    static __thread bool raised;
    if (!raised) { raised = true; sp_thread_realtime(8333333, 500000, 2000000); }
    SPFinger fingers[16];
    int32_t n = count < 16 ? count : 16;
    for (int32_t i = 0; i < n; i++) {
        fingers[i] = (SPFinger){ touches[i].identifier, touches[i].state, touches[i].normalized.position.x,
                                 touches[i].normalized.position.y, touches[i].size, touches[i].majorAxis, touches[i].minorAxis };
    }
    // The pad's size: hundredths of a millimetre, as the framework gives it. A Magic Trackpad's
    // when it does not say.
    int32_t w = 16000, h = 11500;
    if (MTDeviceGetSensorSurfaceDimensions) MTDeviceGetSensorSurfaceDimensions(device, &w, &h);
    SPFingersCallback callback = heard;
    if (callback) callback(fingers, n, (float)w / 100, (float)h / 100, heardContext);
    return 0;
}

/// Hear every trackpad there is now, in place of those heard before. With the lock held.
static CFIndex listen(void) {
    if (devices) {
        for (CFIndex i = 0; i < CFArrayGetCount(devices); i++) {
            MTDeviceRef device = (MTDeviceRef)CFArrayGetValueAtIndex(devices, i);
            if (MTUnregisterContactFrameCallback) MTUnregisterContactFrameCallback(device, frame);
            MTDeviceStop(device);
        }
        CFRelease(devices);
        devices = NULL;
    }
    devices = MTDeviceCreateList();
    CFIndex count = devices ? CFArrayGetCount(devices) : 0;
    for (CFIndex i = 0; i < count; i++) {
        MTDeviceRef device = (MTDeviceRef)CFArrayGetValueAtIndex(devices, i);
        MTRegisterContactFrameCallback(device, frame);
        MTDeviceStart(device, 0);
    }
    return count;
}

// **A trackpad that comes back is another trackpad.** A Bluetooth Magic Trackpad that drops its
// connection and takes it up again (it does, by itself, in the middle of use) is a new device to
// the framework, and the one being heard never says another word: the room's pointer stood still
// under the finger while macOS's own cursor went on moving (the owner, 2026-11-21). So the
// trackpads are looked for anew whenever one arrives or goes, for as long as they are heard.
static IONotificationPortRef watchPort;
static io_iterator_t watchArrived, watchGone;

static void changed(void *context, io_iterator_t iterator) {
    (void)context;
    // The iterator has to be emptied to be told of the next.
    bool any = false;
    for (io_object_t one; (one = IOIteratorNext(iterator)); IOObjectRelease(one)) any = true;
    if (!any) return;
    // A moment later: the framework has not the new trackpad in its list the instant it arrives.
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 300 * NSEC_PER_MSEC), dispatch_get_global_queue(QOS_CLASS_USER_INTERACTIVE, 0), ^{
        pthread_mutex_lock(&lock);
        if (heard) listen();
        pthread_mutex_unlock(&lock);
    });
}

/// With the lock held.
static void watch(void) {
    if (watchPort) return;
    watchPort = IONotificationPortCreate(kIOMainPortDefault);
    if (!watchPort) return;
    IONotificationPortSetDispatchQueue(watchPort, dispatch_get_global_queue(QOS_CLASS_USER_INTERACTIVE, 0));
    // Those there already are heard by `listen`: the iterators are only emptied, to arm them.
    if (IOServiceAddMatchingNotification(watchPort, kIOFirstMatchNotification, IOServiceMatching("AppleMultitouchDevice"), changed, NULL, &watchArrived) == KERN_SUCCESS)
        for (io_object_t one; (one = IOIteratorNext(watchArrived)); IOObjectRelease(one)) {}
    if (IOServiceAddMatchingNotification(watchPort, kIOTerminatedNotification, IOServiceMatching("AppleMultitouchDevice"), changed, NULL, &watchGone) == KERN_SUCCESS)
        for (io_object_t one; (one = IOIteratorNext(watchGone)); IOObjectRelease(one)) {}
}

/// With the lock held.
static void unwatch(void) {
    if (watchArrived) { IOObjectRelease(watchArrived); watchArrived = 0; }
    if (watchGone) { IOObjectRelease(watchGone); watchGone = 0; }
    if (watchPort) { IONotificationPortDestroy(watchPort); watchPort = NULL; }
}

bool sp_multitouch_start(SPFingersCallback callback, void *context) {
    if (!load()) return false;
    pthread_mutex_lock(&lock);
    heard = callback;
    heardContext = context;
    CFIndex count = listen();
    // Watched even with none: the trackpad may be yet to connect.
    watch();
    pthread_mutex_unlock(&lock);
    return count > 0;
}

bool sp_multitouch_again(void) {
    pthread_mutex_lock(&lock);
    CFIndex count = heard ? listen() : 0;
    pthread_mutex_unlock(&lock);
    return count > 0;
}

void sp_multitouch_stop(void) {
    pthread_mutex_lock(&lock);
    unwatch();
    if (devices) {
        for (CFIndex i = 0; i < CFArrayGetCount(devices); i++) {
            MTDeviceRef device = (MTDeviceRef)CFArrayGetValueAtIndex(devices, i);
            if (MTUnregisterContactFrameCallback) MTUnregisterContactFrameCallback(device, frame);
            MTDeviceStop(device);
        }
        CFRelease(devices);
        devices = NULL;
    }
    heard = NULL;
    pthread_mutex_unlock(&lock);
}

bool sp_thread_realtime(uint64_t period_ns, uint64_t computation_ns, uint64_t constraint_ns) {
    mach_timebase_info_data_t base;
    mach_timebase_info(&base);
    // Nanoseconds in the clock's own ticks.
    #define TICKS(ns) ((uint32_t)((ns) * base.denom / base.numer))
    thread_time_constraint_policy_data_t policy = {
        .period = TICKS(period_ns), .computation = TICKS(computation_ns), .constraint = TICKS(constraint_ns), .preemptible = true,
    };
    #undef TICKS
    return thread_policy_set(pthread_mach_thread_np(pthread_self()), THREAD_TIME_CONSTRAINT_POLICY,
                             (thread_policy_t)&policy, THREAD_TIME_CONSTRAINT_POLICY_COUNT) == KERN_SUCCESS;
}
