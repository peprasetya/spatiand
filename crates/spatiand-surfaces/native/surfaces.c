/* See surfaces.h. */

#include "surfaces.h"

#include <CoreFoundation/CoreFoundation.h>
#include <IOSurface/IOSurface.h>
#include <bootstrap.h>
#include <bsm/libbsm.h>
#include <libproc.h>
#include <mach/mach.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

#ifndef SURFACES_ENV
#define SURFACES_ENV "SPATIAND_SURFACES"
#endif
#define SURFACES_STRING2(x) #x
#define SURFACES_STRING(x) SURFACES_STRING2(x)

/* How long a picture is kept after it was last handed over or opened. */
#define KEPT_SECONDS 5.0
/* How long surfaces_open waits for a picture it has not got: the child sends it before it
 * names it, but this process's receiving thread may not have taken it in yet. */
#define OPEN_WAIT_SECONDS 0.25
#define MOST_KEPT 256

typedef struct {
    mach_msg_header_t header;
    mach_msg_body_t body;
    mach_msg_port_descriptor_t surface;
    uint32_t number;
} Post;

typedef struct {
    Post post;
    mach_msg_audit_trailer_t trailer;
} Received;

typedef struct {
    uint32_t number;
    IOSurfaceRef surface;
    double when;
} Kept;

static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t arrived = PTHREAD_COND_INITIALIZER;
static int listening;
static mach_port_t service = MACH_PORT_NULL;
static Kept kept[MOST_KEPT];
static int parent_read;
static char parent_name[128];
static mach_port_t parent_port = MACH_PORT_NULL;

static double now(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec + t.tv_nsec / 1e9;
}

/* Where this process's parent takes pictures, as it was told: read once, before this process
 * names its own place to its children in the same variable. Called with the lock held. */
static void read_parent(void) {
    if (parent_read)
        return;
    parent_read = 1;
    const char *name = getenv(SURFACES_ENV);
    if (name && strlen(name) < sizeof parent_name)
        strcpy(parent_name, name);
}

/* Whether `pid` is a child of this process. */
static int is_child(pid_t pid) {
    struct proc_bsdinfo info;
    if (proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, sizeof info) != (int)sizeof info)
        return 0;
    return (pid_t)info.pbi_ppid == getpid();
}

/* Keep `surface` (adopted) as `number`, in place of what was kept as it, and let go of what
 * has not been wanted for a while. Called with the lock held. */
static void keep(uint32_t number, IOSurfaceRef surface) {
    double t = now();
    int free_slot = -1, oldest = 0;
    for (int i = 0; i < MOST_KEPT; i++) {
        if (kept[i].surface && t - kept[i].when > KEPT_SECONDS) {
            CFRelease(kept[i].surface);
            kept[i].surface = NULL;
        }
        if (kept[i].surface && kept[i].number == number) {
            CFRelease(kept[i].surface);
            kept[i].surface = NULL;
        }
        if (!kept[i].surface && free_slot < 0)
            free_slot = i;
        if (kept[i].surface && kept[i].when < kept[oldest].when)
            oldest = i;
    }
    if (free_slot < 0) {
        CFRelease(kept[oldest].surface);
        free_slot = oldest;
    }
    kept[free_slot] = (Kept) { number, surface, t };
}

static void *receive(void *unused) {
    (void)unused;
    for (;;) {
        Received message;
        memset(&message, 0, sizeof message);
        kern_return_t kr = mach_msg(&message.post.header,
            MACH_RCV_MSG | MACH_RCV_TRAILER_TYPE(MACH_MSG_TRAILER_FORMAT_0) | MACH_RCV_TRAILER_ELEMENTS(MACH_RCV_TRAILER_AUDIT),
            0, sizeof message, service, MACH_MSG_TIMEOUT_NONE, MACH_PORT_NULL);
        if (kr == MACH_RCV_TOO_LARGE)
            continue;
        if (kr != KERN_SUCCESS)
            break;
        int complex = (message.post.header.msgh_bits & MACH_MSGH_BITS_COMPLEX) != 0;
        mach_port_t port = complex && message.post.body.msgh_descriptor_count >= 1 ? message.post.surface.name : MACH_PORT_NULL;
        int whole = complex && message.post.header.msgh_size >= sizeof(Post) && message.post.body.msgh_descriptor_count == 1
            && message.post.surface.type == MACH_MSG_PORT_DESCRIPTOR;
        /* From one of this process's own children, as the kernel says: no one else's. */
        if (whole && is_child(audit_token_to_pid(message.trailer.msgh_audit))) {
            IOSurfaceRef surface = IOSurfaceLookupFromMachPort(port);
            if (surface) {
                pthread_mutex_lock(&lock);
                keep(message.post.number, surface);
                pthread_cond_broadcast(&arrived);
                pthread_mutex_unlock(&lock);
            }
        }
        /* The rights that came with it (the port, opened above or not) go. */
        mach_msg_destroy(&message.post.header);
    }
    return NULL;
}

