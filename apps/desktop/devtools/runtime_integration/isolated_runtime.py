#!/usr/bin/env python3
"""Test-only isolated Runtime launcher with a graceful stop trigger.

The production CLI enters uvicorn through ``orchestrator_app.api:main``, which
cannot be asked to stop gracefully from a harness. This launcher runs the same
FastAPI application and lifespan in-process, writes a readiness marker and exits
through ``Server.should_exit`` when a stop file appears, so the async lifespan
shutdown (including RT8 descriptor removal) actually runs.

It is a development/integration helper, not a product entry point: the argv,
config, database and discovery location are supplied by the harness.
"""

from __future__ import annotations

import argparse
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


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", required=True)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--stop-file", required=True)
    parser.add_argument("--ready-file", default=None)
    args = parser.parse_args(argv)

    from orchestrator_app import api

    api._load_config(args.config)
    config = uvicorn.Config(
        api.app, host=args.host, port=args.port, log_level="info", lifespan="on"
    )
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
