#!/usr/bin/env python3
"""Test-only isolated Runtime launcher with media transport-delay wrappers.

This is the same in-process launcher as ``isolated_runtime.py`` plus two ASGI
wrappers used only by the RT3 media-refetch harness:

1. **descriptor delivery delay** (``..._DESCRIPTOR_DELAY_*``): while its flag
   file exists, the wrapper sleeps after a media *descriptor* response has been
   produced and before its bytes are written. The Runtime has already minted
   the descriptor (and its handle) at the original request time, so the client
   observes the real server-issued expiry rather than a rewritten document.
2. **blob request delay** (``..._BLOB_DELAY_*``): while its flag file exists,
   the wrapper sleeps *before* the Runtime handles a media ``/blob`` request.
   The server therefore receives the real handle after its real TTL and its own
   expiry check produces the wire ``410 MEDIA_HANDLE_EXPIRED``. This is
   harness-injected request latency, not a rewritten response.

Both delays are evidence scaffolding, not product entry points. They are never
loaded by the application binary, and they never rewrite a descriptor, handle
or blob. The blob wrapper logs the status (and the value-free error body for a
410) so the evidence can name the wire answer.
"""

from __future__ import annotations

import argparse
import asyncio
import os
import sys
import threading
import time
from pathlib import Path

# The harness runs this file with cwd = Runtime root; the Python interpreter only
# adds this script's directory, so the Runtime package root must be explicit.
RUNTIME_ROOT = Path(os.environ.get("ORCHESTRATOR_RUNTIME_ROOT", "C:/AI/orchestrator"))
sys.path.insert(0, str(RUNTIME_ROOT))

import uvicorn  # noqa: E402 - path setup must precede the import

DESCRIPTOR_DELAY_ENV = "ORCHESTRATOR_HARNESS_MEDIA_DESCRIPTOR_DELAY_SECONDS"
DESCRIPTOR_FLAG_ENV = "ORCHESTRATOR_HARNESS_MEDIA_DESCRIPTOR_DELAY_FLAG"
BLOB_DELAY_ENV = "ORCHESTRATOR_HARNESS_MEDIA_BLOB_DELAY_SECONDS"
BLOB_FLAG_ENV = "ORCHESTRATOR_HARNESS_MEDIA_BLOB_DELAY_FLAG"

MAX_LOGGED_BODY = 240


def _path(scope) -> str:
    return str(scope.get("path") or "")


def _is_descriptor(path: str) -> bool:
    return (
        path.startswith("/api/cos/media/")
        and not path.endswith("/blob")
        and not path.endswith("/thumbnail")
    )


def _is_blob(path: str) -> bool:
    return path.startswith("/api/cos/media/") and path.endswith("/blob")


class DelayMediaDescriptor:
    """Delay only the delivery of a media descriptor response."""

    def __init__(self, app, *, flag: Path, seconds: float):
        self.app = app
        self.flag = flag
        self.seconds = seconds

    async def __call__(self, scope, receive, send):
        if (
            scope.get("type") != "http"
            or scope.get("method") not in ("GET", "HEAD")
            or self.seconds <= 0
            or not self.flag.is_file()
            or not _is_descriptor(_path(scope))
        ):
            await self.app(scope, receive, send)
            return

        request_at = time.time()
        delayed = False

        async def delayed_send(message):
            nonlocal delayed
            if not delayed and message.get("type") == "http.response.start":
                delayed = True
                print(
                    "harness media descriptor delay begin "
                    f"path={_path(scope)} seconds={self.seconds:.3f} "
                    f"at={request_at:.3f}",
                    file=sys.stderr,
                    flush=True,
                )
                await asyncio.sleep(self.seconds)
                print(
                    "harness media descriptor delay release "
                    f"path={_path(scope)} at={time.time():.3f}",
                    file=sys.stderr,
                    flush=True,
                )
            await send(message)

        await self.app(scope, receive, delayed_send)


