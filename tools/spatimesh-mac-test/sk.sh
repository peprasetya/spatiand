#!/bin/sh
# sk.sh <png> <wait s> [SpatiMesh flags and URLs]: go.sh with a stand-in for SpatiKey's service
# (crates/spatikey-core/examples/soft-service: a vault in /tmp/smtest/sk, every touch a yes) and
# tools/chromium-test/pages served at http://localhost:18765 (copied to /tmp/smtest/skpages).
# Never the owner's vault: SPATIKEY_SOCKET names the stand-in. The service is built once with
#   git clone ~/Developer/Spatiand.git /tmp/smtest/spatiand-src && cd /tmp/smtest/spatiand-src &&
#   CARGO_TARGET_DIR=/tmp/smtest/target cargo build --release --offline -p spatikey-core --example soft-service
# Its log (/tmp/smtest/sk/log) says each touch that was asked for.
pkill -f "[e]xamples/soft-service /tmp/smtest/sk"; pkill -f "[h]ttp.server 18765"
rm -rf /tmp/smtest/sk; mkdir -p /tmp/smtest/sk
/tmp/smtest/target/release/examples/soft-service /tmp/smtest/sk /tmp/smtest/sk/sock > /tmp/smtest/sk/log 2>&1 &
SK=$!
(cd /tmp/smtest/skpages && exec /usr/bin/python3 -m http.server 18765 --bind 127.0.0.1 > /tmp/smtest/sk/http.log 2>&1) &
WEB=$!
sleep 1
SPATIKEY_SOCKET=/tmp/smtest/sk/sock /tmp/smtest/go.sh "$@"
kill $SK $WEB 2>/dev/null
pkill -f "[h]ttp.server 18765"
