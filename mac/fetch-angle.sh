#!/bin/sh
# ANGLE -- OpenGL ES on Metal -- for the Mac's compositor, which is the Deck's and draws with GLES.
#
# macOS has no OpenGL ES of its own. ANGLE is Google's (BSD licence), and every Electron
# application carries a build of it as two libraries; this copies them from one that is
# installed, preferring a universal build so the same app runs on Intel and Apple Silicon.
# They are not kept in the repository: run this once before `make-app.sh`.
set -e
out="$(cd "$(dirname "$0")" && pwd)/vendor/angle"
best=""
for app in "/Applications/Visual Studio Code.app" /Applications/*.app "$HOME"/Applications/*.app; do
    lib="$app/Contents/Frameworks/Electron Framework.framework/Versions/A/Libraries"
    [ -f "$lib/libEGL.dylib" ] && [ -f "$lib/libGLESv2.dylib" ] || continue
    archs="$(lipo -archs "$lib/libGLESv2.dylib")"
    case "$archs" in
        *x86_64*arm64*|*arm64*x86_64*) best="$lib"; break ;;
        *"$(uname -m)"*) [ -n "$best" ] || best="$lib" ;;
    esac
done
[ -n "$best" ] || { echo "no Electron application found to take ANGLE from" >&2; exit 1; }
mkdir -p "$out"
cp "$best/libEGL.dylib" "$best/libGLESv2.dylib" "$out/"
# Found beside each other wherever the app puts them.
install_name_tool -id @rpath/libEGL.dylib "$out/libEGL.dylib" 2>/dev/null || true
install_name_tool -id @rpath/libGLESv2.dylib "$out/libGLESv2.dylib" 2>/dev/null || true
codesign --force -s - "$out/libEGL.dylib" "$out/libGLESv2.dylib" 2>/dev/null || true
echo "ANGLE ($(lipo -archs "$out/libGLESv2.dylib")) from $best"
