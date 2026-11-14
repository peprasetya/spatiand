#!/bin/sh
# libxkbcommon's grammar (src/xkbcomp/parser.y), compiled by bison into parser.c and parser.h
# here, beside this script, which are committed; parser.y.sha256 says which grammar they are.
#
#   third_party/xkbcommon/generate-parser.sh     (with a release changed in source.sh)
#
# Why they are kept rather than made at each build: the grammar asks for bison 3.6 or newer
# (`%define parse.error detailed`), and a Mac has only Apple's bison 2.3 (littleotter has no
# Homebrew either). Release tarballs used to carry the compiled grammar; GitHub's tag archives,
# the only download since 1.7, do not. So it is compiled once, on Linux, exactly as meson.build's
# yacc_gen does (`-p _xkbcommon_`, the header beside it), and every platform builds the same
# file. Made for 1.13.2 with GNU Bison 3.8.2 (Ubuntu's, on deepmagpie).
set -eu
cd "$(dirname "$0")"
XKB_DIR=$(pwd)
. ./source.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
xkb_fetch "$XKB_URL" $XKB_SHA "libxkbcommon-$XKB_VERSION.tar.gz"
tar -C "$work" -xf "cache/libxkbcommon-$XKB_VERSION.tar.gz"
src=$work/libxkbcommon-xkbcommon-$XKB_VERSION
# Run where the grammar is, so the #line notes in the result name src/xkbcomp/parser.y and not
# this machine's folders.
(cd "$src" && bison --defines=parser.h -o parser.c -p _xkbcommon_ src/xkbcomp/parser.y)
cp "$src/parser.c" "$src/parser.h" .
(cd "$src" && shasum -a 256 src/xkbcomp/parser.y) | cut -d' ' -f1 > parser.y.sha256
echo "parser.c and parser.h from $XKB_VERSION's parser.y, by $(bison --version | head -1)"
