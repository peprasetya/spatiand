# The FFmpeg Spatiand ships on every platform, and what it is built with. The one place a
# version is changed: third_party/ffmpeg/build.sh reads it, for the Mac, the Beam Pro and the Deck.
FFMPEG_VERSION=9.1
# TLS for FFmpeg's own https (streaming playlists): BoringSSL, the one SpatiMesh's Chromium carries,
# read from that Chromium's own source so that Spatiand has one TLS library's source, at whatever
# revision that Chromium pins (the month's: the build's stamp names it). Its checkout in the tree
# SpatiMesh is built from on this machine -- deepmagpie's for Linux and the Beam Pro, littleotter's
# for the Mac -- or SPATIAND_BORINGSSL, another checkout of it. The Deck has no tree: it has a copy of
# deepmagpie's checkout, ~/spatiand-build/boringssl, made again at each month's build:
#   git clone --depth 1 file://<deepmagpie's tree>/third_party/boringssl/src <copy>
#   tar -C <copy> -cf - .git | ssh deck@steamdeck 'rm -rf ~/spatiand-build/boringssl && mkdir -p ~/spatiand-build/boringssl && tar -x -C ~/spatiand-build/boringssl && git -C ~/spatiand-build/boringssl checkout -f HEAD'
# (the same commit, so the same stamp). Read only: the tree is never written to. Apache 2.0, as
# OpenSSL 3 was: FFmpeg is LGPL version 3 (--enable-version3) with it.
case "$(uname -s)" in
Darwin) BORINGSSL_SRC=${SPATIAND_BORINGSSL:-$HOME/Developer/spatimesh-mac/chromium/src/third_party/boringssl/src} ;;
*) BORINGSSL_SRC=${SPATIAND_BORINGSSL:-/media/shared/Linux/App/spatimesh-linux/chromium/src/third_party/boringssl/src}
    [ -n "${SPATIAND_BORINGSSL:-}" ] || [ -d "$BORINGSSL_SRC" ] || BORINGSSL_SRC=$HOME/spatiand-build/boringssl ;;
esac
