# The FFmpeg Spatiand ships on every platform, and what it is built with. The one place a
# version is changed: third_party/ffmpeg/build.sh reads it, for the Mac, the Beam Pro and the Deck.
FFMPEG_VERSION=9.0.2
# TLS for FFmpeg's own https (streaming playlists): OpenSSL, the long-term release. Its Apache
# licence asks FFmpeg to be LGPL version 3 (--enable-version3), which it then is.
OPENSSL_VERSION=3.5.9
