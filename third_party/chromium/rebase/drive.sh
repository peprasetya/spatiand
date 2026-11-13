#!/bin/bash
# carry.sh until a conflict the rules cannot resolve.
T=$(dirname $0); S=${CARRY_STATE:?the folder that keeps this carry's state and logs}; W=${CARRY_WORKTREE:?}; C=${CONTINUE:-0}
while true; do
    out=$(CONTINUE=$C bash $T/carry.sh $S 2>&1); echo "$out" | grep -E "CONFLICT|PORT DONE|still unresolved" | tee -a $S/resolve.log
    echo "$out" | grep -q "PORT DONE" && exit 0
    echo "$out" | grep -q "CONFLICT" || { echo "$out" | tail -5; exit 2; }
    if (cd $W && python3 $T/autoresolve.py >> $S/resolve.log 2>&1); then C=1; else (cd $W && tail -8 $S/resolve.log | grep LEFT); exit 1; fi
done
