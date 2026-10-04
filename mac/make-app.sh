#!/bin/sh
# Build Spatiand.app: a menu-bar app with no Dock icon, ad-hoc signed so it runs on this Mac.
# The result is ./Spatiand.app. Not notarised, so it is for this Mac, not for handing out.
set -e
cd "$(dirname "$0")"
MACOSX_DEPLOYMENT_TARGET=14.0 cargo build -p spatiand-mac-core --release --manifest-path ../Cargo.toml
SPATIAND_CORE=release swift build -c release
app=Spatiand.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp .build/release/Spatiand "$app/Contents/MacOS/Spatiand"
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
</dict>
</plist>
PLIST

# Signing identity. Ad-hoc signing gives the bundle a new code hash on every build, and macOS binds
# Screen Recording and Accessibility to that hash, so each rebuild would silently take both away.
# HoloFrame's self-signed "HoloFrame Dev" certificate is used when it is in the keychain: the
# permission is then bound to the bundle's name and that certificate, which do not change. See
# HoloFrame's Tools/make-app.sh for how such a certificate is made, and why it is searched for
# without `-v` (a self-signed root is never "valid" to the trust policy, and signs fine).
IDENTITY_NAME="${SPATIAND_SIGN_NAME:-HoloFrame Dev}"
IDENTITY=$(security find-identity -p codesigning 2>/dev/null | grep -F "\"$IDENTITY_NAME" \
  | sed -E 's/^[[:space:]]*[0-9]+\)[[:space:]]*([0-9A-F]+)[[:space:]]+.*$/\1/' | head -1)
if [ -n "$IDENTITY" ]; then
  codesign --force --sign "$IDENTITY" --identifier com.peprasetya.spatiand "$app" >/dev/null 2>&1 \
    && echo "signed with \"$IDENTITY_NAME\" ($IDENTITY): permissions survive rebuilds" \
    || { echo "warning: could not sign with \"$IDENTITY_NAME\"; signing ad hoc"; codesign --force --sign - "$app" >/dev/null 2>&1 || true; }
else
  codesign --force --sign - "$app" >/dev/null 2>&1 || echo "warning: could not sign"
  echo "note: signed ad hoc, so each rebuild asks for Screen Recording and Accessibility again;"
  echo "      create the \"$IDENTITY_NAME\" certificate (see HoloFrame) to stop that"
fi
echo "built $(pwd)/$app"
