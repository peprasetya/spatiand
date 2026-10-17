/* The pages' sound, handed to the room to be placed where SpatiMesh's window is.
 *
 * WebKit plays a page's sound from its GPU process, which is launchd's child and not this
 * program's: nothing of it reaches the room by itself, and the Mac's taps of its own applications
 * cannot tell it from Spatiand's own (it is "responsible" to whatever started Spatiand's
 * session). So the engine host, which knows its own WebKit processes (the web view says, by
 * SPI), taps their sound itself -- a Core Audio process tap, muting what it takes while it takes
 * it -- and writes it to the session's socket for Spatiand's own applications' sound
 * (crates/spatiand/src/own.rs, `sound`), under the id SpatiMesh's window has
 * (SPATIAND_APP_ID, from the window process). The session places it through the HRTF at that
 * window in the room, and plays it unplaced -- plain stereo -- in 2D mode.
 *
 * Where there is no session to take it (SpatiMesh run on its own) nothing is tapped and WebKit
 * plays as it always does. Needs macOS 14.2 (process taps), and the session's permission to
 * record what the Mac plays (the Spatiand app's, which its own taps already ask for).
 */
#import <CoreAudio/CoreAudio.h>
#import <CoreAudio/CATapDescription.h>
#import <CoreAudio/AudioHardwareTapping.h>
#import <Foundation/Foundation.h>
#include <fcntl.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

static dispatch_queue_t queue;
static AudioObjectID tap = kAudioObjectUnknown;
static AudioObjectID aggregate = kAudioObjectUnknown;
static AudioDeviceIOProcID proc;
static NSSet<NSNumber *> *tapped;
static int sink = -1;
static double last_playing;
/* Resampling to 48 kHz, carried from one block to the next. */
static double phase;
static float carry[2];

/* Process taps are macOS 14.2's, asked for by whether they are there (weakly linked): `@available`
 * needs a part of clang's runtime that a Rust program is not linked with. */
static BOOL taps_exist(void) {
    return NSClassFromString(@"CATapDescription") != nil && &AudioHardwareCreateProcessTap != NULL;
}

static double now_s(void) {
    return [NSDate timeIntervalSinceReferenceDate];
}

static AudioObjectID object_of(pid_t pid) {
    AudioObjectPropertyAddress address = {kAudioHardwarePropertyTranslatePIDToProcessObject, kAudioObjectPropertyScopeGlobal,
                                          kAudioObjectPropertyElementMain};
    AudioObjectID object = kAudioObjectUnknown;
    UInt32 size = sizeof object;
    if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, sizeof pid, &pid, &size, &object) != noErr)
        return kAudioObjectUnknown;
    return object;
}

/* The session's socket for its own applications' sound, said hello to as SpatiReel does. */
static int open_sink(void) {
    const char *dir = getenv("XDG_RUNTIME_DIR");
    if (!dir || getenv("SPATIMESH_SOUND_OFF"))
        return -1;
    struct sockaddr_un where = {.sun_family = AF_UNIX};
    snprintf(where.sun_path, sizeof where.sun_path, "%s/spatiand-sound", dir);
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0)
        return -1;
    if (connect(fd, (struct sockaddr *)&where, sizeof where) != 0) {
        close(fd);
        return -1;
    }
    int on = 1;
    setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, sizeof on);
    const char *app = getenv("SPATIAND_APP_ID");
    if (!app)
        app = "id.prasetya.spatiand.spatimesh";
    uint16_t channels = 2, length = (uint16_t)strlen(app);
    uint32_t rate = 48000;
    unsigned char head[12];
    memcpy(head, "SPSN", 4);
    memcpy(head + 4, &channels, 2);
    memcpy(head + 6, &rate, 4);
    memcpy(head + 10, &length, 2);
    if (write(fd, head, sizeof head) != sizeof head || write(fd, app, length) != length) {
        close(fd);
        return -1;
    }
    /* From here on a write that would wait is dropped: the room is behind, not this. */
    fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK);
    return fd;
}

static void stop_tap(void) {
    if (proc && aggregate != kAudioObjectUnknown) {
        AudioDeviceStop(aggregate, proc);
        AudioDeviceDestroyIOProcID(aggregate, proc);
    }
    proc = NULL;
    if (aggregate != kAudioObjectUnknown)
        AudioHardwareDestroyAggregateDevice(aggregate);
    aggregate = kAudioObjectUnknown;
    if (tap != kAudioObjectUnknown && taps_exist())
        AudioHardwareDestroyProcessTap(tap);
    tap = kAudioObjectUnknown;
    tapped = nil;
    if (sink >= 0)
        close(sink);
    sink = -1;
}

