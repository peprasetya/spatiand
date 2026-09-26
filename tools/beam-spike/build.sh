#!/bin/sh
# Builds the Phase 0 spike APK with the bare SDK tools: no Gradle, no AndroidX.
set -eu
cd "$(dirname "$0")"
SDK=${ANDROID_HOME:-$HOME/Library/Android/sdk}
BT=$SDK/build-tools/36.0.0
JAR=$SDK/platforms/android-34/android.jar
OUT=build
rm -rf $OUT && mkdir -p $OUT/classes $OUT/dex

$BT/aapt2 compile --dir res -o $OUT/res.zip
$BT/aapt2 link -I $JAR --manifest AndroidManifest.xml -o $OUT/base.apk $OUT/res.zip
javac --release 17 -classpath $JAR -d $OUT/classes $(find src -name '*.java')
$BT/d8 --min-api 34 --lib $JAR --output $OUT/dex $(find $OUT/classes -name '*.class')
(cd $OUT/dex && zip -q ../base.apk classes.dex)
$BT/zipalign -f 4 $OUT/base.apk $OUT/aligned.apk
$BT/apksigner sign --ks "$HOME/.android/debug.keystore" --ks-pass pass:android \
    --out spike.apk $OUT/aligned.apk
echo "built $(pwd)/spike.apk"
