/*
 * libEGL.so.1, for Smithay, which opens EGL by the name Linux gives it
 * (smithay/src/backend/egl/ffi.rs). Android's is libEGL.so. This library is named libEGL.so.1 and
 * needs libEGL.so, so once the app has loaded it, a dlopen("libEGL.so.1") finds it by that name
 * and every symbol looked up through it is found in Android's own EGL -- but for the few below.
 *
 * Those give the room dmabuf import, which Android's EGL has not got, for the one kind of
 * "dmabuf" a client hands over on Android: an Android hardware buffer, sent through a socket
 * (AHardwareBuffer_sendHandleToUnixSocket) that stands as the plane's file, marked with
 * Spatiand's own modifier. SpatiMesh's Chromium hands the room its windows' pictures so
 * (third_party/chromium/android-libc/0020), and Smithay takes them as any dmabuf: the room's
 * dmabuf global offers RGBA and RGBX with that modifier (crates/spatiand/src/dmabuf.rs asks the
 * renderer, which asks here), and the image Smithay asks for is made from the hardware buffer
 * through EGL_ANDROID_image_native_buffer. Nothing is copied.
 *
 * Only Smithay sees these: the rest of the app reaches Android's EGL by its own name.
 */
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <android/hardware_buffer.h>
#include <dlfcn.h>
#include <poll.h>
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

/* Spatiand's own modifier: the plane's file is a socket a hardware buffer was sent through.
 * third_party/chromium/android-libc/0020 has the same number. */
#define SPATIAND_HARDWARE_BUFFER_MODIFIER 0xf500000000000001ull

#define FOURCC(a, b, c, d) \
    ((EGLint)((uint32_t)(a) | ((uint32_t)(b) << 8) | ((uint32_t)(c) << 16) | ((uint32_t)(d) << 24)))
static const EGLint formats[] = {FOURCC('A', 'B', '2', '4'), FOURCC('X', 'B', '2', '4')};
#define FORMAT_COUNT ((EGLint)(sizeof formats / sizeof formats[0]))

#define EXPORT __attribute__((visibility("default")))

static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;

/* Android's own, by name. */
static void *real(const char *name) {
    static void *egl;
    if (!egl) egl = dlopen("libEGL.so", RTLD_NOW | RTLD_NOLOAD);
    if (!egl) egl = dlopen("libEGL.so", RTLD_NOW);
    return egl ? dlsym(egl, name) : NULL;
}

/* A hardware buffer received, by the socket it came through, and how many images of it live. */
struct received {
    dev_t device;
    ino_t inode;
    AHardwareBuffer *buffer;
    int images;
};
/* An image made here, and the buffer it holds. */
struct made {
    EGLImageKHR image;
    AHardwareBuffer *buffer;
};
static struct received *receiveds;
static size_t received_count, received_room;
static struct made *mades;
static size_t made_count, made_room;

static struct received *find_received(dev_t device, ino_t inode) {
    for (size_t i = 0; i < received_count; i++)
        if (receiveds[i].device == device && receiveds[i].inode == inode) return &receiveds[i];
    return NULL;
}

static void drop_received(struct received *r) {
    AHardwareBuffer_release(r->buffer);
    *r = receiveds[--received_count];
}

/* The hardware buffer behind a plane's file, with a reference for the caller; NULL if there is
 * none. The buffer is in the socket once, the first time its client's buffer is imported; it is
 * kept by the socket's identity while images of it live. Called with the lock held. */
static AHardwareBuffer *hardware_buffer_of(int fd) {
    struct stat st;
    if (fstat(fd, &st) != 0 || !S_ISSOCK(st.st_mode)) return NULL;
    struct pollfd waiting = {fd, POLLIN, 0};
    if (poll(&waiting, 1, 0) == 1 && (waiting.revents & POLLIN)) {
        AHardwareBuffer *buffer = NULL;
        if (AHardwareBuffer_recvHandleFromUnixSocket(fd, &buffer) == 0 && buffer) {
            /* An earlier socket of the same identity is gone: this is a new one. */
            struct received *old = find_received(st.st_dev, st.st_ino);
            if (old && old->images == 0) drop_received(old);
            else if (old) old->inode = 0;
            if (received_count == received_room) {
                size_t room = received_room ? received_room * 2 : 16;
                struct received *grown = realloc(receiveds, room * sizeof *grown);
                if (!grown) {
                    AHardwareBuffer_release(buffer);
                    return NULL;
                }
                receiveds = grown;
                received_room = room;
            }
            receiveds[received_count++] = (struct received){st.st_dev, st.st_ino, buffer, 0};
        }
    }
    struct received *r = find_received(st.st_dev, st.st_ino);
    if (!r) return NULL;
    AHardwareBuffer_acquire(r->buffer);
    r->images++;
    return r->buffer;
}

