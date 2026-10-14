#!/bin/sh
# spatiand-storage, for one platform: rclone and every backend it has, linked in, as one
# program with no C in it -- so every platform is built from wherever Go is.
#
#   storage/build.sh [mac|linux|android] <out>
#
# mac is this Mac's own architecture; linux is the Deck's (x86_64); android the Beam Pro's
# (arm64), named as the APK's library folder wants a program to be named by the caller.
set -eu
cd "$(dirname "$0")"
target=${1:-mac}
out=${2:-spatiand-storage}
case "$target" in
    mac) env CGO_ENABLED=0 go build -trimpath -ldflags "-s -w" -o "$out" . ;;
    linux) env CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -trimpath -ldflags "-s -w" -o "$out" . ;;
    android) env CGO_ENABLED=0 GOOS=android GOARCH=arm64 go build -trimpath -ldflags "-s -w" -o "$out" . ;;
    *) echo "which platform? mac, linux or android" >&2; exit 1 ;;
esac
echo "spatiand-storage for $target: $out"
