#!/bin/sh
# libmysofa for Android, and the measured head it reads: what makes a window's sound able to
# come from behind (crates/spatiand-audio/src/hrtf.rs), as it does on the Deck.
#
#   android/mysofa/build.sh      -> out/libmysofa.so and out/default.sofa
#
# Upstream's own release, built with its CMake and the NDK's toolchain, and pinned by SHA-256.
# Android's toolchain gives it no version in its name, so hrtf.rs opens libmysofa.so there;
# the app ships it beside libspatiand.so, where that name is found. The dataset is the MIT KEMAR one libmysofa ships as its
# default, the same measured head SteamOS installs. Run by android/build.sh.
set -eu
cd "$(dirname "$0")"
VERSION=1.3.5
SHA=f29508c335c83d8703f943ffc9ca783ac39aca84e851357f13a55af0f8143137
SDK=${ANDROID_HOME:-$HOME/Library/Android/sdk}
NDK=${ANDROID_NDK_HOME:-$SDK/ndk/28.2.13676358}

[ -f out/libmysofa.so ] && [ -f out/default.sofa ] && exit 0

if [ ! -f cache/libmysofa-$VERSION.tar.gz ]; then
    mkdir -p cache
    curl -sSfL -o cache/part "https://github.com/hoene/libmysofa/archive/refs/tags/v$VERSION.tar.gz"
    mv cache/part cache/libmysofa-$VERSION.tar.gz
fi
echo "$SHA  cache/libmysofa-$VERSION.tar.gz" | shasum -a 256 -c - >/dev/null || { echo "libmysofa: checksum mismatch"; exit 1; }

rm -rf work out && mkdir -p work out
tar -C work -xzf cache/libmysofa-$VERSION.tar.gz
cmake -S work/libmysofa-$VERSION -B work/build -G Ninja \
    -DCMAKE_TOOLCHAIN_FILE="$NDK/build/cmake/android.toolchain.cmake" \
    -DANDROID_ABI=arm64-v8a -DANDROID_PLATFORM=android-34 \
    -DCMAKE_BUILD_TYPE=Release -DBUILD_TESTS=OFF -DBUILD_STATIC_LIBS=OFF >/dev/null
cmake --build work/build --target mysofa-shared >/dev/null
cp -L work/build/src/libmysofa.so out/libmysofa.so
cp -L work/libmysofa-$VERSION/share/default.sofa out/default.sofa
rm -rf work
echo "built $(pwd)/out/libmysofa.so and out/default.sofa"
