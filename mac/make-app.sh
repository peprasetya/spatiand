#!/bin/sh
# Build Spatiand.app: a menu-bar app with no Dock icon, ad-hoc signed so it runs on this Mac.
# The result is ./Spatiand.app. Not notarised, so it is for this Mac, not for handing out.
set -e
cd "$(dirname "$0")"
[ -f xkbcommon/out/libxkbcommon.a ] || xkbcommon/build.sh
[ -f vendor/angle/libEGL.dylib ] || ./fetch-angle.sh
(cd ../crates/spatiand-mac && MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --release)
rm -f .build/release/Spatiand
SPATIAND_CORE=release swift build -c release
# Put together in a scratch folder and installed at ~/Applications afterwards. ~/Documents is
# synced by iCloud, which puts attributes on every file that a signature refuses, and an app that
# lives at a fixed place outside it is also what macOS wants for a permission it is to remember.
stage=$(mktemp -d)
app="$stage/Spatiand.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp .build/release/Spatiand "$app/Contents/MacOS/Spatiand"
# Spatiand's own applications, each a program beside the app's, which is where the launcher
# looks for them (crates/spatiand/src/own.rs). SpatiReel, SpatiLoupe, SpatiShelf and SpatiMesh
# link **the FFmpeg the app carries** (third_party/ffmpeg: the same version on every platform,
# LGPL), not one the Mac happens to have: it is in Contents/Frameworks, found by the programs'
# own search path (.cargo/config.toml). bindgen wants to be told where libclang is. Each is built on its own, so that
# one that does not build leaves the others.
../third_party/ffmpeg/build.sh mac
FFMPEG=$(cd ../third_party/ffmpeg/out/mac && pwd)
mkdir -p "$app/Contents/Frameworks"
for lib in "$FFMPEG"/lib/lib*.[0-9]*.dylib; do
  case "$lib" in *.*.*.*.dylib) continue ;; esac   # libavcodec.63.dylib, not libavcodec.63.1.102.dylib
  cp "$lib" "$app/Contents/Frameworks/"
done
# And what the licences ask to go with them.
mkdir -p "$app/Contents/Resources/licences"
cp ../third_party/NOTICES.md "$app/Contents/Resources/licences/"
. ../third_party/ffmpeg/versions.sh
cp "../third_party/ffmpeg/src/ffmpeg-$FFMPEG_VERSION/COPYING.LGPLv3" "$app/Contents/Resources/licences/"
cp "../third_party/ffmpeg/src/openssl-$OPENSSL_VERSION/LICENSE.txt" "$app/Contents/Resources/licences/OpenSSL-LICENSE.txt"
cp Resources/hrtf/NOTICE.txt "$app/Contents/Resources/licences/mysofa-NOTICE.txt" 2>/dev/null || true
OWN="spatireel spatiloupe spatiquill spatishelf spatimesh"
for own in $OWN; do
  if (cd .. && LIBCLANG_PATH="${LIBCLANG_PATH:-/Library/Developer/CommandLineTools/usr/lib}" \
      FFMPEG_PKG_CONFIG_PATH="$FFMPEG/lib/pkgconfig" \
      MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --release -p "$own"); then
    cp "../target/release/$own" "$app/Contents/MacOS/$own"
  else
    echo "warning: $own did not build; the app is made without it"
  fi
