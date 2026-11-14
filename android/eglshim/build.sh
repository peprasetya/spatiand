#!/bin/sh
# Builds libeglshim.so: see eglshim.c. Run by android/build.sh.
set -eu
cd "$(dirname "$0")"
. ../ndk.sh
NDK=$SPATIAND_NDK
# The NDK's tools are in a folder named for the machine building: a Mac's or Linux's.
HOST_TAG=$(case "$(uname -s)" in (Darwin) echo darwin-x86_64 ;; (*) echo linux-x86_64 ;; esac)
CC="$NDK/toolchains/llvm/prebuilt/$HOST_TAG/bin/aarch64-linux-android34-clang"
mkdir -p out
"$CC" -shared -fPIC -O2 -Wall -Wextra -fvisibility=hidden -o out/libeglshim.so eglshim.c \
    -Wl,-soname,libEGL.so.1 -Wl,--no-as-needed -lEGL -lnativewindow -ldl
