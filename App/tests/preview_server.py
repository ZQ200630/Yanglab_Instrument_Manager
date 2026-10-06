"""Static loopback browser QA; normal use cannot control instruments.

Explicit factory injection is reserved for bounded contract tests. There is no
default controller, backend selector or instrument data source in the server.
"""

from __future__ import annotations

import copy
import json
import mimetypes
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import unquote, urlsplit

from App.worker.controller import ConsoleController
from App.worker.protocol import encode_v2, parse_v2
from App.worker.contracts import Request
import uuid
from App.worker.settings import _clean


WEB_ROOT = Path(__file__).resolve().parents[1] / "web"
SHIM_PATH = Path(__file__).resolve().parent / "preview-bridge.js"


class PreviewBridge:
    def __init__(self, *, controller_factory=None) -> None:
        self._lock = threading.RLock()
        self._controller_factory = controller_factory
        self._controller: ConsoleController | None = None
        self._settings = _clean({})

    def _management(self, request):
        with self._lock:
            if request.method == "settings_save":
                if set(request.params) != {"settings"}:
                    raise ValueError("settings_save requires only settings")
                self._settings = _clean(request.params["settings"])
            return copy.deepcopy(self._settings)

    def invoke(self, command: str, args: dict[str, Any]) -> Any:
        with self._lock:
            if command == "worker_start":
                config = args.get("config") if type(args) is dict else None
                if self._controller_factory is None:
                    raise ValueError("Native App required for instrument control")
                if type(config) is not dict or config:
                    raise ValueError("preview startup has no backend configuration")
                if self._controller is not None:
                    raise ValueError("preview worker is already running")
                self._controller = self._controller_factory()
                self._controller._management = self._management
                owner = self._controller
            else:
                owner = self._controller
        if owner is None:
            raise ValueError("preview worker is not running")
        if command == "worker_start":
            return owner.submit(Request(uuid.uuid4().hex, "ping", {}, None)).result().result
        if command == "worker_stop":
            report = owner.close()
            if not report.get("unreleased"):
                with self._lock:
                    if self._controller is owner:
                        self._controller = None
            return report
        if command != "worker_request":
            raise ValueError("unknown preview command")
        raw = args.get("request") if type(args) is dict else None
        request = parse_v2(json.dumps(raw, allow_nan=False))
        if request.method == "shutdown":
            raise ValueError("use worker_stop for shutdown")
        return json.loads(encode_v2(request.id, owner.submit(request).result()))

    def shutdown(self) -> None:
        with self._lock:
            owner = self._controller
        if owner is not None:
            report = owner.close()
            if not report.get("unreleased"):
                with self._lock:
                    if self._controller is owner:
                        self._controller = None


class PreviewHTTPServer(ThreadingHTTPServer):
    def __init__(self, address: tuple[str, int]):
        super().__init__(address, PreviewHandler)
        self.bridge = PreviewBridge()


class PreviewHandler(BaseHTTPRequestHandler):
    server: PreviewHTTPServer

    def log_message(self, _format: str, *_args: object) -> None:
        return

    def _send(self, code: int, body: bytes, content_type: str) -> None:
        self.send_response(code)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        path = unquote(urlsplit(self.path).path)
        if path == "/":
            page = (WEB_ROOT / "index.html").read_text(encoding="utf-8")
            marker = '<script type="module" src="./main.js"></script>'
            if marker not in page:
                self._send(500, b"frontend entry point is missing", "text/plain")
                return
            page = page.replace(marker, '<script src="./preview-bridge.js"></script>\n    ' + marker)
            self._send(200, page.encode("utf-8"), "text/html; charset=utf-8")
            return
        if path == "/preview-bridge.js":
            self._send(200, SHIM_PATH.read_bytes(), "text/javascript; charset=utf-8")
            return
        target = (WEB_ROOT / path.lstrip("/")).resolve()
        if not target.is_relative_to(WEB_ROOT.resolve()) or not target.is_file():
            self._send(404, b"not found", "text/plain")
            return
        content_type = mimetypes.guess_type(target.name)[0] or "application/octet-stream"
        self._send(200, target.read_bytes(), content_type)

    def do_POST(self) -> None:
        if urlsplit(self.path).path != "/__preview_invoke":
            self._send(404, b"not found", "text/plain")
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if not 0 < length <= 1_000_000:
                raise ValueError("preview request must be under 1 MB")
            payload = json.loads(self.rfile.read(length))
            if type(payload) is not dict:
                raise ValueError("preview request must be an object")
            outcome = self.server.bridge.invoke(payload.get("command"), payload.get("args"))
            body = json.dumps(outcome, ensure_ascii=False, allow_nan=False).encode("utf-8")
        except (TypeError, ValueError) as error:
            self._send(400, json.dumps({"error": str(error)}).encode("utf-8"),
                       "application/json; charset=utf-8")
            return
        self._send(200, body, "application/json; charset=utf-8")


def make_server(host: str = "127.0.0.1", port: int = 8766) -> PreviewHTTPServer:
    if host != "127.0.0.1":
        raise ValueError("preview server must bind loopback")
    return PreviewHTTPServer((host, port))


def main() -> None:
    import argparse

    parser = argparse.ArgumentParser(description="Static-only browser preview")
    parser.add_argument("--port", type=int, default=8766)
    args = parser.parse_args()
    server = make_server(port=args.port)
    print(f"Static preview: http://127.0.0.1:{server.server_port}/")
    try:
        server.serve_forever()
    finally:
        server.server_close()
        server.bridge.shutdown()


if __name__ == "__main__":
    main()
