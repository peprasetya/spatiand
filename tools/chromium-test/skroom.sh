#!/bin/bash
# The test room with the fake SpatiKey: skroom.sh <fresh|keep> <seconds> <url>
# Never the real vault: SPATIKEY_SOCKET points at the fake service; the shared folder is a copy.
M=$1; S=$2; U=${3:-about:blank}
T=/tmp/crt; C=/tmp/crt-config; P=/tmp/crt-keep
pkill -f "[e]xamples/soft-service" ; pkill -f "[p]ython3 -m http.server 18765"; pkill -f "[r]ange-server" ; sleep 0.5
cat > /tmp/sk-run.sh <<E
#!/bin/bash
mkdir -p /tmp/crt-sk
/home/deck/spatiand/target/release/examples/soft-service /tmp/crt-sk /tmp/crt-sk/sock > /tmp/crt-sk/log 2>&1 &
cd /tmp/crt-skpages && python3 -m http.server 18765 --bind 127.0.0.1 > /tmp/crt-sk/http.log 2>&1 &
export SPATIKEY_SOCKET=/tmp/crt-sk/sock
sleep 1
\$HOME/crsync.sh $M $S $U > /tmp/crsync.out 2>&1
kill %1 %2 2>/dev/null
E
chmod +x /tmp/sk-run.sh
systemctl --user stop crt-room 2>/dev/null; systemctl --user reset-failed crt-room 2>/dev/null
pkill -f "[u]ser-data-dir=/tmp/crt"; sleep 1
systemd-run --user --quiet --unit=crt-room --collect --setenv=STEPS="${STEPS:-wait:$S;dump:/tmp/crt/page.png}" --setenv=HOME=$HOME bash /tmp/sk-run.sh
