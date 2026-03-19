import argparse
import base64
import json
import os
import secrets
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse


class UploadHandler(BaseHTTPRequestHandler):
    server_version = "self-agent-upload/0.1"

    def do_GET(self):
        parsed = urlparse(self.path)
        if parsed.path == "/health":
            self._json(200, {"ok": True})
            return

        if parsed.path.startswith("/files/"):
            rel = parsed.path[len("/files/") :]
            rel = rel.lstrip("/").replace("\\", "/")
            if ".." in rel.split("/"):
                self._json(400, {"ok": False, "error": "bad path"})
                return
            p = self.server.upload_dir / rel
            if not p.exists() or not p.is_file():
                self._json(404, {"ok": False, "error": "not found"})
                return
            self.send_response(200)
            self.send_header("Content-Type", guess_mime(p.name))
            self.send_header("Content-Length", str(p.stat().st_size))
            self.end_headers()
            with p.open("rb") as f:
                while True:
                    chunk = f.read(1024 * 1024)
                    if not chunk:
                        break
                    self.wfile.write(chunk)
            return

        self._json(404, {"ok": False, "error": "not found"})

    def do_POST(self):
        parsed = urlparse(self.path)
        if parsed.path != "/upload":
            self._json(404, {"ok": False, "error": "not found"})
            return

        if self.server.require_token:
            auth = self.headers.get("Authorization", "")
            expected = f"Bearer {self.server.require_token}"
            if auth != expected:
                self._json(401, {"ok": False, "error": "unauthorized"})
                return

        length = int(self.headers.get("Content-Length", "0") or "0")
        if length <= 0 or length > self.server.max_body_bytes:
            self._json(413, {"ok": False, "error": "payload too large"})
            return

        raw = self.rfile.read(length)
        try:
            payload = json.loads(raw.decode("utf-8"))
        except Exception:
            self._json(400, {"ok": False, "error": "invalid json"})
            return

        mime = str(payload.get("mime") or "application/octet-stream").strip().lower()
        data_b64 = payload.get("data_base64")
        if not isinstance(data_b64, str) or not data_b64.strip():
            self._json(400, {"ok": False, "error": "missing data_base64"})
            return

        try:
            data = base64.b64decode(data_b64, validate=True)
        except Exception:
            self._json(400, {"ok": False, "error": "invalid base64"})
            return

        ext = mime_to_ext(mime)
        name = f"{secrets.token_urlsafe(18)}{ext}"
        out_path = self.server.upload_dir / name
        out_path.write_bytes(data)

        url = f"{self.server.public_base.rstrip('/')}/files/{name}"
        self._json(200, {"url": url})

    def log_message(self, format, *args):
        if getattr(self.server, "quiet", False):
            return
        super().log_message(format, *args)

    def _json(self, code, obj):
        body = json.dumps(obj, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class UploadServer(ThreadingHTTPServer):
    def __init__(
        self,
        addr,
        handler,
        upload_dir: Path,
        public_base: str,
        require_token: str | None,
        max_body_bytes: int,
        quiet: bool,
    ):
        super().__init__(addr, handler)
        self.upload_dir = upload_dir
        self.public_base = public_base
        self.require_token = require_token
        self.max_body_bytes = max_body_bytes
        self.quiet = quiet


def guess_mime(name: str) -> str:
    n = name.lower()
    if n.endswith(".png"):
        return "image/png"
    if n.endswith(".jpg") or n.endswith(".jpeg"):
        return "image/jpeg"
    if n.endswith(".webp"):
        return "image/webp"
    if n.endswith(".gif"):
        return "image/gif"
    return "application/octet-stream"


def mime_to_ext(mime: str) -> str:
    if mime == "image/png":
        return ".png"
    if mime == "image/jpeg":
        return ".jpg"
    if mime == "image/webp":
        return ".webp"
    if mime == "image/gif":
        return ".gif"
    return ".bin"


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--host", default=os.environ.get("UPLOAD_HOST", "127.0.0.1"))
    p.add_argument("--port", type=int, default=int(os.environ.get("UPLOAD_PORT", "9000")))
    p.add_argument("--upload-dir", default=os.environ.get("UPLOAD_DIR", "uploads"))
    p.add_argument("--public-base", default=os.environ.get("PUBLIC_BASE", "http://127.0.0.1:9000"))
    p.add_argument("--token", default=os.environ.get("UPLOAD_TOKEN"))
    p.add_argument("--max-mb", type=int, default=int(os.environ.get("UPLOAD_MAX_MB", "16")))
    p.add_argument("--quiet", action="store_true")
    args = p.parse_args()

    upload_dir = Path(args.upload_dir).resolve()
    upload_dir.mkdir(parents=True, exist_ok=True)

    httpd = UploadServer(
        (args.host, args.port),
        UploadHandler,
        upload_dir=upload_dir,
        public_base=args.public_base,
        require_token=args.token,
        max_body_bytes=args.max_mb * 1024 * 1024,
        quiet=args.quiet,
    )

    t = threading.Thread(target=httpd.serve_forever, daemon=True)
    t.start()
    t.join()


if __name__ == "__main__":
    main()

