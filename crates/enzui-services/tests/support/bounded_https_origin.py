"""Private TLS range origin for bounded media-source integration tests."""
import http.server
import json
import os
from pathlib import Path
import ssl
import sys
import threading

root = Path(sys.argv[1])
total = int(sys.argv[2])
lock = threading.Lock()
state = {"bytes_sent": 0, "requests": [], "proxy_credentials_seen": False}


def publish():
    pending = root / "origin-stats.tmp"
    pending.write_text(json.dumps(state))
    os.replace(pending, root / "origin-stats.json")


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def do_HEAD(self):
        self.serve(False)

    def do_GET(self):
        self.serve(True)

    def serve(self, body):
        offset, end = 0, total - 1
        requested = self.headers.get("Range")
        if requested:
            assert requested.startswith("bytes=") and "," not in requested
            first, last = requested[6:].split("-", 1)
            offset = int(first)
            if last:
                end = min(end, int(last))
        with lock:
            state["requests"].append({"path": self.path, "range": requested,
                                      "version": self.request_version,
                                      "tls": self.connection.version()})
            state["proxy_credentials_seen"] |= "Proxy-Authorization" in self.headers
            publish()
        if offset >= total or end < offset:
            self.send_response(416)
            self.send_header("Content-Range", f"bytes */{total}")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        self.send_response(206 if requested else 200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(end - offset + 1))
        self.send_header("Connection", "close")
        if requested:
            self.send_header("Content-Range", f"bytes {offset}-{end}/{total}")
        self.end_headers()
        if not body:
            return
        remaining = end - offset + 1
        try:
            while remaining:
                length = min(remaining, 65536)
                part = bytes(((position * 37) ^ (position >> 8) ^
                              (position >> 16) ^ (position >> 24)) & 255
                             for position in range(offset, offset + length))
                self.wfile.write(part)
                offset += length
                remaining -= length
                with lock:
                    state["bytes_sent"] += len(part)
                    publish()
        except (BrokenPipeError, ConnectionResetError, ssl.SSLError):
            pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
server.daemon_threads = True
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.minimum_version = ssl.TLSVersion.TLSv1_2
context.set_alpn_protocols(["http/1.1"])
context.load_cert_chain(root / "certificate.pem", root / "key.pem")
server.socket = context.wrap_socket(server.socket, server_side=True)
with lock:
    publish()
(root / "ready.tmp").write_text(json.dumps({"port": server.server_port}))
os.replace(root / "ready.tmp", root / "ready.json")
server.serve_forever(poll_interval=0.05)