done
# SpatiMesh's engine: WPE WebKit, which third_party/wpe/build.sh mac makes (SpatiMesh has no
# other on a Mac), in a program of its own beside SpatiMesh, which starts one for each profile in
# use (docs/spatimesh.md). WPE and every library it was built on go into
# Contents/Frameworks/wpe, laid out as the build's prefix (lib, libexec) so WebKit, which
# finds its processes and modules from where its library was loaded, finds them in the app (its
# data, the developer tools' pages, is in Resources: data may not be among code); the
# dots in folder names are underscores there (wpe-webkit-2_0), since codesign takes a dotted
# folder for a bundle (the WebKit patch looks for them so). The libraries name each other by the
# build folder's path; here they are renamed to @rpath.
WPE_OUT="$HOME/Developer/spatiand-wpe/out/mac"
WPE_CODE=""
if [ -f "$WPE_OUT/lib/pkgconfig/wpe-webkit-2.0.pc" ] && [ -f "$app/Contents/MacOS/spatimesh" ]; then
  if (cd .. && MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --release -p spatimesh-wpe); then
    wpe="$app/Contents/Frameworks/wpe"
    mkdir -p "$wpe/lib" "$wpe/libexec"
    cp -a "$WPE_OUT"/lib/*.dylib "$wpe/lib/"
    for dir in gio wpe-webkit-2.0 wpe-platform-2.0 gstreamer-1.0; do if [ -d "$WPE_OUT/lib/$dir" ]; then cp -a "$WPE_OUT/lib/$dir" "$wpe/lib/$(echo $dir | tr . _)"; fi; done
    cp -a "$WPE_OUT/libexec/wpe-webkit-2.0" "$wpe/libexec/wpe-webkit-2_0"
    rm -rf "$wpe/lib/gstreamer-1_0/include"   # GStreamer GL's headers: data, not for the app
    # GStreamer's program that looks into its plugins (the engine program tells GStreamer where).
    mkdir -p "$wpe/libexec/gstreamer-1_0"
    cp "$WPE_OUT/libexec/gstreamer-1.0/gst-plugin-scanner" "$wpe/libexec/gstreamer-1_0/"
    # What the app carries already (OpenSSL from FFmpeg's build, ANGLE, put in further down) is
    # carried once.
    for lib in "$wpe"/lib/*.dylib; do
      name=$(basename "$lib")
      if [ -e "$app/Contents/Frameworks/$name" ] || [ -e "vendor/angle/$name" ]; then rm "$lib"; ln -s "../../$name" "$lib"; fi
    done
    cp ../target/release/spatimesh-wpe "$app/Contents/MacOS/spatimesh-wpe"
    # The developer tools' pages: data, so in Resources (the engine program tells WebKit).
    if [ -f "$WPE_OUT/share/wpe-webkit-2.0/inspector.gresource" ]; then
      mkdir -p "$app/Contents/Resources/wpe"
      cp "$WPE_OUT/share/wpe-webkit-2.0/inspector.gresource" "$app/Contents/Resources/wpe/"
    fi
    cp ../third_party/wpe/mac/fonts.conf "$app/Contents/Resources/fonts.conf"
    for f in $(find "$wpe" -type f \( -name '*.dylib' -o -name '*.so' -o -perm +111 \)) "$app/Contents/MacOS/spatimesh-wpe"; do
      file "$f" | grep -q Mach-O || continue
      case "$f" in *.dylib) install_name_tool -id "@rpath/$(basename "$f")" "$f" 2>/dev/null ;; esac
      for dep in $(otool -L "$f" | tail -n +2 | awk '{print $1}' | grep "^$WPE_OUT/lib/"); do
        install_name_tool -change "$dep" "@rpath/${dep#"$WPE_OUT/lib/"}" "$f" 2>/dev/null
      done
      otool -l "$f" | grep -A2 LC_RPATH | awk '/path /{print $2}' | grep -e "^$WPE_OUT" -e "/third_party/ffmpeg/" | while read -r old; do
        install_name_tool -delete_rpath "$old" "$f" 2>/dev/null
      done
      WPE_CODE="$WPE_CODE $f"
    done
    for lib in "$wpe"/lib/*.dylib; do
      [ -L "$lib" ] || otool -l "$lib" | grep -q '@loader_path$' || install_name_tool -add_rpath @loader_path "$lib" 2>/dev/null || true
    done
    # GStreamer's plugins, one folder below lib, and its scanner.
    for plugin in "$wpe"/lib/gstreamer-1_0/*.dylib; do
      otool -l "$plugin" | grep -q 'path @loader_path/.. ' || install_name_tool -add_rpath @loader_path/.. "$plugin" 2>/dev/null || true
    done
    install_name_tool -add_rpath @loader_path/../../lib "$wpe/libexec/gstreamer-1_0/gst-plugin-scanner" 2>/dev/null || true
    # Plug-ins two folders below lib: GIO's TLS module, WebKit's injected bundle.
    for so in $(find "$wpe/lib" -name '*.so'); do
      otool -l "$so" | grep -q '@loader_path/../..' || install_name_tool -add_rpath @loader_path/../.. "$so" 2>/dev/null || true
    done
    install_name_tool -add_rpath @executable_path/../Frameworks/wpe/lib "$app/Contents/MacOS/spatimesh-wpe"
    leftover=$(for f in $WPE_CODE; do otool -L "$f" | grep -e "$WPE_OUT" -e /usr/local -e /opt/homebrew || true; done)
    [ -z "$leftover" ] || { echo "warning: WPE in the app still names the build folder or Homebrew:"; echo "$leftover"; }
    OWN="$OWN spatimesh-wpe"
  else
    echo "warning: spatimesh-wpe did not build; SpatiMesh will show no pages"
  fi
fi
# The storage service the applications reach remotes through: Go, with rclone linked in.
if command -v go >/dev/null && ../storage/build.sh mac "$app/Contents/MacOS/spatiand-storage"; then
  OWN="$OWN spatiand-storage"
else
  echo "warning: the storage service did not build (is Go installed?); remotes will not be reachable"
fi
# PDFium, which SpatiLoupe draws PDFs with, beside the programs that load it.
case "$(uname -m)" in arm64) pdfium=mac-arm64 ;; *) pdfium=mac-x64 ;; esac
../tools/fetch-pdfium.sh "$pdfium" && mkdir -p "$app/Contents/Frameworks" \
  && cp "../third_party/pdfium/$pdfium/lib/libpdfium.dylib" "$app/Contents/Frameworks/" \
  || echo "warning: no PDFium; SpatiLoupe will not open PDFs"
# The measured head: the library that reads SOFA files and the dataset (built once; see tools/build-hrtf.sh).
./tools/build-hrtf.sh || echo "warning: no measured head; sound will be placed by the parametric one"
if [ -f Resources/hrtf/libmysofa.1.dylib ]; then
  mkdir -p "$app/Contents/Frameworks" "$app/Contents/Resources"
  cp Resources/hrtf/libmysofa.1.dylib "$app/Contents/Frameworks/"
  cp Resources/hrtf/default.sofa Resources/hrtf/NOTICE.txt "$app/Contents/Resources/"
fi
# What the compositor draws with and types with: ANGLE (OpenGL ES on Metal) beside the program,
# and the keyboard layouts libxkbcommon reads.
mkdir -p "$app/Contents/Frameworks" "$app/Contents/Resources"
cp vendor/angle/libEGL.dylib vendor/angle/libGLESv2.dylib "$app/Contents/Frameworks/"
cp -R xkbcommon/out/xkb "$app/Contents/Resources/xkb"
cp Resources/Spatiand.icns "$app/Contents/Resources/Spatiand.icns"
# What the launcher shows for this Mac and for another computer: looked up as a Linux desktop
# would look them up, in a theme of the app's own.
cp -R icons "$app/Contents/Resources/icons"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Spatiand</string>
    <key>CFBundleIconFile</key><string>Spatiand</string>
    <key>CFBundleDisplayName</key><string>Spatiand</string>
    <key>CFBundleIdentifier</key><string>com.peprasetya.spatiand</string>
    <key>CFBundleExecutable</key><string>Spatiand</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.1</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <!-- A menu-bar app: no Dock icon, no main window. -->
    <key>LSUIElement</key><true/>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
    <key>NSAudioCaptureUsageDescription</key><string>Spatiand places a Mac application's sound where its window is in the glasses, which needs to take that sound from the speakers.</string>
    <key>NSLocalNetworkUsageDescription</key><string>Spatiand's applications reach your own servers on this network, such as the film library SpatiReel plays from.</string>
    <key>NSMicrophoneUsageDescription</key><string>Spatiand sends your voice to an application on another computer when that application asks for the microphone.</string>
</dict>
</plist>
PLIST

# Signing identity. Ad-hoc signing gives the bundle a new code hash on every build, and macOS binds
# Screen Recording and Accessibility to that hash, so each rebuild would silently take both away.
# HoloFrame's self-signed "HoloFrame Dev" certificate is used when it is in the keychain: the
# permission is then bound to the bundle's name and that certificate, which do not change. See
# HoloFrame's Tools/make-app.sh for how such a certificate is made, and why it is searched for
# without `-v` (a self-signed root is never "valid" to the trust policy, and signs fine).
# Another checkout building its own app sets this, so as not to replace this one.
dest="${SPATIAND_APP_DEST:-$HOME/Applications/Spatiand.app}"
mkdir -p "$(dirname "$dest")"
rm -rf "$dest"
ditto --noextattr --norsrc "$app" "$dest"
rm -rf "$stage"
# Signed where it is installed: a signed bundle copied elsewhere lost its signature (2026-10-25).
stage_app="$app"
app="$dest"
WPE_CODE=$(for f in $WPE_CODE; do echo "$dest${f#"$stage_app"}"; done)
# A signature refuses a bundle with extended attributes on it (a copied binary carries some).
xattr -cr "$app" 2>/dev/null || true
IDENTITY_NAME="${SPATIAND_SIGN_NAME:-HoloFrame Dev}"
IDENTITY=$(security find-identity -p codesigning 2>/dev/null | grep -F "\"$IDENTITY_NAME" \
  | sed -E 's/^[[:space:]]*[0-9]+\)[[:space:]]*([0-9A-F]+)[[:space:]]+.*$/\1/' | head -1)
if [ -n "$IDENTITY" ]; then
  # What is inside is signed first, with the same certificate.
  for lib in "$app"/Contents/Frameworks/*.dylib $WPE_CODE; do [ -f "$lib" ] && codesign --force --sign "$IDENTITY" "$lib" >/dev/null 2>&1; done
  for own in $OWN; do [ -f "$app/Contents/MacOS/$own" ] && codesign --force --sign "$IDENTITY" "$app/Contents/MacOS/$own" >/dev/null 2>&1; done
  codesign --force --sign "$IDENTITY" --identifier com.peprasetya.spatiand "$app" >/dev/null 2>&1 \
    && echo "signed with \"$IDENTITY_NAME\" ($IDENTITY): permissions survive rebuilds" \
    || { echo "warning: could not sign with \"$IDENTITY_NAME\"; signing ad hoc"; codesign --force --sign - "$app" >/dev/null 2>&1 || true; }
else
  for lib in "$app"/Contents/Frameworks/*.dylib $WPE_CODE; do [ -f "$lib" ] && codesign --force --sign - "$lib" >/dev/null 2>&1; done
  for own in $OWN; do [ -f "$app/Contents/MacOS/$own" ] && codesign --force --sign - "$app/Contents/MacOS/$own" >/dev/null 2>&1; done
  codesign --force --sign - "$app" >/dev/null 2>&1 || echo "warning: could not sign"
  echo "note: signed ad hoc, so each rebuild asks for Screen Recording and Accessibility again;"
  echo "      create the \"$IDENTITY_NAME\" certificate (see HoloFrame) to stop that"
fi
codesign --verify --deep --strict "$dest" 2>/dev/null && echo "signature verified" || echo "warning: the installed app's signature does not verify"
echo "built $dest"
echo "run:  open -a $dest"