class DelayMediaBlob:
    """Delay a media blob request before the Runtime handles it."""

    def __init__(self, app, *, flag: Path, seconds: float):
        self.app = app
        self.flag = flag
        self.seconds = seconds

    async def __call__(self, scope, receive, send):
        if (
            scope.get("type") != "http"
            or scope.get("method") not in ("GET", "HEAD")
            or self.seconds <= 0
            or not self.flag.is_file()
            or not _is_blob(_path(scope))
        ):
            await self.app(scope, receive, send)
            return

        arrived_at = time.time()
        print(
            "harness media blob request delay begin "
            f"path={_path(scope)} seconds={self.seconds:.3f} at={arrived_at:.3f}",
            file=sys.stderr,
            flush=True,
        )
        await asyncio.sleep(self.seconds)
        print(
            "harness media blob request delay release "
            f"path={_path(scope)} at={time.time():.3f}",
            file=sys.stderr,
            flush=True,
        )

        status: dict[str, int] = {}
        body = bytearray()

        async def observed_send(message):
            kind = message.get("type")
            if kind == "http.response.start":
                status["code"] = int(message.get("status") or 0)
            elif kind == "http.response.body" and len(body) < MAX_LOGGED_BODY:
                body.extend(bytes(message.get("body") or b"")[:MAX_LOGGED_BODY])
            await send(message)

        await self.app(scope, receive, observed_send)
        code = status.get("code", 0)
        if code >= 400:
            snippet = body[:MAX_LOGGED_BODY].decode("utf-8", errors="replace")
            snippet = "".join(
                character if character.isprintable() else " " for character in snippet
            )
            print(
                "harness media blob response "
                f"path={_path(scope)} status={code} body={snippet!r}",
                file=sys.stderr,
                flush=True,
            )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", required=True)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--stop-file", required=True)
    parser.add_argument("--ready-file", default=None)
    parser.add_argument("--control-dir", default=None)
    args = parser.parse_args(argv)

    from orchestrator_app import api

    api._load_config(args.config)
    app = api.app

    descriptor_seconds = float(os.environ.get(DESCRIPTOR_DELAY_ENV, "0") or 0)
    descriptor_flag = (
        Path(os.environ[DESCRIPTOR_FLAG_ENV]) if os.environ.get(DESCRIPTOR_FLAG_ENV) else None
    )
    if descriptor_seconds > 0 and descriptor_flag is not None:
        app = DelayMediaDescriptor(app, flag=descriptor_flag, seconds=descriptor_seconds)
        print(
            f"harness media descriptor delay armed seconds={descriptor_seconds:.3f} "
            f"flag={descriptor_flag}",
            file=sys.stderr,
            flush=True,
        )

    blob_seconds = float(os.environ.get(BLOB_DELAY_ENV, "0") or 0)
    blob_flag = Path(os.environ[BLOB_FLAG_ENV]) if os.environ.get(BLOB_FLAG_ENV) else None
    if blob_seconds > 0 and blob_flag is not None:
        app = DelayMediaBlob(app, flag=blob_flag, seconds=blob_seconds)
        print(
            f"harness media blob delay armed seconds={blob_seconds:.3f} flag={blob_flag}",
            file=sys.stderr,
            flush=True,
        )

    config = uvicorn.Config(app, host=args.host, port=args.port, log_level="info", lifespan="on")
    server = uvicorn.Server(config)
    thread = threading.Thread(target=server.run, name="isolated-runtime", daemon=True)
    thread.start()
    deadline = time.monotonic() + 120
    while not server.started:
        if not thread.is_alive():
            print("isolated runtime failed before readiness", file=sys.stderr, flush=True)
            return 1
        if time.monotonic() > deadline:
            print("isolated runtime readiness timed out", file=sys.stderr, flush=True)
            server.should_exit = True
            thread.join(timeout=10)
            return 1
        time.sleep(0.05)
    if args.ready_file:
        Path(args.ready_file).write_text("ready", encoding="utf-8")
    stop = Path(args.stop_file)
    while not stop.exists():
        if not thread.is_alive():
            return 0
        time.sleep(0.1)
    server.should_exit = True
    thread.join(timeout=60)
    return 0 if not thread.is_alive() else 1


if __name__ == "__main__":
    raise SystemExit(main())
