#!/bin/bash
# decoders.sh [label] [extra SpatiMesh flags]: which decoder SpatiMesh uses for each codec, in the snapshot room.
#
# Each clip of media/ (CLIPS, by default h264.mp4 hevc.mp4 vp9.webm av1.mp4) is played from a server that
# answers Range requests (rangeserve.py), and for each: the media pipeline's own account over DevTools
# (kVideoDecoderName, kIsPlatformVideoDecoder -- what chrome://media-internals shows), the CPU of SpatiMesh's
# processes by kind while it plays (ps; powermetrics wants root), and what the page saw (frames, dropped).
# Then chrome://gpu's text to gpu-<label>.txt (Video Decode in its feature list, and its table of what is
# decoded by hardware). `decoders.sh soft --disable-accelerated-video-decode` is the comparison: the same
# clips in software.
#
# The clips are 20 s of testsrc2 at 3840x2160, 60 a second, made with ffmpeg on deepmagpie, so that software
# decoding shows in the CPU: -c:v libx264 | libx265 -tag:v hvc1 | libvpx-vp9 | libsvtav1.
# R is the rig's folder (go.sh's), RANGE_PORT and PORT the two ports.
R=${R:-/tmp/smtest}; LABEL=${1:-run}; shift
PORT=${PORT:-9339}; RANGE_PORT=${RANGE_PORT:-18909}
pgrep -f "rangeserve.py $RANGE_PORT" >/dev/null || (cd $R && nohup python3 rangeserve.py $RANGE_PORT $R/media >/dev/null 2>&1 &)
SPATIAND_STEPS="wait:150" nohup $R/go.sh $R/decoders.png 170 --remote-debugging-port=$PORT --autoplay-policy=no-user-gesture-required "$@" about:blank > $R/go.out 2>&1 &
sleep 12
cpu() {  # the %CPU of the test SpatiMesh's processes, summed by kind, four times
  for i in 1 2 3 4; do
    ps -axo %cpu=,command= | grep "user-data-dir=$R/profile" | grep -v grep | awk '{k="browser"; for(i=2;i<=NF;i++){ if($i ~ /^--type=/){k=substr($i,8)} if($i ~ /^--utility-sub-type=/){k=substr($i,20)} } s[k]+=$1} END{for(k in s) printf "%s=%.0f ", k, s[k]; print ""}'
    sleep 2
  done
}
for f in ${CLIPS:-h264.mp4 hevc.mp4 vp9.webm av1.mp4}; do
  echo "=== $LABEL $f"
  python3 $R/cdp.py $PORT media "http://127.0.0.1:$RANGE_PORT/play.html?f=$f" 6 > $R/media-$LABEL-$f.txt 2>&1
  grep -E "kVideoDecoderName|kIsPlatformVideoDecoder|kResolution|^err|msg error|msg warning" $R/media-$LABEL-$f.txt | cut -c1-400
  cpu
  python3 $R/cdp.py $PORT eval - 1 '(()=>{const v=document.querySelector("video");const q=v.getVideoPlaybackQuality();return JSON.stringify({t:v.currentTime,w:v.videoWidth,h:v.videoHeight,frames:q.totalVideoFrames,dropped:q.droppedVideoFrames,err:v.error&&v.error.message,vis:document.visibilityState})})()'
done
python3 $R/cdp.py $PORT eval "chrome://gpu" 6 '(()=>{const iv=document.querySelector("info-view"); const root=iv&&iv.shadowRoot?iv.shadowRoot:document; return root.textContent.replace(/[ \t]+/g," ").replace(/\n\s*\n+/g,"\n").slice(0,60000)})()' > $R/gpu-$LABEL.txt 2>&1
echo "=== chrome://gpu ($R/gpu-$LABEL.txt)"
grep -o "Video Decode: [A-Za-z ]*accelerated\|Video Decode: [A-Za-z ]*only" $R/gpu-$LABEL.txt | head -1
grep -o "Decode [a-z0-9 -]* :[0-9x]* to [0-9x]* pixels" $R/gpu-$LABEL.txt
# Only this rig's SpatiMesh, by its profile; the room ends with its steps.
pkill -f "user-data-dir=$R/profile"; sleep 1
pgrep -fl "user-data-dir=$R/profile" | cut -c1-120
