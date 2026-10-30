#!/usr/bin/env python3
"""Asks SpatiKey's socket directly: skask.py <socket> <op> [json fields]"""
import json, socket, struct, sys
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])
req = {"id": 1, "op": sys.argv[2]}
if len(sys.argv) > 3:
    req.update(json.loads(sys.argv[3]))
b = json.dumps(req).encode(); s.sendall(struct.pack(">I", len(b)) + b)
n = struct.unpack(">I", s.recv(4, socket.MSG_WAITALL))[0]
print(s.recv(n, socket.MSG_WAITALL).decode())
