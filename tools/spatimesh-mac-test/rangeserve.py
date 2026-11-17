#!/usr/bin/env python3
"""A static file server that answers Range requests (python's http.server does not).
    rangeserve.py PORT FOLDER"""
import http.server, os, re, sys, socketserver

class H(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def send_head(self):
        path = self.translate_path(self.path)
        rng = self.headers.get("Range")
        self._left = None
        if rng is None or not os.path.isfile(path):
            return super().send_head()
        m = re.match(r"bytes=(\d*)-(\d*)", rng)
        size = os.path.getsize(path)
        if m.group(1):
            start = int(m.group(1))
            end = int(m.group(2)) if m.group(2) else size - 1
        else:
            start = max(0, size - int(m.group(2) or 0))
            end = size - 1
        end = min(end, size - 1)
        if start > end:
            self.send_error(416)
            return None
        f = open(path, "rb")
        f.seek(start)
        self.send_response(206)
        self.send_header("Content-Type", self.guess_type(path))
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.send_header("Content-Length", str(end - start + 1))
        self.end_headers()
        self._left = end - start + 1
        return f

    def copyfile(self, src, dst):
        left = getattr(self, "_left", None)
        if left is None:
            return super().copyfile(src, dst)
        try:
            while left > 0:
                b = src.read(min(1 << 16, left))
                if not b:
                    break
                dst.write(b)
                left -= len(b)
        except (BrokenPipeError, ConnectionResetError):
            pass

class S(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True

os.chdir(sys.argv[2])
S(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