static NSString *default_output_uid(void) {
    AudioObjectPropertyAddress address = {kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeGlobal,
                                          kAudioObjectPropertyElementMain};
    AudioObjectID device = 0;
    UInt32 size = sizeof device;
    AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, NULL, &size, &device);
    address.mSelector = kAudioDevicePropertyDeviceUID;
    CFStringRef uid = NULL;
    size = sizeof uid;
    AudioObjectGetPropertyData(device, &address, 0, NULL, &size, &uid);
    return uid ? (__bridge_transfer NSString *)uid : @"";
}

/* How many streams of sound the output device takes in itself (a microphone's): the tap's come
 * after them in what the device hands over. */
static UInt32 input_streams(NSString *uid) {
    AudioObjectPropertyAddress address = {kAudioHardwarePropertyTranslateUIDToDevice, kAudioObjectPropertyScopeGlobal,
                                          kAudioObjectPropertyElementMain};
    AudioObjectID device = kAudioObjectUnknown;
    UInt32 size = sizeof device;
    CFStringRef name = (__bridge CFStringRef)uid;
    if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, sizeof name, &name, &size, &device) != noErr)
        return 0;
    AudioObjectPropertyAddress streams = {kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput, kAudioObjectPropertyElementMain};
    UInt32 bytes = 0;
    if (AudioObjectGetPropertyDataSize(device, &streams, 0, NULL, &bytes) != noErr)
        return 0;
    return bytes / sizeof(AudioObjectID);
}

/* Stereo float at `rate` to 16-bit at 48 kHz, by straight lines between samples, and written. */
static void hand_over(const float *left, const float *right, size_t stride, size_t frames, double rate) {
    if (sink < 0 || frames == 0)
        return;
    double step = rate / 48000.0;
    int16_t out[8192];
    size_t n = 0;
    double position = phase;
    while (position <= (double)(frames - 1) && n + 2 <= sizeof out / sizeof out[0]) {
        long a = (long)floor(position);
        float t = (float)(position - a);
        float l0 = a < 0 ? carry[0] : left[a * stride], r0 = a < 0 ? carry[1] : right[a * stride];
        size_t b = (size_t)(a + 1 < (long)frames ? a + 1 : (long)frames - 1);
        float l = l0 * (1 - t) + left[b * stride] * t, r = r0 * (1 - t) + right[b * stride] * t;
        out[n++] = (int16_t)(fmaxf(-1, fminf(1, l)) * 32767);
        out[n++] = (int16_t)(fmaxf(-1, fminf(1, r)) * 32767);
        position += step;
    }
    phase = position - (double)frames;
    carry[0] = left[(frames - 1) * stride];
    carry[1] = right[(frames - 1) * stride];
    ssize_t wrote = write(sink, out, n * sizeof out[0]);
    if (wrote < 0 && errno != EAGAIN && errno != EWOULDBLOCK) {
        /* The session has gone: let WebKit be heard again. */
        dispatch_async(queue, ^{
            stop_tap();
        });
    }
}