/* An image made here is gone: its reference, and the received buffer's once none is left. Called
 * with the lock held. */
static void forget_image(AHardwareBuffer *buffer) {
    AHardwareBuffer_release(buffer);
    for (size_t i = 0; i < received_count; i++) {
        if (receiveds[i].buffer == buffer) {
            if (--receiveds[i].images <= 0) drop_received(&receiveds[i]);
            return;
        }
    }
}

EXPORT const char *eglQueryString(EGLDisplay display, EGLint name) {
    static const char *(*query)(EGLDisplay, EGLint);
    if (!query) query = (const char *(*)(EGLDisplay, EGLint))real("eglQueryString");
    const char *said = query ? query(display, name) : NULL;
    if (name != EGL_EXTENSIONS || display == EGL_NO_DISPLAY || !said) return said;
    /* Android's string for a display stays, so the longer one is made once for it. */
    static const char *from[4];
    static char *extended[4];
    const char *answer = said;
    pthread_mutex_lock(&lock);
    for (int i = 0; i < 4; i++) {
        if (from[i] == said) {
            answer = extended[i];
            break;
        }
        if (!from[i]) {
            static const char more[] =
                " EGL_EXT_image_dma_buf_import EGL_EXT_image_dma_buf_import_modifiers";
            char *longer = malloc(strlen(said) + sizeof more);
            if (longer) {
                strcpy(longer, said);
                strcat(longer, more);
                from[i] = said;
                extended[i] = longer;
                answer = longer;
            }
            break;
        }
    }
    pthread_mutex_unlock(&lock);
    return answer;
}

EXPORT EGLBoolean eglQueryDmaBufFormatsEXT(EGLDisplay display, EGLint most, EGLint *out,
                                           EGLint *count) {
    (void)display;
    if (!count) return EGL_FALSE;
    if (most <= 0 || !out) {
        *count = FORMAT_COUNT;
        return EGL_TRUE;
    }
    EGLint n = most < FORMAT_COUNT ? most : FORMAT_COUNT;
    for (EGLint i = 0; i < n; i++) out[i] = formats[i];
    *count = n;
    return EGL_TRUE;
}

EXPORT EGLBoolean eglQueryDmaBufModifiersEXT(EGLDisplay display, EGLint format, EGLint most,
                                             EGLuint64KHR *modifiers, EGLBoolean *external_only,
                                             EGLint *count) {
    (void)display;
    if (!count) return EGL_FALSE;
    int ours = 0;
    for (EGLint i = 0; i < FORMAT_COUNT; i++) ours |= formats[i] == format;
    if (!ours) {
        *count = 0;
        return EGL_TRUE;
    }
    if (most <= 0 || !modifiers) {
        *count = 1;
        return EGL_TRUE;
    }
    modifiers[0] = SPATIAND_HARDWARE_BUFFER_MODIFIER;
    if (external_only) external_only[0] = EGL_FALSE;
    *count = 1;
    return EGL_TRUE;
}

