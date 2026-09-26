#!/bin/sh
# Builds the id.prasetya.spatiand APK with the SDK's build-tools and cargo-ndk: no Gradle.
#
#   android/build.sh            build android/spatiand.apk
#   android/build.sh install    ...and install it on the device adb is talking to
#
# The Rust half is crates/spatiand-android, built with the rustup toolchain in ~/.cargo/bin
# (Homebrew's rust has no Android targets). Java libraries come from libs/fetch.sh.
set -eu
cd "$(dirname "$0")"
SDK=${ANDROID_HOME:-$HOME/Library/Android/sdk}
BT=$SDK/build-tools/36.0.0
JAR=$SDK/platforms/android-34/android.jar
NDK=${ANDROID_NDK_HOME:-$SDK/ndk/28.2.13676358}
OUT=build

# --- Rust ---
# The compositor's keyboard needs libxkbcommon, which Android has not got: built here, once.
xkbcommon/build.sh
PATH=$HOME/.cargo/bin:$PATH ANDROID_NDK_HOME=$NDK \
    RUSTFLAGS="-L native=$(pwd)/xkbcommon/out" \
    cargo ndk -t arm64-v8a -P 34 -o jniLibs build --release -p spatiand-android

# Smithay opens EGL as libEGL.so.1; this is that name for Android's: see eglshim/eglshim.c.
eglshim/build.sh

# --- Java ---
libs/fetch.sh
CLASSPATH=$(ls libs/*.jar 2>/dev/null | tr '\n' ':')
rm -rf $OUT && mkdir -p $OUT/classes $OUT/dex $OUT/apk/lib/arm64-v8a $OUT/assets
# The keyboard layouts, unpacked into the app's files on first start.
cp -R xkbcommon/out/xkb $OUT/assets/xkb
$BT/aapt2 compile --dir res -o $OUT/res.zip
$BT/aapt2 link -I $JAR --manifest AndroidManifest.xml -A $OUT/assets -o $OUT/base.apk $OUT/res.zip
javac --release 17 -Xlint:-options -classpath "$JAR:$CLASSPATH" -d $OUT/classes $(find src -name '*.java')
$BT/d8 --release --min-api 34 --lib $JAR --output $OUT/dex $(find $OUT/classes -name '*.class')

# --- the APK ---
cp jniLibs/arm64-v8a/libspatiand.so eglshim/out/libeglshim.so $OUT/apk/lib/arm64-v8a/
cp $OUT/dex/classes.dex $OUT/apk/
(cd $OUT/apk && zip -qr ../base.apk classes.dex lib)
$BT/zipalign -f -p 4 $OUT/base.apk $OUT/aligned.apk
$BT/apksigner sign --ks "$HOME/.android/debug.keystore" --ks-pass pass:android \
    --out spatiand.apk $OUT/aligned.apk 2>/dev/null
echo "built $(pwd)/spatiand.apk"

if [ "${1:-}" = install ]; then
    adb ${ANDROID_SERIAL:+-s "$ANDROID_SERIAL"} install -r spatiand.apk
fi
