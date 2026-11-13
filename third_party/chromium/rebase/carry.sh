#!/bin/bash
# Carry the stack (stack.txt: "<commit> <subject>" a line, oldest first) onto the new version, one commit at a time, three-way. Stops at a
# conflict; resolve, git add, and run again with CONTINUE=1.
S=$1; W=${CARRY_WORKTREE:?the worktree at the new version}; cd $W
git config user.email spatiand@localhost; git config user.name "Spatiand build"
SUBS="v8 third_party/skia third_party/devtools-frontend/src third_party/boringssl/src third_party/perfetto"
start=$(cat $S/port157.at 2>/dev/null || echo 0); n=0
while read h subj; do
    n=$((n+1)); [ $n -le $start ] && continue
    if [ "$CONTINUE" = 1 ]; then
        CONTINUE=0
        if git diff --name-only --diff-filter=U | grep -q .; then echo "still unresolved at $n"; exit 1; fi
    else
        ex=""; for d in $SUBS; do ex="$ex :!$d"; done
        for d in $SUBS; do
            if git -C $W diff --quiet $h^ $h -- $d 2>/dev/null; then :; else
                mkdir -p $S/sub/$(echo $d | tr / _); git diff --binary --relative=$d/ $h^ $h -- $d > $S/sub/$(echo $d | tr / _)/$(printf %04d $n).patch
                git log -1 --format='%s' $h > $S/sub/$(echo $d | tr / _)/$(printf %04d $n).subject
            fi
        done
        # Each file by itself (applyeach.py): `git apply` takes a whole diff or none of it, and at the
        # first rebase one file gone upstream silently cost ten patches every other file they had.
        git diff --binary $h^ $h -- . $ex > $S/one.diff
        if ! python3 $(dirname $0)/applyeach.py $S/one.diff > $S/port.err 2>&1; then
            echo "CONFLICT at $n: $subj ($h)"; grep -E "CONFLICT|FAILED" $S/port.err | head -40; exit 1
        fi
    fi
    git commit -q -C $h --allow-empty
    echo $n > $S/port157.at
done < $S/stack.txt
echo "PORT DONE: $n"
