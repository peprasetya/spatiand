#!/usr/bin/env python3
"""Ask a running Chromium over the DevTools protocol, no libraries needed.

    crq.py PORT URL JS [screenshot.png]

Opens URL in a new tab (or "-": the first page), waits for it to load, evaluates JS (promises
awaited), prints the result as JSON, optionally saves a screenshot, closes the tab it opened.
"""
import base64, json, os, socket, struct, sys, time, urllib.request

port, url, expr = int(sys.argv[1]), sys.argv[2], sys.argv[3]
shot = sys.argv[4] if len(sys.argv) > 4 else None

def targets():
    return [p for p in json.load(urllib.request.urlopen(f"http://127.0.0.1:{port}/json")) if p.get("type") == "page"]

for _ in range(60):
    try:
        pages = targets()
        if pages:
            break
    except Exception:
        pass
    time.sleep(1)
else:
    sys.exit("no page")

opened = False
if url == "-":
    target = pages[0]
elif url.startswith("tab:"):
    target = next(p for p in pages if url[4:] in p["url"])
else:
    req = urllib.request.Request(f"http://127.0.0.1:{port}/json/new?{url}", method="PUT")
    target = json.load(urllib.request.urlopen(req)); opened = True

path = target["webSocketDebuggerUrl"].split(f":{port}", 1)[1]
sock = socket.create_connection(("127.0.0.1", port))
key = base64.b64encode(os.urandom(16)).decode()
sock.sendall(f"GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
             f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n".encode())
buf = b""
while b"\r\n\r\n" not in buf:
    buf += sock.recv(4096)
buf = buf.split(b"\r\n\r\n", 1)[1]

def send(msg_id, method, params=None):
    data = json.dumps({"id": msg_id, "method": method, "params": params or {}}).encode()
    header = bytearray([0x81]); n = len(data)
    if n < 126: header.append(0x80 | n)
    elif n < 65536: header.append(0x80 | 126); header += struct.pack(">H", n)
    else: header.append(0x80 | 127); header += struct.pack(">Q", n)
    mask = os.urandom(4); header += mask
    sock.sendall(bytes(header) + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

def recv_exact(n):
    global buf
    while len(buf) < n:
        chunk = sock.recv(1 << 20)
        if not chunk: raise EOFError
        buf += chunk
    out, buf = buf[:n], buf[n:]
    return out

partial = b""
def recv():
    global partial
    while True:
        b1, b2 = recv_exact(2); n = b2 & 0x7F
        if n == 126: n = struct.unpack(">H", recv_exact(2))[0]
        elif n == 127: n = struct.unpack(">Q", recv_exact(8))[0]
        payload = recv_exact(n)
        if (b1 & 0x0F) not in (0, 1): continue
        partial += payload
        if b1 & 0x80:
            out, partial = partial, b""
            return json.loads(out)

def call(msg_id, method, params=None, timeout=30):
    send(msg_id, method, params); sock.settimeout(timeout)
    while True:
        m = recv()
        if m.get("id") == msg_id:
            return m

call(1, "Runtime.enable")
call(2, "Page.enable")
call(6, "Page.bringToFront")
call(7, "Emulation.setFocusEmulationEnabled", {"enabled": True})
end = time.time() + 15
while time.time() < end:
    r = call(3, "Runtime.evaluate", {"expression": "document.readyState", "returnByValue": True})
    if r.get("result", {}).get("result", {}).get("value") == "complete":
        break
    time.sleep(0.3)
time.sleep(1)
if expr.startswith("@"):
    method, _, params = expr[1:].partition(" ")
    print(json.dumps(call(4, method, json.loads(params) if params else {}, 60).get("result"), ensure_ascii=False)[:60000])
    res = {"result": {"value": None}}
else:
    r = call(4, "Runtime.evaluate", {"expression": expr, "returnByValue": True, "awaitPromise": True, "userGesture": True}, 130)
    res = r.get("result", {})
if expr.startswith("@"):
    pass
elif "exceptionDetails" in res:
    print("EXC", json.dumps(res["exceptionDetails"])[:1500])
else:
    print(json.dumps(res.get("result", {}).get("value"), ensure_ascii=False, indent=None)[:60000])
if shot:
    r = call(5, "Page.captureScreenshot", {"format": "png"})
    open(shot, "wb").write(base64.b64decode(r["result"]["data"]))
if opened and not os.environ.get("KEEP"):
    try:
        urllib.request.urlopen(f"http://127.0.0.1:{port}/json/close/{target['id']}")
    except Exception:
        pass
