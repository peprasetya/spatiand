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
# The NDK's tools are in a folder named for the machine building: a Mac's or Linux's.
HOST_TAG=$(case "$(uname -s)" in Darwin) echo darwin-x86_64 ;; *) echo linux-x86_64 ;; esac)
OUT=build

# --- Rust ---
# The compositor's keyboard needs libxkbcommon, which Android has not got: built here, once.
xkbcommon/build.sh
PATH=$HOME/.cargo/bin:$PATH ANDROID_NDK_HOME=$NDK \
    RUSTFLAGS="-L native=$(pwd)/xkbcommon/out" \
    cargo ndk -t arm64-v8a -P 34 -o jniLibs build --release -p spatiand-android

# Spatiand's own applications (docs/spatiand-apps.md): programs of their own, carried in the
# APK's library folder as lib<name>.so -- the only name Android extracts there -- and started
# by the compositor (crates/spatiand/src/own.rs). Three decode with FFmpeg, built here once
# for Android with MediaCodec in it; SpatiLoupe draws PDFs with PDFium; SpatiShelf reaches
# remotes through the storage service, which is Go and built for Android from this Mac.
ffmpeg/build.sh
FF=$(pwd)/ffmpeg/out
[ "$(uname -s)" = Darwin ] && export LIBCLANG_PATH=${LIBCLANG_PATH:-/Library/Developer/CommandLineTools/usr/lib}
PATH=$HOME/.cargo/bin:$PATH ANDROID_NDK_HOME=$NDK \
    FFMPEG_INCLUDE_DIR=$FF/include FFMPEG_LIBS_DIR=$FF/lib FFMPEG_LINK_MODE=dynamic \
    BINDGEN_EXTRA_CLANG_ARGS="--sysroot=$NDK/toolchains/llvm/prebuilt/$HOST_TAG/sysroot" \
    RUSTFLAGS="-L native=$(pwd)/xkbcommon/out -L native=$FF/lib -C link-arg=-lc++abi" \
    cargo ndk -t arm64-v8a -P 34 build --release -p spatireel -p spatiloupe -p spatiquill -p spatishelf -p spatimesh
../tools/fetch-pdfium.sh android-arm64
STORAGE=$(pwd)/$OUT-storage
mkdir -p "$STORAGE"
if command -v go >/dev/null; then
    ../storage/build.sh android "$STORAGE/libspatiand-storage.so"
else
    echo "warning: no Go here; the APK is made without the storage service (no remotes)"
fi

# The measured head for spatial sound: libmysofa and its dataset. See mysofa/build.sh.
mysofa/build.sh

# Smithay opens EGL as libEGL.so.1; this is that name for Android's: see eglshim/eglshim.c.
eglshim/build.sh

# --- Java ---
libs/fetch.sh
CLASSPATH=$(ls libs/*.jar 2>/dev/null | tr '\n' ':')
rm -rf $OUT && mkdir -p $OUT/classes $OUT/dex $OUT/apk/lib/arm64-v8a $OUT/assets
# The keyboard layouts, unpacked into the app's files on first start.
cp -R xkbcommon/out/xkb $OUT/assets/xkb
cp mysofa/out/default.sofa $OUT/assets/
$BT/aapt2 compile --dir res -o $OUT/res.zip
$BT/aapt2 link -I $JAR --manifest AndroidManifest.xml -A $OUT/assets -o $OUT/base.apk $OUT/res.zip
javac --release 17 -Xlint:-options -classpath "$JAR:$CLASSPATH" -d $OUT/classes $(find src -name '*.java')
$BT/d8 --release --min-api 34 --lib $JAR --output $OUT/dex $(find $OUT/classes -name '*.class')

# --- the APK ---
cp jniLibs/arm64-v8a/libspatiand.so eglshim/out/libeglshim.so mysofa/out/libmysofa.so $OUT/apk/lib/arm64-v8a/
for own in spatireel spatiloupe spatiquill spatishelf spatimesh; do
    cp ../target/aarch64-linux-android/release/$own $OUT/apk/lib/arm64-v8a/lib$own.so
done
for lib in avcodec avdevice avfilter avformat avutil swresample swscale; do
    cp "$FF/lib/lib$lib.so" $OUT/apk/lib/arm64-v8a/
done
cp ../third_party/pdfium/android-arm64/lib/libpdfium.so $OUT/apk/lib/arm64-v8a/
[ -f "$STORAGE/libspatiand-storage.so" ] && cp "$STORAGE/libspatiand-storage.so" $OUT/apk/lib/arm64-v8a/
cp $OUT/dex/classes.dex $OUT/apk/
(cd $OUT/apk && zip -qr ../base.apk classes.dex lib)
$BT/zipalign -f -p 4 $OUT/base.apk $OUT/aligned.apk
# Signed with the debug key Android Studio makes; made here the same way where there is none
# (a machine that never had Android Studio, such as CI).
if [ ! -f "$HOME/.android/debug.keystore" ]; then
    mkdir -p "$HOME/.android"
    keytool -genkeypair -keystore "$HOME/.android/debug.keystore" -storepass android -keypass android \
        -alias androiddebugkey -dname "CN=Android Debug,O=Android,C=US" -keyalg RSA -keysize 2048 -validity 10000 >/dev/null
fi
$BT/apksigner sign --ks "$HOME/.android/debug.keystore" --ks-pass pass:android \
    --out spatiand.apk $OUT/aligned.apk 2>/dev/null
echo "built $(pwd)/spatiand.apk"

if [ "${1:-}" = install ]; then
    adb ${ANDROID_SERIAL:+-s "$ANDROID_SERIAL"} install -r spatiand.apk
fi
