#!/usr/bin/env python3
"""Test-only isolated Runtime launcher with a file-triggered fault injector.

This is the same in-process uvicorn launcher as ``isolated_runtime.py`` (so the
async lifespan and the RT8 descriptor shutdown still run), plus one bounded
test control channel: a trigger file asks this process to revoke every Desktop
credential held in the serving process's in-memory registry.

The trigger exists only for the MP-ACC1 native credential-revocation injection:
the Desktop's token is deliberately never exposed, so an external process
cannot revoke it through the product route. The harness therefore asks the
serving process itself to drop its registry entries; the app observes exactly
the same 401 the product's revoke route would produce. No token value is ever
written to the acknowledgement file.

It is a development/integration helper, not a product entry point: the argv,
config, database, discovery location and control directory are supplied by the
harness.
"""

from __future__ import annotations

import argparse
import json
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

TRIGGER_NAME = "revoke-desktop-credentials"
ACK_NAME = "revoke-desktop-credentials.ack.json"
LIST_TRIGGER_NAME = "list-desktop-credentials"
LIST_ACK_NAME = "list-desktop-credentials.ack.json"


def control_loop(control_dir: Path, stop_event: threading.Event) -> None:
    """Watch the test triggers; write an ack for each."""

    from orchestrator_app import api

    trigger = control_dir / TRIGGER_NAME
    ack = control_dir / ACK_NAME
    list_trigger = control_dir / LIST_TRIGGER_NAME
    list_ack = control_dir / LIST_ACK_NAME
    while not stop_event.is_set():
        try:
            if list_trigger.is_file():
                with api._desktop_credentials_lock:
                    principals = [
                        str(record.get("principal", ""))
                        for record in api._desktop_credentials.values()
                    ]
                    holders = len(api._desktop_credentials)
                document = {
                    "at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
                    "credential_count": holders,
                    "principals": principals,
                    "token_values_recorded": False,
                }
                list_ack.write_text(json.dumps(document, ensure_ascii=True), encoding="utf-8")
                list_trigger.unlink()
            if trigger.is_file():
                with api._desktop_credentials_lock:
                    principals = sorted(
                        {str(record.get("principal", "")) for record in api._desktop_credentials.values()}
                    )
                    revoked = len(api._desktop_credentials)
                    api._desktop_credentials.clear()
                document = {
                    "at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
                    "revoked_count": revoked,
                    "principals": principals,
                    "token_values_recorded": False,
                }
                ack.write_text(json.dumps(document, ensure_ascii=True), encoding="utf-8")
                trigger.unlink()
        except OSError:
            pass
        stop_event.wait(0.15)


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
    control_stop = threading.Event()
    control_thread = None
    if args.control_dir:
        control_dir = Path(args.control_dir)
        control_dir.mkdir(parents=True, exist_ok=True)
        control_thread = threading.Thread(
            target=control_loop, args=(control_dir, control_stop), name="acc1-control", daemon=True
        )
        control_thread.start()
    stop = Path(args.stop_file)
    while not stop.exists():
        if not thread.is_alive():
            return 0
        time.sleep(0.1)
    control_stop.set()
    server.should_exit = True
    thread.join(timeout=60)
    return 0 if not thread.is_alive() else 1


if __name__ == "__main__":
    raise SystemExit(main())
