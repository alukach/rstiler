#!/usr/bin/env python3
"""Static file server that honours Range requests, which http.server does not.

    python3 fixtures/serve.py [port]     # default 8099
"""
import http.server, os, sys


class RangeHandler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        path = self.translate_path(self.path)
        if not os.path.isfile(path):
            return super().do_GET()
        size = os.path.getsize(path)
        rng = self.headers.get("Range")
        with open(path, "rb") as f:
            if rng:
                start, _, end = rng.split("=")[1].partition("-")
                start = int(start)
                end = int(end) if end else size - 1
                f.seek(start)
                body = f.read(end - start + 1)
                self.send_response(206)
                self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
            else:
                body = f.read()
                self.send_response(200)
        self.send_header("Content-Type", "image/tiff")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Accept-Ranges", "bytes")
        self.end_headers()
        self.wfile.write(body)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8099
    os.chdir(os.path.dirname(os.path.abspath(__file__)))
    print(f"serving {os.getcwd()} on http://127.0.0.1:{port}")
    http.server.HTTPServer(("127.0.0.1", port), RangeHandler).serve_forever()
