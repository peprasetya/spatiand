#!/usr/bin/env python3
"""The GPU's resources stay macOS's in the Linux UI on macOS.

The rule of the port (docs/spatimesh-mac-port.md): the UI follows Linux's path, the system
below it stays macOS's. The GPU process's images are the system's: on macOS they are IOSurfaces
(IOSurfaceImageBacking, Metal through ANGLE), on Linux native pixmaps (dma-buf, GBM, Vulkan).
Chromium's shared-image code picks one by `#elif BUILDFLAG(IS_OZONE)` *before*
`#elif BUILDFLAG(IS_APPLE)`, so with Ozone on macOS it would take Linux's and lose the Mac's.

So in the shared-image core below, an Ozone condition says it is not macOS's:

    BUILDFLAG(IS_OZONE)  ->  (BUILDFLAG(IS_OZONE) && !BUILDFLAG(IS_APPLE))

on `#if`/`#elif` lines only. What the window shows still goes through Ozone (Wayland's surface
factory, in the GPU process and in viz): that is the UI's, and is not touched here. The Ozone
image backings themselves are left out of the Mac's build in gpu/command_buffer/service/BUILD.gn
(by hand, `# Spatiand:`).

    gpu-mac.py          (in the Chromium tree's src; safe to run again)
"""

import re

FILES = [
    "gpu/command_buffer/service/shared_image/shared_image_factory.h",
    "gpu/command_buffer/service/shared_image/shared_image_factory.cc",
    "gpu/command_buffer/service/shared_image/shared_image_representation.h",
    "gpu/command_buffer/service/shared_image/shared_image_representation.cc",
    "gpu/command_buffer/service/shared_image/shared_image_manager.h",
    "gpu/command_buffer/service/shared_image/shared_image_manager.cc",
    # viz: what asks the shared images for native pixmaps
    "components/viz/service/display/overlay_processor_interface.cc",
    "components/viz/service/gl/gpu_service_impl.cc",
]

OZONE = re.compile(r"(?<!\()BUILDFLAG\(IS_OZONE\)(?! && !BUILDFLAG\(IS_APPLE\))")


def main():
    changed = 0
    for path in FILES:
        lines = open(path).read().split("\n")
        out = []
        for line in lines:
            if line.lstrip().startswith(("#if", "#elif")):
                line = OZONE.sub("(BUILDFLAG(IS_OZONE) && !BUILDFLAG(IS_APPLE))", line)
            out.append(line)
        text = "\n".join(out)
        if text != "\n".join(lines):
            open(path, "w").write(text)
            changed += 1
    print(f"{changed} of {len(FILES)} files changed")


if __name__ == "__main__":
    main()
