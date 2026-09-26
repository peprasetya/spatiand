#!/bin/sh
# Fetches the Java libraries the app links, straight from Maven Central: no Gradle, no Maven.
#
# Each is pinned by version and by SHA-256, so a changed file on the server fails the fetch
# rather than ending up in the APK. What an AAR carries beyond its classes.jar has been looked
# at and is handled by hand in AndroidManifest.xml:
#
#   Shizuku 13.1.5 (api, provider, aidl, shared) -- no resources (every R.txt is empty). The
#   provider AAR's manifest adds the permission moe.shizuku.manager.permission.API_V23 and the
#   meta-data moe.shizuku.client.V3_SUPPORT=true; nothing else does anything.
#   androidx.annotation, which they name as a dependency, is annotations only and not needed.
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

fetch dev/rikka/shizuku api 13.1.5 4def9bde498ef8626614c2fc5db9af4749c86f16f6c33e3f5658d35e70bab59b
fetch dev/rikka/shizuku provider 13.1.5 b0f18cd9812464ec171c53cac93a819fe411718a3965c311f01eb4de265381b3
fetch dev/rikka/shizuku aidl 13.1.5 33fe7191cdd69fcb66d649264f3b0c47acb2f3d6343afc05b98dbbff6f221963
fetch dev/rikka/shizuku shared 13.1.5 4659642c9339be0a26e9c65bb8648f7ad6d8f4a465f557993ccbc78802381635