EXPORT EGLImageKHR eglCreateImageKHR(EGLDisplay display, EGLContext context, EGLenum target,
                                     EGLClientBuffer client_buffer, const EGLint *attributes) {
    static PFNEGLCREATEIMAGEKHRPROC create;
    static PFNEGLGETNATIVECLIENTBUFFERANDROIDPROC native;
    if (!create) create = (PFNEGLCREATEIMAGEKHRPROC)real("eglCreateImageKHR");
    if (!native) native = (PFNEGLGETNATIVECLIENTBUFFERANDROIDPROC)real("eglGetNativeClientBufferANDROID");
    if (!create) return EGL_NO_IMAGE_KHR;
    if (target != EGL_LINUX_DMA_BUF_EXT || !attributes || !native)
        return create(display, context, target, client_buffer, attributes);

    int fd = -1;
    uint64_t modifier_lo = 0, modifier_hi = 0;
    int planes = 0;
    for (const EGLint *a = attributes; a[0] != EGL_NONE; a += 2) {
        switch (a[0]) {
        case EGL_DMA_BUF_PLANE0_FD_EXT: fd = a[1]; planes |= 1; break;
        case EGL_DMA_BUF_PLANE1_FD_EXT: planes |= 2; break;
        case EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT: modifier_lo = (uint32_t)a[1]; break;
        case EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT: modifier_hi = (uint32_t)a[1]; break;
        default: break;
        }
    }
    if (planes != 1 || ((modifier_hi << 32) | modifier_lo) != SPATIAND_HARDWARE_BUFFER_MODIFIER)
        /* A real dmabuf, which Android's EGL refuses with its own error. */
        return create(display, context, target, client_buffer, attributes);

    pthread_mutex_lock(&lock);
    EGLImageKHR image = EGL_NO_IMAGE_KHR;
    AHardwareBuffer *buffer = hardware_buffer_of(fd);
    if (buffer) {
        static const EGLint preserved[] = {EGL_IMAGE_PRESERVED_KHR, EGL_TRUE, EGL_NONE};
        image = create(display, EGL_NO_CONTEXT, EGL_NATIVE_BUFFER_ANDROID, native(buffer), preserved);
        if (image != EGL_NO_IMAGE_KHR && made_count == made_room) {
            size_t room = made_room ? made_room * 2 : 16;
            struct made *grown = realloc(mades, room * sizeof *grown);
            if (grown) {
                mades = grown;
                made_room = room;
            }
        }
        if (image != EGL_NO_IMAGE_KHR && made_count < made_room) {
            mades[made_count++] = (struct made){image, buffer};
        } else {
            if (image != EGL_NO_IMAGE_KHR) {
                static PFNEGLDESTROYIMAGEKHRPROC destroy;
                if (!destroy) destroy = (PFNEGLDESTROYIMAGEKHRPROC)real("eglDestroyImageKHR");
                if (destroy) destroy(display, image);
                image = EGL_NO_IMAGE_KHR;
            }
            forget_image(buffer);
        }
    }
    pthread_mutex_unlock(&lock);
    return image;
}

EXPORT EGLBoolean eglDestroyImageKHR(EGLDisplay display, EGLImageKHR image) {
    static PFNEGLDESTROYIMAGEKHRPROC destroy;
    if (!destroy) destroy = (PFNEGLDESTROYIMAGEKHRPROC)real("eglDestroyImageKHR");
    EGLBoolean done = destroy ? destroy(display, image) : EGL_FALSE;
    pthread_mutex_lock(&lock);
    for (size_t i = 0; i < made_count; i++) {
        if (mades[i].image == image) {
            forget_image(mades[i].buffer);
            mades[i] = mades[--made_count];
            break;
        }
    }
    pthread_mutex_unlock(&lock);
    return done;
}

EXPORT __eglMustCastToProperFunctionPointerType eglGetProcAddress(const char *name) {
    static __eglMustCastToProperFunctionPointerType (*get)(const char *);
    if (!get) get = (__eglMustCastToProperFunctionPointerType(*)(const char *))real("eglGetProcAddress");
    if (name) {
        if (!strcmp(name, "eglQueryDmaBufFormatsEXT"))
            return (__eglMustCastToProperFunctionPointerType)eglQueryDmaBufFormatsEXT;
        if (!strcmp(name, "eglQueryDmaBufModifiersEXT"))
            return (__eglMustCastToProperFunctionPointerType)eglQueryDmaBufModifiersEXT;
        if (!strcmp(name, "eglCreateImageKHR"))
            return (__eglMustCastToProperFunctionPointerType)eglCreateImageKHR;
        if (!strcmp(name, "eglDestroyImageKHR"))
            return (__eglMustCastToProperFunctionPointerType)eglDestroyImageKHR;
        if (!strcmp(name, "eglQueryString"))
            return (__eglMustCastToProperFunctionPointerType)eglQueryString;
        /* Itself: Smithay loads every function through this, this one too, and loads the
         * rest through what it was given. */
        if (!strcmp(name, "eglGetProcAddress"))
            return (__eglMustCastToProperFunctionPointerType)eglGetProcAddress;
    }
    return get ? get(name) : NULL;
}
