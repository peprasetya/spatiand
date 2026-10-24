# The web engine SpatiMesh draws with on every platform (docs/spatimesh.md), and what it needs
# that a platform has not got. The one place a version is changed: build.sh reads it.
WPE_VERSION=2.54.1
LIBSOUP_VERSION=3.8.0
GLIB_NETWORKING_VERSION=2.90.0
XKBCOMMON_VERSION=1.7.0   # the compositor's own (mac/xkbcommon/build.sh)
HARFBUZZ_VERSION=14.3.0   # with ICU, which WebKit asks of it
LIBEPOXY_VERSION=1.5.10
GSTREAMER_VERSION=1.28.8   # WPE's media player; decoding by gst-libav on Spatiand's own FFmpeg
# What WPE needs that macOS has not got, built here so the app carries its own (pinned, for
# macOS 14): before these were Homebrew's, and the app ran only on the Mac it was built on.
GLIB_VERSION=2.88.4
PCRE2_VERSION=10.49
GETTEXT_VERSION=0.26
FREETYPE_VERSION=2.15.0
FONTCONFIG_VERSION=2.17.1
LIBPNG_VERSION=1.6.58
LIBJPEG_TURBO_VERSION=3.2.0
LIBWEBP_VERSION=1.6.0
ZSTD_VERSION=1.5.7
BROTLI_VERSION=1.2.1   # web fonts (WOFF2) and pages sent compressed with it
WOFF2_VERSION=1.0.2
NGHTTP2_VERSION=1.70.0
LIBPSL_VERSION=0.23.3
LIBTASN1_VERSION=4.21.0
LIBGPG_ERROR_VERSION=1.61
LIBGCRYPT_VERSION=1.12.4
