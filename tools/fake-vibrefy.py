#!/usr/bin/env python3
"""A pretend Vibrefy, for exercising the client without a real deployment.

Implements just enough of `CLIENT-API.md` to drive the library screens: the JSON face on
`/files`, `/home`, `/pair` and `/progress`, plus the session-in-the-path `/stream` and
`/medialist`. It is a test fixture rather than a server - there is no security here at
all - but it is also the shortest honest statement of what the wire format actually is.

    tools/fake-server.py [port] [media-directory]
"""
import json
import os
import re
import secrets
import sys
import socketserver
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import unquote, urlparse

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8099
ROOT = os.path.abspath(sys.argv[2]) if len(sys.argv) > 2 else os.path.expanduser("~/Videos")

SESSION_ID = "FAKESESSION0001"
COOKIE = "JSESSIONID=fake-session-cookie"
# Approved the moment it is asked for, so pairing can be walked through without a browser.
PAIR_CODE = "48219037"
# Who the pairing signs in as: two runs with two names are two accounts on one server.
ACCOUNT = os.environ.get("FAKE_VIBREFY_ACCOUNT", "someone@example.com")
# With FAKE_VIBREFY_SESSIONS=1 sessions are real enough to end: each client gets its own
# cookie, only a pairing signs it in, and the JSON face answers anything else with the 401 a
# lapsed session gets. Sessions live in this process, so restarting it signs everybody out.
SESSIONS = os.environ.get("FAKE_VIBREFY_SESSIONS") == "1"
signed_in = set()
progress = {}


def is_media(name):
    return name.lower().endswith((".mp4", ".m4v"))


