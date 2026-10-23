# The web engine SpatiMesh draws with on every platform (docs/spatimesh.md), and what it needs
# that a platform has not got. The one place a version is changed: build.sh reads it.
WPE_VERSION=2.54.1
LIBSOUP_VERSION=3.8.0
GLIB_NETWORKING_VERSION=2.90.0
XKBCOMMON_VERSION=1.7.0   # the compositor's own (mac/xkbcommon/build.sh)
HARFBUZZ_VERSION=14.3.0   # with ICU, which WebKit asks of it
LIBEPOXY_VERSION=1.5.10
