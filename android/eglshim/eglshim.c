/*
 * libEGL.so.1, for Smithay, which opens EGL by the name Linux gives it
 * (smithay/src/backend/egl/ffi.rs). Android's is libEGL.so. This library has no code: it is
 * named libEGL.so.1 and needs libEGL.so, so once the app has loaded it, a dlopen("libEGL.so.1")
 * finds it by that name and every symbol looked up through it is found in Android's own EGL.
 */