static BOOL start_tap(NSArray<NSNumber *> *objects) {
    stop_tap();
    sink = open_sink();
    if (sink < 0)
        return NO;
    CATapDescription *description = [[CATapDescription alloc] initStereoMixdownOfProcesses:objects];
    description.UUID = [NSUUID UUID];
    description.muteBehavior = CATapMutedWhenTapped;
    description.privateTap = YES;
    AudioObjectID made = kAudioObjectUnknown;
    OSStatus status = AudioHardwareCreateProcessTap(description, &made);
    if (status != noErr) {
        fprintf(stderr, "spatimesh-webkit: the pages' sound could not be taken (%d); it plays on the Mac\n", (int)status);
        stop_tap();
        return NO;
    }
    tap = made;
    NSString *output = default_output_uid();
    NSDictionary *spec = @{
        @kAudioAggregateDeviceNameKey : @"SpatiMesh pages",
        @kAudioAggregateDeviceUIDKey : [NSUUID UUID].UUIDString,
        @kAudioAggregateDeviceMainSubDeviceKey : output,
        @kAudioAggregateDeviceIsPrivateKey : @YES,
        @kAudioAggregateDeviceIsStackedKey : @NO,
        @kAudioAggregateDeviceTapAutoStartKey : @YES,
        @kAudioAggregateDeviceSubDeviceListKey : @[ @{@kAudioSubDeviceUIDKey : output} ],
        @kAudioAggregateDeviceTapListKey : @[ @{@kAudioSubTapDriftCompensationKey : @YES, @kAudioSubTapUIDKey : description.UUID.UUIDString} ],
    };
    AudioObjectID device = kAudioObjectUnknown;
    if (AudioHardwareCreateAggregateDevice((__bridge CFDictionaryRef)spec, &device) != noErr) {
        stop_tap();
        return NO;
    }
    aggregate = device;
    /* The rate it arrives at is the device's it is read through, on the output's clock. */
    Float64 rate = 0;
    AudioObjectPropertyAddress nominal = {kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    UInt32 size = sizeof rate;
    AudioObjectGetPropertyData(aggregate, &nominal, 0, NULL, &size, &rate);
    if (rate <= 0)
        rate = 48000;
    UInt32 skip = input_streams(output);
    phase = 0;
    carry[0] = carry[1] = 0;
    status = AudioDeviceCreateIOProcIDWithBlock(&proc, aggregate, queue,
        ^(const AudioTimeStamp *now, const AudioBufferList *input, const AudioTimeStamp *input_time, AudioBufferList *output_list,
          const AudioTimeStamp *output_time) {
            if (!input || input->mNumberBuffers == 0)
                return;
            UInt32 at = skip < input->mNumberBuffers ? skip : input->mNumberBuffers - 1;
            const AudioBuffer *b = &input->mBuffers[at];
            if (!b->mData)
                return;
            UInt32 held = b->mNumberChannels ? b->mNumberChannels : 1;
            size_t frames = b->mDataByteSize / (sizeof(float) * held);
            const float *p = b->mData;
            if (held >= 2) {
                hand_over(p, p + 1, held, frames, rate);
            } else {
                const AudioBuffer *next = at + 1 < input->mNumberBuffers ? &input->mBuffers[at + 1] : NULL;
                const float *r = next && next->mData && next->mNumberChannels == 1 && next->mDataByteSize == b->mDataByteSize ? next->mData : p;
                hand_over(p, r, 1, frames, rate);
            }
        });
    if (status != noErr || !proc || AudioDeviceStart(aggregate, proc) != noErr) {
        stop_tap();
        return NO;
    }
    tapped = [NSSet setWithArray:objects];
    fprintf(stderr, "spatimesh-webkit: the pages' sound goes to the room (%lu process(es), %.0f Hz)\n", (unsigned long)objects.count, rate);
    return YES;
}

/* A page is playing; `pids` are this engine's WebKit processes that may be making its sound. Taps
 * them if they are not tapped already (more of them may be playing than when the tap was made),
 * and lets go of them once nothing has played for half a minute. */
void spatimesh_sound_playing(const pid_t *pids, size_t count) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        queue = dispatch_queue_create("spatimesh.sound", DISPATCH_QUEUE_SERIAL);
        dispatch_source_t timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, queue);
        dispatch_source_set_timer(timer, dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC), 10 * NSEC_PER_SEC, NSEC_PER_SEC);
        dispatch_source_set_event_handler(timer, ^{
            if (tap != kAudioObjectUnknown && now_s() - last_playing > 30)
                stop_tap();
        });
        dispatch_resume(timer);
        CFRetain((__bridge CFTypeRef)timer);
    });
    NSMutableArray<NSNumber *> *objects = [NSMutableArray array];
    for (size_t i = 0; i < count; i++) {
        AudioObjectID object = pids[i] > 0 ? object_of(pids[i]) : kAudioObjectUnknown;
        if (object != kAudioObjectUnknown && ![objects containsObject:@(object)])
            [objects addObject:@(object)];
    }
    dispatch_async(queue, ^{
        last_playing = now_s();
        if (objects.count == 0)
            return;
        if (tapped && [[NSSet setWithArray:objects] isSubsetOfSet:tapped] && sink >= 0)
            return;
        if (!taps_exist())
            return;
        NSMutableSet *all = [NSMutableSet setWithArray:objects];
        if (tapped)
            [all unionSet:tapped];
        start_tap(all.allObjects);
    });
}
