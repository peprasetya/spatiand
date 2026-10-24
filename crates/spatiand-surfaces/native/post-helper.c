/* For the crate's test: a child that makes a small private surface, hands it to its parent as
 * the number given (argv[1]), says the surface's ID, and waits to be ended. */

#include "surfaces.h"

#include <IOSurface/IOSurface.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 2)
        return 2;
    int width = 16, height = 16, bytes = 4;
    CFNumberRef w = CFNumberCreate(NULL, kCFNumberIntType, &width);
    CFNumberRef h = CFNumberCreate(NULL, kCFNumberIntType, &height);
    CFNumberRef b = CFNumberCreate(NULL, kCFNumberIntType, &bytes);
    const void *keys[] = { kIOSurfaceWidth, kIOSurfaceHeight, kIOSurfaceBytesPerElement };
    const void *values[] = { w, h, b };
    CFDictionaryRef properties = CFDictionaryCreate(NULL, keys, values, 3, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    IOSurfaceRef surface = IOSurfaceCreate(properties);
    if (!surface || !spatiand_surfaces_post(surface, (uint32_t)strtoul(argv[1], NULL, 10)))
        return 1;
    printf("posted %u\n", IOSurfaceGetID(surface));
    fflush(stdout);
    sleep(30);
    return 0;
}