int SURFACES_NAME(surfaces_listen)(void) {
    pthread_mutex_lock(&lock);
    read_parent();
    if (listening) {
        pthread_mutex_unlock(&lock);
        return 1;
    }
    char name[128];
    snprintf(name, sizeof name, "id.prasetya.spatiand.surfaces.%d.%s", (int)getpid(), SURFACES_STRING(SURFACES_PREFIX));
    kern_return_t kr = bootstrap_check_in(bootstrap_port, name, &service);
    if (kr != KERN_SUCCESS) {
        pthread_mutex_unlock(&lock);
        return 0;
    }
    /* Room for every picture of a busy child, so that a child never waits on this one. */
    mach_port_limits_t limits = { MACH_PORT_QLIMIT_LARGE };
    mach_port_set_attributes(mach_task_self(), service, MACH_PORT_LIMITS_INFO, (mach_port_info_t)&limits, MACH_PORT_LIMITS_INFO_COUNT);
    /* A process's first use of IOSurface takes about a second (its connection is set up): done
     * here, so that the first picture a child hands over is not that late. */
    IOSurfaceGetPropertyMaximum(kIOSurfaceWidth);
    pthread_t thread;
    if (pthread_create(&thread, NULL, receive, NULL)) {
        pthread_mutex_unlock(&lock);
        return 0;
    }
    pthread_detach(thread);
    listening = 1;
    setenv(SURFACES_ENV, name, 1);
    pthread_mutex_unlock(&lock);
    return 1;
}

int SURFACES_NAME(surfaces_post)(IOSurfaceRef surface, uint32_t number) {
    if (!surface)
        return 0;
    pthread_mutex_lock(&lock);
    read_parent();
    for (int attempt = 0; attempt < 2; attempt++) {
        if (parent_port == MACH_PORT_NULL) {
            if (!parent_name[0] || bootstrap_look_up(bootstrap_port, parent_name, &parent_port) != KERN_SUCCESS) {
                parent_port = MACH_PORT_NULL;
                break;
            }
        }
        Post post;
        memset(&post, 0, sizeof post);
        post.header.msgh_bits = MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, 0) | MACH_MSGH_BITS_COMPLEX;
        post.header.msgh_size = sizeof post;
        post.header.msgh_remote_port = parent_port;
        post.body.msgh_descriptor_count = 1;
        post.surface.name = IOSurfaceCreateMachPort(surface);
        post.surface.disposition = MACH_MSG_TYPE_MOVE_SEND;
        post.surface.type = MACH_MSG_PORT_DESCRIPTOR;
        post.number = number;
        kern_return_t kr = mach_msg(&post.header, MACH_SEND_MSG | MACH_SEND_TIMEOUT, sizeof post, 0, MACH_PORT_NULL, 100, MACH_PORT_NULL);
        if (kr == KERN_SUCCESS) {
            pthread_mutex_unlock(&lock);
            return 1;
        }
        /* Not sent: the port is still ours to let go of. */
        mach_port_deallocate(mach_task_self(), post.surface.name);
        if (kr != MACH_SEND_INVALID_DEST)
            break;
        /* The parent's service went (and may be back): look it up again, once. */
        mach_port_deallocate(mach_task_self(), parent_port);
        parent_port = MACH_PORT_NULL;
    }
    pthread_mutex_unlock(&lock);
    return 0;
}

IOSurfaceRef SURFACES_NAME(surfaces_open)(uint32_t number) {
    IOSurfaceRef found = NULL;
    pthread_mutex_lock(&lock);
    double until = now() + OPEN_WAIT_SECONDS;
    while (listening) {
        for (int i = 0; i < MOST_KEPT; i++) {
            if (kept[i].surface && kept[i].number == number) {
                found = (IOSurfaceRef)CFRetain(kept[i].surface);
                kept[i].when = now();
                break;
            }
        }
        double left = until - now();
        if (found || left <= 0)
            break;
        struct timeval tv;
        gettimeofday(&tv, NULL);
        double at = tv.tv_sec + tv.tv_usec / 1e6 + left;
        struct timespec deadline = { (time_t)at, (long)((at - (time_t)at) * 1e9) };
        pthread_cond_timedwait(&arrived, &lock, &deadline);
    }
    pthread_mutex_unlock(&lock);
    return found;
}
