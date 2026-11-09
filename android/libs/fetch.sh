#!/bin/sh
# Fetches the Java libraries the app links, straight from Maven Central: no Gradle, no Maven.
#
# Each is pinned by version and by SHA-256, so a changed file on the server fails the fetch
# rather than ending up in the APK. What an AAR carries beyond its classes.jar is to be looked
# at and handled by hand in AndroidManifest.xml.
#
# None is fetched now. Shizuku's client was, for starting the shell helper through the Shizuku
# app; Spatiand starts it itself through Wireless debugging instead (AdbStarter.java,
# crates/spatiand-adb), as the owner asked on 2026-11-09, with no other app to install.
set -eu
cd "$(dirname "$0")"
CENTRAL=https://repo1.maven.org/maven2

fetch() { # group-path artifact version sha256
    out="$2-$3.jar"
    [ -f "$out" ] && return 0
    curl -sSfL -o "$2.aar" "$CENTRAL/$1/$2/$3/$2-$3.aar"
    echo "$4  $2.aar" | shasum -a 256 -c - >/dev/null || {
        echo "checksum mismatch for $2-$3.aar" >&2
        rm -f "$2.aar"
        exit 1
    }
    unzip -p "$2.aar" classes.jar > "$out"
    rm -f "$2.aar"
    echo "fetched $out"
}
