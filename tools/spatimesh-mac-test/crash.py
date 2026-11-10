import json, sys, glob, os
f = sorted(glob.glob(os.path.expanduser("~/Library/Logs/DiagnosticReports/SpatiMesh*.ips")), key=os.path.getmtime)[-1]
t = open(f).read(); d = json.loads(t.split("\n", 1)[1])
print(f, d.get("exception", {}).get("type"))
th = d["threads"][d.get("faultingThread", 0)]; imgs = d["usedImages"]
for fr in th["frames"][:int(sys.argv[1]) if len(sys.argv) > 1 else 14]:
    print(imgs[fr["imageIndex"]].get("name"), "|", fr.get("symbol", hex(fr["imageOffset"]))[:150], fr.get("symbolLocation", ""))
