#!/usr/bin/env python3
# The conflict hunks left in the worktree, with a little context, numbered lines.
import subprocess,sys
import os
W=os.environ["CARRY_WORKTREE"]
fs=[l for l in subprocess.run(["git","-C",W,"diff","--name-only","--diff-filter=U"],capture_output=True,text=True).stdout.split("\n") if l]
for f in fs:
    try: L=open(W+"/"+f).read().split("\n")
    except Exception as e: print("==",f,"(no file:",e,")"); continue
    print("==",f); i=0
    while i<len(L):
        if L[i].startswith("<<<<<<< "):
            j=i
            while not L[j].startswith(">>>>>>> "): j+=1
            for k in range(max(0,i-4),min(len(L),j+5)): print(f"{k+1:5d} {L[k][:170]}")
            print("     ..."); i=j
        i+=1
