#!/usr/bin/env python3
"""A stand-in for a Jellyfin or Emby server, for looking at SpatiReel without one.

    tools/fake-jellyfin.py <folder of films> [port] [--emby]

Serves the handful of calls SpatiReel makes -- sign in (any name, password "pw"), the
library's views, a folder's items, what is part watched, a poster, the film itself with byte
ranges, and the three reports of how far a film has got -- and writes each report to standard
output, which is how "the place was kept on the server" is checked. Films are the folder's
files; every third is filed under a series, so there are folders to go into. A poster is the
file beside the film with the same name and .jpg, if there is one.

It is a fake: it answers what this client asks in the shape the real servers answer it, as
far as their documentation says. It proves the client's plumbing and nothing about a real
server's quirks.
"""
import json, os, re, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

root = os.path.abspath(sys.argv[1])
port = int(sys.argv[2]) if len(sys.argv) > 2 and sys.argv[2].isdigit() else 8096
emby = "--emby" in sys.argv
prefix = "/emby" if emby else ""
films = sorted(f for f in os.listdir(root) if f.lower().endswith((".mp4", ".mkv", ".mov")))
TOKEN, USER = "fake-token", "user-1"
places = {}  # item id -> ticks
TICKS = 10_000_000
RUNTIME = 120 * TICKS


def film(index):
    name = films[index]
    stem = os.path.splitext(name)[0]
    item = {
        "Id": f"film{index}", "Name": stem.replace(".", " ").replace("_", " "), "Type": "Movie", "IsFolder": False,
        "ProductionYear": 2001 + index, "RunTimeTicks": RUNTIME, "Path": f"/media/films/{name}",
        "UserData": {"PlaybackPositionTicks": places.get(f"film{index}", 0), "Played": False},
        "PrimaryImageAspectRatio": 0.6667,
    }
    if os.path.exists(os.path.join(root, stem + ".jpg")):
        item["ImageTags"] = {"Primary": "tag"}
    if "HSBS" in name: item["Video3DFormat"] = "HalfSideBySide"
    if index % 3 == 2:
        item.update({"Type": "Episode", "SeriesName": "A Show", "IndexNumber": index // 3 + 1, "ParentIndexNumber": 1, "PrimaryImageAspectRatio": 1.7778})
    return item


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def answer(self, body, status=200, kind="application/json"):
        data = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def signed_in(self, query):
        said = (self.headers.get("X-Emby-Authorization") or "") + (self.headers.get("Authorization") or "")
        return TOKEN in said or self.headers.get("X-Emby-Token") == TOKEN or query.get("api_key") == [TOKEN]

    def route(self):
        url = urllib.parse.urlparse(self.path)
        path = url.path
        if emby and not path.startswith(prefix):
            return None, None, None
        return path[len(prefix):], urllib.parse.parse_qs(url.query), url

    def do_POST(self):
        path, query, _ = self.route()
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0)) or 0) or b"{}")
        if path == "/Users/AuthenticateByName":
            if body.get("Pw") != "pw":
                return self.answer({"error": "no"}, 401)
            if "MediaBrowser" not in ((self.headers.get("X-Emby-Authorization") or "") + (self.headers.get("Authorization") or "")):
                return self.answer({"error": "no client named"}, 400)
            return self.answer({"AccessToken": TOKEN, "ServerId": "s", "User": {"Id": USER, "Name": body.get("Username", "")}})
        if path is None or not self.signed_in(query):
            return self.answer({}, 401)
        if path.startswith("/Sessions/Playing"):
            what = path.rsplit("/", 1)[-1]
            ticks = body.get("PositionTicks", 0)
            if what == "Stopped" and ticks > RUNTIME * 0.9:
                places.pop(body["ItemId"], None)
            else:
                places[body["ItemId"]] = ticks
            print(f"report {what:8} {body.get('ItemId')} at {min(ticks, 10**12) / TICKS:.1f}s paused={body.get('IsPaused')}", flush=True)
            return self.answer(b"", 204)
        self.answer({}, 404)

    def do_GET(self):
        path, query, _ = self.route()
        if path == "/System/Info/Public":
            return self.answer({"ServerName": "fake", "ProductName": "Emby Server" if emby else "Jellyfin Server"})
        if path is None or not self.signed_in(query):
            return self.answer({}, 401)
        if path == f"/Users/{USER}/Views":
            return self.answer({"Items": [
                {"Id": "films", "Name": "Films", "Type": "CollectionFolder", "IsFolder": True, "CollectionType": "movies"},
                {"Id": "shows", "Name": "Shows", "Type": "CollectionFolder", "IsFolder": True, "CollectionType": "tvshows"}]})
        if path == f"/Users/{USER}/Items/Resume":
            return self.answer({"Items": [film(i) for i in range(len(films)) if places.get(f"film{i}", 0) > 0]})
        if path == f"/Users/{USER}/Items":
            parent = query.get("ParentId", [""])[0]
            if parent == "films":
                items = [film(i) for i in range(len(films)) if i % 3 != 2]
            elif parent == "shows":
                items = [{"Id": "series", "Name": "A Show", "Type": "Series", "IsFolder": True, "ProductionYear": 2019}]
            elif parent == "series":
                items = [{"Id": "season", "Name": "Season 1", "Type": "Season", "IsFolder": True}]
            elif parent == "season":
                items = [film(i) for i in range(len(films)) if i % 3 == 2]
            else:
                items = []
            return self.answer({"Items": items})
        m = re.match(r"/Items/film(\d+)/Images/Primary$", path)
        if m:
            poster = os.path.join(root, os.path.splitext(films[int(m.group(1))])[0] + ".jpg")
            if os.path.exists(poster):
                return self.answer(open(poster, "rb").read(), kind="image/jpeg")
            return self.answer({}, 404)
        m = re.match(r"/Videos/film(\d+)/stream", path)
        if m:
            file = os.path.join(root, films[int(m.group(1))])
            size = os.path.getsize(file)
            start, end = 0, size - 1
            ranged = re.match(r"bytes=(\d+)-(\d*)", self.headers.get("Range") or "")
            if ranged:
                start = int(ranged.group(1))
                end = int(ranged.group(2)) if ranged.group(2) else size - 1
            self.send_response(206 if ranged else 200)
            self.send_header("Content-Type", "video/mp4")
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("Content-Length", str(end - start + 1))
            if ranged:
                self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
            self.end_headers()
            with open(file, "rb") as f:
                f.seek(start)
                left = end - start + 1
                try:
                    while left > 0:
                        chunk = f.read(min(65536, left))
                        if not chunk: break
                        self.wfile.write(chunk)
                        left -= len(chunk)
                except (BrokenPipeError, ConnectionResetError):
                    pass
            return
        self.answer({}, 404)


print(f"fake {'Emby' if emby else 'Jellyfin'} on :{port}, {len(films)} films from {root}; sign in with any name and the password pw", flush=True)
ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
