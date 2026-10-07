#!/usr/bin/env python3
"""Serves the click-test page and writes what the page says it saw, one line each, to the log file given."""
import http.server, sys, urllib.parse, json, time, os
here = os.path.dirname(os.path.abspath(__file__))
log = open(sys.argv[1], 'a', buffering=1)
layout_path = sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        if self.path.startswith('/log'):
            q = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)
            log.write(f"{q.get('t',['?'])[0]:>7} {q.get('m',[''])[0]}\n"); self.send_response(204); self.end_headers(); return
        body = open(os.path.join(here, 'page.html'), 'rb').read()
        self.send_response(200); self.send_header('Content-Type', 'text/html'); self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body)
    def do_POST(self):
        n = int(self.headers.get('Content-Length', 0)); data = self.rfile.read(n)
        if self.path.startswith('/layout'):
            open(layout_path, 'wb').write(data)
        elif self.path.startswith('/log'):
            q = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)
            log.write(f"{q.get('t',['?'])[0]:>7} {q.get('m',[''])[0]}\n")
        self.send_response(204); self.end_headers()
http.server.ThreadingHTTPServer(('127.0.0.1', 8765), H).serve_forever()
