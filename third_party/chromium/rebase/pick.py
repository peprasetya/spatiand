#!/usr/bin/env python3
# pick.py <file> <choices> [note]: one letter per conflict hunk, in order: o(urs = upstream 157), t(heirs = the
# patch), b(oth, upstream's first), r(both, the patch's first). A single letter serves every hunk. Adds the file.
import sys,subprocess
import os
W=os.environ["CARRY_WORKTREE"]; f=sys.argv[1]; ch=sys.argv[2]
L=open(W+"/"+f).read().split("\n"); out=[]; i=0; n=0
while i<len(L):
    if L[i].startswith("<<<<<<< "):
        O=[];B=[];T=[];cur=O;i+=1
        while not L[i].startswith(">>>>>>> "):
            if L[i].startswith("||||||| "): cur=B
            elif L[i]=="=======": cur=T
            else: cur.append(L[i])
            i+=1
        c=ch[n] if len(ch)>1 else ch; n+=1
        out+={"o":O,"t":T,"b":O+T,"r":T+O}[c]
    else: out.append(L[i])
    i+=1
open(W+"/"+f,"w").write("\n".join(out)); subprocess.run(["git","-C",W,"add","--",f])
open(os.environ["CARRY_STATE"]+"/resolve.log","a").write(f"MANUAL {f}: {ch} {' '.join(sys.argv[3:])}\n")
