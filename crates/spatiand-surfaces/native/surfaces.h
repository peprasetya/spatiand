/*
 * Pictures on the GPU (IOSurfaces) handed from a process to its parent in private, on macOS.
 *
 * A global IOSurface can be opened by any process on the Mac that knows or guesses its number,
 * and read: a page, a password being typed. These are not global. The parent registers a Mach
 * service of its own (surfaces_listen) and names it to its children in an environment variable;
 * a child sends each picture's Mach port there with a number of its choosing (surfaces_post),
 * and the parent opens it by that number (surfaces_open). The parent takes pictures only from
 * its own children, as the kernel says who sent them.
 *
 * The numbers still travel the way they did (a socket, a Wayland buffer's name); they are only
 * names now, worthless to anyone else.
 *
 * Built twice into some processes (SpatiMesh's WPE engine program: Spatiand's own copy, and
 * WebKit's for its processes), so everything is static or prefixed, and each copy has its own
 * variable: SURFACES_ENV and SURFACES_PREFIX are given at build time.
 */

#pragma once

#include <IOSurface/IOSurfaceRef.h>
#include <stdint.h>

#ifndef SURFACES_PREFIX
#define SURFACES_PREFIX spatiand_
#endif
#define SURFACES_JOIN2(a, b) a##b
#define SURFACES_JOIN(a, b) SURFACES_JOIN2(a, b)
#define SURFACES_NAME(name) SURFACES_JOIN(SURFACES_PREFIX, name)

#ifdef __cplusplus
extern "C" {
#endif

/* Take pictures from this process's children from now on: once; later calls do nothing. The
 * children started after it are told where (the environment variable). The parent's own place,
 * as this process was told it, is kept first for surfaces_post. 0 if it could not be done. */
int SURFACES_NAME(surfaces_listen)(void);

/* Hand `surface` to the parent, as `number`. 0 if there is no parent listening, or it failed. */
int SURFACES_NAME(surfaces_post)(IOSurfaceRef surface, uint32_t number);

/* The picture a child handed over as `number`, retained (CFRelease it), or NULL. A picture is
 * kept for a few seconds after it was last handed over, so it can be opened more than once. */
IOSurfaceRef SURFACES_NAME(surfaces_open)(uint32_t number);

#ifdef __cplusplus
}
#endif