def listing(relative):
    """`BrowseBean.getJson()`: one item list, sorted by type then name."""
    directory = os.path.join(ROOT, relative) if relative else ROOT
    items = []
    if os.path.isdir(directory):
        for name in sorted(os.listdir(directory)):
            full = os.path.join(directory, name)
            path = f"{relative}/{name}" if relative else name
            if os.path.isdir(full):
                # At the root the entries are *libraries*, not directories - `FileItem`
                # numbers them 1 and 2 respectively and each has its own glyph on a tile.
                # A fixture that called everything a directory would let a client that
                # cannot draw a library through.
                items.append({"name": name, "path": path,
                              "type": 1 if not relative else 2, "time": 0,
                              "modified": int(os.path.getmtime(full) * 1000)})
            elif is_media(name):
                items.append({"name": name, "path": path, "type": 3,
                              "time": int(progress.get(f"files/{path}", 0)),
                              "modified": int(os.path.getmtime(full) * 1000)})
    items.sort(key=lambda i: (i["type"], i["name"].lower()))
    # A cloud mount, which only ever appears at the root and sorts after the local
    # libraries. `~cloud/` is `CloudConfig.CLOUDPREFIX`.
    if not relative:
        items.append({"name": "Google Drive", "path": "~cloud/gdrive/", "type": 5,
                      "time": 0, "modified": 0})
    crumbs = [c for c in relative.split("/") if c]
    return {"session": SESSION_ID, "library": crumbs[0] if crumbs else None,
            "path": relative, "crumbs": crumbs, "admin": False, "cloud": False,
            "items": items}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        sys.stderr.write("  %s\n" % (args[0] % args[1:]))

    def wants_json(self):
        return "application/json" in (self.headers.get("Accept") or "")

    def send_json(self, payload, status=200):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Set-Cookie", self.cookie() + "; Path=/")
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def session(self):
        for part in (self.headers.get("Cookie") or "").split(";"):
            name, _, value = part.strip().partition("=")
            if name == "JSESSIONID":
                return value
        return None

    def cookie(self):
        if not SESSIONS:
            return COOKIE
        # The session the client has, or a new one for a client with none.
        if not getattr(self, "fresh", None):
            self.fresh = self.session() or secrets.token_hex(8)
        return "JSESSIONID=" + self.fresh

    def body(self):
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length).decode() if length else ""
        fields = {}
        for pair in raw.split("&"):
            if "=" in pair:
                key, value = pair.split("=", 1)
                fields[unquote(key.replace("+", " "))] = unquote(value.replace("+", " "))
        return fields

    def do_POST(self):
        path = unquote(urlparse(self.path).path)
        form = self.body()
        self.fresh = None
        if SESSIONS and path != "/pair" and self.session() not in signed_in:
            return self.send_json({"error": "not signed in"}, 401)

        if path.startswith("/files"):
            return self.send_json(listing(path[len("/files"):].strip("/")))
        if path == "/home":
            watching = []
            for key, seconds in progress.items():
                relative = key[len("files/"):]
                if seconds > 0:
                    watching.append({"name": os.path.basename(relative), "path": relative,
                                     "type": 3, "time": int(seconds), "modified": 0})
            return self.send_json({"watching": watching})
        if path == "/progress":
            key, seconds = form.get("media", ""), float(form.get("time", 0))
            if seconds < -10:
                progress.pop(key, None)
            else:
                progress[key] = seconds
            return self.send_json({"ok": True})
        if path == "/pair":
            order = form.get("order")
            if order == "Request":
                return self.send_json({"state": "waiting", "code": PAIR_CODE,
                                       "seconds": 180, "account": None})
            if order == "Poll":
                # Approved straight away: there is nobody here to press Approve.
                signed_in.add(self.cookie().partition("=")[2])
                return self.send_json({"state": "done", "account": ACCOUNT})
            if order == "Claim":
                if form.get("code") == PAIR_CODE:
                    return self.send_json({"state": "done", "account": ACCOUNT})
                return self.send_json({"state": "invalid",
                                       "message": "That code is not valid, or it has expired.",
                                       "account": None})
        return self.send_json({"error": "no such command"}, 404)

    def do_GET(self):
        path = unquote(urlparse(self.path).path)

        # The session-in-the-path endpoints. A wrong session id is answered with 200 and an
        # empty body, exactly as the real server does - that behaviour is worth reproducing
        # because it is the one the client has to detect.
        for command in ("stream", "subtitles", "subtitle", "medialist"):
            prefix = f"/{command}/"
            if path.startswith(prefix):
                rest = path[len(prefix):]
                session, _, relative = rest.partition("/")
                if session != SESSION_ID or not relative:
                    self.send_response(200)
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                    return
                if command == "stream":
                    return self.send_file(os.path.join(ROOT, relative))
                if command == "medialist":
                    directory = os.path.dirname(os.path.join(ROOT, relative))
                    names = sorted(n for n in os.listdir(directory) if is_media(n))
                    return self.send_json(names)
                return self.send_json([])

        if path.startswith("/thumbmp4/") or path.startswith("/covermp4/"):
            # `path` was unquoted on the way in; doing it twice would eat a literal %.
            relative = path.split("/", 2)[2] if path.count("/") > 1 else ""
            full = os.path.join(ROOT, relative)
            # A *directory* thumbnail is the trap, and it is reproduced deliberately.
            # `BrowseBean` only enters its thumbnail branch for a file; asked about a
            # folder it falls through to the ordinary browse page and answers 200 with
            # `text/html` - byte for byte what it answers on a dead session. A client that
            # reads that as "signed out" signs itself out the moment it lists a library,
            # which is the shape the real bug took: signed in, then straight back to the
            # pairing screen.
            if os.path.isdir(full):
                body = b"<html><body>the browse page, which is not a picture</body></html>"
                self.send_response(200)
                self.send_header("Content-Type", "text/html;charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            # No artwork for a file: a tile with a name on it is a perfectly good tile.
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return

        if self.wants_json():
            return self.send_json({"error": "no such command"}, 404)
        self.send_response(404)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def send_file(self, full):
        if not os.path.isfile(full):
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        size = os.path.getsize(full)
        start, end = 0, size - 1
        header = self.headers.get("Range")
        match = re.match(r"bytes=(\d+)-(\d*)", header or "")
        partial = bool(match)
        if match:
            start = int(match.group(1))
            end = int(match.group(2)) if match.group(2) else size - 1
        length = max(0, end - start + 1)

        self.send_response(206 if partial else 200)
        self.send_header("Content-Type", "video/mp4")
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(length))
        if partial:
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.end_headers()
        try:
            with open(full, "rb") as f:
                f.seek(start)
                remaining = length
                while remaining > 0:
                    chunk = f.read(min(65536, remaining))
                    if not chunk:
                        break
                    self.wfile.write(chunk)
                    remaining -= len(chunk)
        except (BrokenPipeError, ConnectionResetError):
            # A player that has read enough of a range closes the connection. Normal, and
            # not worth a traceback in the log.
            pass


class Server(ThreadingHTTPServer):
    """`ThreadingHTTPServer` without the reverse DNS lookup of its own address.

    `HTTPServer.server_bind` asks for its own fully qualified name between binding and
    listening, which on a network with slow DNS takes as long as the resolver does - five
    seconds on a Steam Deck on one particular day. For all of that the port is bound, not
    listening, and every connection is refused, while the lines above say the server is up.
    A client that tries once at startup concludes there is no server. The name is only ever
    used in `SERVER_NAME` for CGI, which nothing here has.
    """

    def server_bind(self):
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]


if __name__ == "__main__":
    server = Server(("0.0.0.0", PORT), Handler)
    print(f"pretend Vibrefy on http://0.0.0.0:{PORT} serving {ROOT}", flush=True)
    print(f"the pairing code is {PAIR_CODE}", flush=True)
    server.serve_forever()
