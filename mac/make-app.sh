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
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Spatiand</string>
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
  codesign --force --sign "$IDENTITY" --identifier com.peprasetya.spatiand "$app" >/dev/null 2>&1 \
    && echo "signed with \"$IDENTITY_NAME\" ($IDENTITY): permissions survive rebuilds" \
    || { echo "warning: could not sign with \"$IDENTITY_NAME\"; signing ad hoc"; codesign --force --sign - "$app" >/dev/null 2>&1 || true; }
else
  for lib in "$app"/Contents/Frameworks/*.dylib; do [ -f "$lib" ] && codesign --force --sign - "$lib" >/dev/null 2>&1; done
  codesign --force --sign - "$app" >/dev/null 2>&1 || echo "warning: could not sign"
  echo "note: signed ad hoc, so each rebuild asks for Screen Recording and Accessibility again;"
  echo "      create the \"$IDENTITY_NAME\" certificate (see HoloFrame) to stop that"
fi
dest="$HOME/Applications/Spatiand.app"
mkdir -p "$HOME/Applications"
rm -rf "$dest"
ditto --noextattr --norsrc "$app" "$dest"
rm -rf "$stage"
codesign --verify --deep --strict "$dest" 2>/dev/null && echo "signature verified" || echo "warning: the installed app's signature does not verify"
echo "built $dest"
echo "run:  open -a $dest"
