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
# own search path. bindgen wants to be told where libclang is. Each is built on its own, so that
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
    otool -L "$app/Contents/MacOS/$own" | grep -q '@rpath/libav' \
      && install_name_tool -add_rpath @executable_path/../Frameworks "$app/Contents/MacOS/$own"
  else
    echo "warning: $own did not build; the app is made without it"
  fi
done
# SpatiMesh's web engine: WebKit, in a program of its own beside SpatiMesh, which starts one for
# each profile in use (docs/spatimesh.md).
if [ -f "$app/Contents/MacOS/spatimesh" ]; then
  if (cd .. && MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --release -p spatimesh-webkit); then
    cp ../target/release/spatimesh-webkit "$app/Contents/MacOS/spatimesh-webkit"
    OWN="$OWN spatimesh-webkit"
  else
    echo "warning: spatimesh-webkit did not build; SpatiMesh will show no pages"
  fi
fi
# SpatiMesh's engine on WPE WebKit, where third_party/wpe/build.sh mac has made one: SpatiMesh
# takes it over Apple's WebKit when it is here. For now it finds WPE's libraries where that
# script put them (~/Developer/spatiand-wpe/out/mac), so the app is for this Mac; the libraries
# go into the app when they are all built here and pinned (docs/spatimesh.md).
if [ -f "$HOME/Developer/spatiand-wpe/out/mac/lib/pkgconfig/wpe-webkit-2.0.pc" ] && [ -f "$app/Contents/MacOS/spatimesh" ]; then
  if (cd .. && MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --release -p spatimesh-wpe); then
    cp ../target/release/spatimesh-wpe "$app/Contents/MacOS/spatimesh-wpe"
    cp ../third_party/wpe/mac/fonts.conf "$app/Contents/Resources/fonts.conf"
    OWN="$OWN spatimesh-wpe"
  else
    echo "warning: spatimesh-wpe did not build; SpatiMesh uses Apple's WebKit"
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
# A signature refuses a bundle with extended attributes on it (a copied binary carries some).
xattr -cr "$app" 2>/dev/null || true
IDENTITY_NAME="${SPATIAND_SIGN_NAME:-HoloFrame Dev}"
IDENTITY=$(security find-identity -p codesigning 2>/dev/null | grep -F "\"$IDENTITY_NAME" \
  | sed -E 's/^[[:space:]]*[0-9]+\)[[:space:]]*([0-9A-F]+)[[:space:]]+.*$/\1/' | head -1)
if [ -n "$IDENTITY" ]; then
  # What is inside is signed first, with the same certificate.
  for lib in "$app"/Contents/Frameworks/*.dylib; do [ -f "$lib" ] && codesign --force --sign "$IDENTITY" "$lib" >/dev/null 2>&1; done
  for own in $OWN; do [ -f "$app/Contents/MacOS/$own" ] && codesign --force --sign "$IDENTITY" "$app/Contents/MacOS/$own" >/dev/null 2>&1; done
  codesign --force --sign "$IDENTITY" --identifier com.peprasetya.spatiand "$app" >/dev/null 2>&1 \
    && echo "signed with \"$IDENTITY_NAME\" ($IDENTITY): permissions survive rebuilds" \
    || { echo "warning: could not sign with \"$IDENTITY_NAME\"; signing ad hoc"; codesign --force --sign - "$app" >/dev/null 2>&1 || true; }
else
  for lib in "$app"/Contents/Frameworks/*.dylib; do [ -f "$lib" ] && codesign --force --sign - "$lib" >/dev/null 2>&1; done
  for own in $OWN; do [ -f "$app/Contents/MacOS/$own" ] && codesign --force --sign - "$app/Contents/MacOS/$own" >/dev/null 2>&1; done
  codesign --force --sign - "$app" >/dev/null 2>&1 || echo "warning: could not sign"
  echo "note: signed ad hoc, so each rebuild asks for Screen Recording and Accessibility again;"
  echo "      create the \"$IDENTITY_NAME\" certificate (see HoloFrame) to stop that"
fi
# Another checkout building its own app sets this, so as not to replace this one.
dest="${SPATIAND_APP_DEST:-$HOME/Applications/Spatiand.app}"
mkdir -p "$(dirname "$dest")"
rm -rf "$dest"
ditto --noextattr --norsrc "$app" "$dest"
rm -rf "$stage"
codesign --verify --deep --strict "$dest" 2>/dev/null && echo "signature verified" || echo "warning: the installed app's signature does not verify"
echo "built $dest"
echo "run:  open -a $dest"
