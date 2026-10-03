#!/bin/sh
# Build Spatiand.app: a menu-bar app with no Dock icon, ad-hoc signed so it runs on this Mac.
# The result is ./Spatiand.app. Not notarised, so it is for this Mac, not for handing out.
set -e
cd "$(dirname "$0")"
MACOSX_DEPLOYMENT_TARGET=13.0 cargo build -p spatiand-mac-core --release --manifest-path ../Cargo.toml
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
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <!-- A menu-bar app: no Dock icon, no main window. -->
    <key>LSUIElement</key><true/>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
codesign --force --sign - "$app" >/dev/null 2>&1 || echo "warning: could not sign"
echo "built $(pwd)/$app"
