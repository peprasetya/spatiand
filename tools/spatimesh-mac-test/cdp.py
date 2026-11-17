#!/usr/bin/env python3
"""A running Chromium over the DevTools protocol, standard library only.

    cdp.py PORT media URL SECONDS        navigate the first page, print the media player's properties/messages
    cdp.py PORT eval URL SECONDS JS      navigate (URL "-": stay), wait, evaluate JS (promises awaited), print it
    cdp.py PORT shot FILE                a screenshot of the first page
    cdp.py PORT tabs                     list the pages
"""
import base64, json, os, socket, struct, sys, time, urllib.request

port, what = int(sys.argv[1]), sys.argv[2]

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

if what == "tabs":
    for p in pages:
        print(p["id"], p["url"][:100], "|", p["title"][:60])
    sys.exit(0)

target = pages[0]
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

events = []
def call(msg_id, method, params=None, timeout=30):
    send(msg_id, method, params); sock.settimeout(timeout)
    while True:
        m = recv()
        if m.get("id") == msg_id:
            return m
        events.append(m)

def show(m):
    meth = m.get("method", "")
    if meth == "Media.playerPropertiesChanged":
        for p in m["params"]["properties"]:
            print("prop", p["name"], "=", p["value"][:300])
    elif meth == "Media.playerMessagesLogged":
        for p in m["params"]["messages"]:
            print("msg", p["level"], p["message"][:300])
    elif meth == "Media.playerErrorsRaised":
        for p in m["params"]["errors"]:
            print("err", json.dumps(p)[:600])

if what == "shot":
    r = call(1, "Page.captureScreenshot", {"format": "png"})
    open(sys.argv[3], "wb").write(base64.b64decode(r["result"]["data"]))
    sys.exit(0)

url, secs = sys.argv[3], float(sys.argv[4])
call(1, "Media.enable")
call(2, "Page.enable")
if url != "-":
    send(3, "Page.navigate", {"url": url})
end = time.time() + secs
sock.settimeout(1)
while time.time() < end:
    try:
        m = recv()
    except (socket.timeout, TimeoutError):
        continue
    if what == "media":
        show(m)
if what == "eval":
    r = call(9, "Runtime.evaluate", {"expression": sys.argv[5], "awaitPromise": True, "returnByValue": True})
    res = r.get("result", {})
    if "exceptionDetails" in res:
        print("EXC", json.dumps(res["exceptionDetails"])[:800])
    else:
        v = res.get("result", {}).get("value")
        print(v if isinstance(v, str) else json.dumps(v))
