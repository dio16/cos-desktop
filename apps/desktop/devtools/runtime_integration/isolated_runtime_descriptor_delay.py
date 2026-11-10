#!/usr/bin/env python3
"""Test-only isolated Runtime launcher with a media-descriptor delivery delay.

This is the same in-process launcher as ``isolated_runtime.py`` plus one ASGI
wrapper used only by the RT3 native-expiry harness. While the delay flag file
exists, the wrapper sleeps after a media **descriptor** response has been
produced and before its bytes are written to the client. The Runtime has
already minted the descriptor (and its handle capability) at the original
request time, so the client observes the real server-issued expiry rather than
a rewritten document. A delayed descriptor is still a real protocol response;
only its delivery is late. Blob/thumbnail responses and every non-media route
are untouched.

The wrapper is evidence scaffolding, not a product entry point. It is never
loaded by the application binary.
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

DELAY_ENV = "ORCHESTRATOR_HARNESS_MEDIA_DESCRIPTOR_DELAY_SECONDS"
FLAG_ENV = "ORCHESTRATOR_HARNESS_MEDIA_DESCRIPTOR_DELAY_FLAG"


def _is_descriptor(path: str) -> bool:
    return (
        path.startswith("/api/cos/media/")
        and not path.endswith("/blob")
        and not path.endswith("/thumbnail")
    )


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
            or not _is_descriptor(str(scope.get("path") or ""))
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
                    f"path={scope.get('path')} seconds={self.seconds:.3f} "
                    f"at={request_at:.3f}",
                    file=sys.stderr,
                    flush=True,
                )
                await asyncio.sleep(self.seconds)
                print(
                    "harness media descriptor delay release "
                    f"path={scope.get('path')} at={time.time():.3f}",
                    file=sys.stderr,
                    flush=True,
                )
            await send(message)

        await self.app(scope, receive, delayed_send)


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
    delay_seconds = float(os.environ.get(DELAY_ENV, "0") or 0)
    flag = Path(os.environ.get(FLAG_ENV, "")) if os.environ.get(FLAG_ENV) else None
    app = api.app
    if delay_seconds > 0 and flag is not None:
        app = DelayMediaDescriptor(app, flag=flag, seconds=delay_seconds)
        print(
            f"harness media descriptor delay armed seconds={delay_seconds:.3f} "
            f"flag={flag}",
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
