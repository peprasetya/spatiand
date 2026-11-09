#!/usr/bin/perl -p
# GNU sed's `0,/^---/s//X\n---/`, which Cromite's apply-cromite-patches.sh runs on each of its
# patches, for a Mac: macOS's sed has no `0,` address. The first line that starts with --- gets
# the line X before it: perl first-dashes.pl X < patch
BEGIN { $x = shift @ARGV; }
if (!$done && s/^---/$x\n---/) { $done = 1; }
