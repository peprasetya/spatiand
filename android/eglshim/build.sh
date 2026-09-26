#!/bin/sh
# Builds libeglshim.so: see eglshim.c. Run by android/build.sh.
set -eu
cd "$(dirname "$0")"
SDK=${ANDROID_HOME:-$HOME/Library/Android/sdk}
NDK=${ANDROID_NDK_HOME:-$SDK/ndk/28.2.13676358}
CC="$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android34-clang"
mkdir -p out
"$CC" -shared -fPIC -o out/libeglshim.so eglshim.c \
    -Wl,-soname,libEGL.so.1 -Wl,--no-as-needed -lEGL
