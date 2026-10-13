#!/usr/bin/env python3
"""A stand-in for a DLNA media server, for looking at SpatiReel without one.

    tools/fake-dlna.py <folder of films> [port]

Answers the network's "who is a media server?" (SSDP), describes itself, and answers a
ContentDirectory `Browse` with the folder's folders and films, which it then serves with byte
ranges. A fake: the shapes are the standard's, and a real server's oddities are not here.
"""
import os, re, socket, struct, sys, threading, urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from xml.sax.saxutils import escape

root = os.path.abspath(sys.argv[1])
port = int(sys.argv[2]) if len(sys.argv) > 2 else 8200


def my_address():
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect(("239.255.255.250", 1900))
        return s.getsockname()[0]
    except OSError:
        return "127.0.0.1"
    finally:
        s.close()


host = my_address()
base = f"http://{host}:{port}"


def ssdp():
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    if hasattr(socket, "SO_REUSEPORT"):
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
    s.bind(("", 1900))
    s.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, struct.pack("4s4s", socket.inet_aton("239.255.255.250"), socket.inet_aton("0.0.0.0")))
    while True:
        data, sender = s.recvfrom(2048)
        if b"M-SEARCH" in data and (b"MediaServer" in data or b"ssdp:all" in data):
            answer = f"HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\nEXT:\r\nLOCATION: {base}/desc.xml\r\nST: urn:schemas-upnp-org:device:MediaServer:1\r\nUSN: uuid:fake-dlna::urn:schemas-upnp-org:device:MediaServer:1\r\n\r\n"
            s.sendto(answer.encode(), sender)


DESC = f"""<?xml version="1.0"?><root xmlns="urn:schemas-upnp-org:device-1-0"><device>
<deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType><friendlyName>Attic NAS</friendlyName><UDN>uuid:fake-dlna</UDN>
<serviceList><service><serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType>
<serviceId>urn:upnp-org:serviceId:ContentDirectory</serviceId><controlURL>/ctl/ContentDir</controlURL></service></serviceList></device></root>"""


def didl(relative):
    out = ['<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">']
    folder = os.path.join(root, relative)
    for name in sorted(os.listdir(folder)):
        path = f"{relative}/{name}" if relative else name
        full = os.path.join(folder, name)
        ident = urllib.parse.quote(path)
        if os.path.isdir(full):
            out.append(f'<container id="{ident}" parentID="0" restricted="1"><dc:title>{escape(name)}</dc:title><upnp:class>object.container.storageFolder</upnp:class></container>')
        elif name.lower().endswith((".mp4", ".mkv")):
            out.append(f'<item id="{ident}" parentID="0" restricted="1"><dc:title>{escape(name)}</dc:title><upnp:class>object.item.videoItem</upnp:class>'
                       f'<res protocolInfo="http-get:*:video/mp4:*">{base}/file/{ident}</res></item>')
    out.append("</DIDL-Lite>")
    return "".join(out)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def send(self, data, kind="text/xml", status=200):
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0))).decode()
        ident = re.search(r"<ObjectID>(.*?)</ObjectID>", body)
        relative = urllib.parse.unquote(ident.group(1)) if ident and ident.group(1) != "0" else ""
        print("browse", relative or "/", flush=True)
        result = escape(didl(relative))
        self.send(f'<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><u:BrowseResponse xmlns:u="urn:schemas-upnp-org:service:ContentDirectory:1"><Result>{result}</Result><NumberReturned>0</NumberReturned><TotalMatches>0</TotalMatches><UpdateID>1</UpdateID></u:BrowseResponse></s:Body></s:Envelope>'.encode())

    def do_GET(self):
        if self.path == "/desc.xml":
            return self.send(DESC.encode())
        if self.path.startswith("/file/"):
            file = os.path.join(root, urllib.parse.unquote(self.path[6:]))
            size = os.path.getsize(file)
            start, end = 0, size - 1
            ranged = re.match(r"bytes=(\d+)-(\d*)", self.headers.get("Range") or "")
            if ranged:
                start = int(ranged.group(1)); end = int(ranged.group(2)) if ranged.group(2) else size - 1
            self.send_response(206 if ranged else 200)
            self.send_header("Content-Type", "video/mp4"); self.send_header("Accept-Ranges", "bytes")
            self.send_header("Content-Length", str(end - start + 1))
            if ranged: self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
            self.end_headers()
            with open(file, "rb") as f:
                f.seek(start); left = end - start + 1
                try:
                    while left > 0:
                        chunk = f.read(min(65536, left))
                        if not chunk: break
                        self.wfile.write(chunk); left -= len(chunk)
                except (BrokenPipeError, ConnectionResetError):
                    pass
            return
        self.send(b"", status=404)


threading.Thread(target=ssdp, daemon=True).start()
print(f"fake DLNA 'Attic NAS' at {base}, films from {root}", flush=True)
ThreadingHTTPServer(("0.0.0.0", port), Handler).serve_forever()
