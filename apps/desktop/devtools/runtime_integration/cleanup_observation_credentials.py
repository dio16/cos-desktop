#!/usr/bin/env python3
"""Recorded follow-up cleanup for Desktop observation credentials (test hygiene only).

The shared ``cleanup_credential_store`` in ``run_integration.py`` used to delete only
the bare ``cos-desktop/observation/<sha256(origin)[:32]>`` service string. The Desktop
writes the keyring-composed ``desktop-observation.cos-desktop/observation/<hash>``
target, so that delete never matched (``CMDKEY: Element not found``) and a stored
entry could survive between runs. The shared helper now attempts the composed target
first and records every attempt's exit code and outcome (``deleted``/``not_found``/
``failed``).

This driver makes the fix verifiable after the fact:

* ``self_test`` - offline check of the composed target string construction.
* ``--round-trip`` - creates a throwaway composed entry for a synthetic origin with
  ``cmdkey /generic``, deletes it through the fixed helper, and records the exact
  exit code (proves ``deleted`` is reachable with an exit-0 record), then verifies
  the entry is gone. The synthetic origin is never used by the Desktop.
* For every ``--origin``: lists the Credential Manager entries that actually exist
  for both target forms, runs the fixed helper, and lists them again, so the record
  distinguishes a real deletion from "already absent".

This never touches an entry it did not name explicitly, never token-audits the
store, and never fails a harness: every attempt is collected into one JSON record.

Usage::

    python apps/desktop/devtools/runtime_integration/cleanup_observation_credentials.py \
        --origin http://127.0.0.1:8132 --origin http://127.0.0.1:8133 \
        --round-trip --output docs/development/evidence-published/acc1-native-latest/cleanup-final-fix.json
"""

from __future__ import annotations

import argparse
import hashlib
import json
import secrets
import subprocess
import sys
from datetime import datetime
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import run_integration as base  # noqa: E402 - local sibling module, path set above

ROUND_TRIP_ORIGIN = "http://127.0.0.1:1"


def now_iso() -> str:
    return datetime.now().astimezone().isoformat(timespec="seconds")


def _listed_entries(output: str) -> list[str]:
    """Extract target names from a ``cmdkey /list`` output (filtered or not).

    An unfiltered listing prints ``Target: LegacyGeneric:target=<name>``; a
    ``/list:<name>`` listing prints ``Target: <name>`` (no prefix). A Japanese
    locale prints ``ターゲット:`` instead of ``Target:``.
    """
    names: list[str] = []
    for line in output.splitlines():
        if "target=" in line:
            names.append(line.split("target=", 1)[1].strip())
            continue
        stripped = line.strip()
        for marker in ("Target:", "ターゲット:"):
            if stripped.startswith(marker):
                names.append(stripped[len(marker) :].strip())
                break
    return names


def listed_targets(filter_text: str) -> dict:
    """Return the exact generic-credential target names ``cmdkey /list`` shows."""
    completed = subprocess.run(
        ["cmdkey", f"/list:{filter_text}"], capture_output=True, text=True, timeout=20
    )
    output = (completed.stdout + completed.stderr).strip()
    return {
        "exit": completed.returncode,
        "targets": _listed_entries(output),
        "output_tail": output[-300:],
    }


def presence(origin: str) -> dict:
    """Which of the two target forms currently exist for ``origin`` (exact match)."""
    composed, bare = base.credential_targets(origin)
    result: dict = {"origin": origin}
    for name, target in (("composed", composed), ("bare", bare)):
        listing = listed_targets(target)
        result[name] = {
            "target": target,
            "present": target in listing["targets"],
            "list_exit": listing["exit"],
            "listed": listing["targets"],
        }
    return result


def round_trip() -> dict:
    """Prove the fixed helper deletes the composed target with an exit-0 record."""
    head = hashlib.sha256(ROUND_TRIP_ORIGIN.encode("utf-8")).hexdigest()[:32]
    composed, bare = base.credential_targets(ROUND_TRIP_ORIGIN)
    password = secrets.token_hex(8)
    created = subprocess.run(
        ["cmdkey", f"/generic:{composed}", "/user:desktop-observation", f"/pass:{password}"],
        capture_output=True,
        text=True,
        timeout=20,
    )
    before = presence(ROUND_TRIP_ORIGIN)
    cleanup = base.cleanup_credential_store(ROUND_TRIP_ORIGIN)
    after = presence(ROUND_TRIP_ORIGIN)
    return {
        "origin": ROUND_TRIP_ORIGIN,
        "head": head,
        "password_recorded": False,
        "create_exit": created.returncode,
        "create_output": (created.stdout + created.stderr).strip()[:200],
        "present_before": before["composed"]["present"],
        "cleanup": cleanup,
        "present_after": after["composed"]["present"],
        "composed_target": composed,
        "bare_target": bare,
        "ok": before["composed"]["present"]
        and not after["composed"]["present"]
        and any(
            attempt.get("target") == composed
            and attempt.get("exit") == 0
            and attempt.get("result") == "deleted"
            for attempt in cleanup["targets"]
        ),
    }


def git_head(root: Path) -> dict:
    try:
        completed = subprocess.run(
            ["git", "-C", str(root), "rev-parse", "HEAD"], capture_output=True, text=True, timeout=20
        )
        status = subprocess.run(
            ["git", "-C", str(root), "status", "--porcelain"], capture_output=True, text=True, timeout=20
        )
        return {
            "head": completed.stdout.strip(),
            "dirty_entries": len(status.stdout.strip().splitlines()),
        }
    except (OSError, subprocess.SubprocessError) as error:  # pragma: no cover - environment
        return {"error": type(error).__name__}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--origin", action="append", default=[], help="observation origin to clean")
    parser.add_argument("--round-trip", action="store_true", help="prove composed deletion with a throwaway entry")
    parser.add_argument("--output", type=Path, default=None, help="write the record here")
    args = parser.parse_args(argv)

    desktop_root = Path(__file__).resolve().parents[4]
    record: dict = {
        "at": now_iso(),
        "purpose": (
            "verified cleanup of Desktop observation credentials in the Windows Credential "
            "Manager after the shared helper was fixed to delete the composed keyring target"
        ),
        "helper": {
            "module": "apps/desktop/devtools/runtime_integration/run_integration.py",
            "function": "cleanup_credential_store",
            "targets_attempted": ["desktop-observation.cos-desktop/observation/<hash>", "cos-desktop/observation/<hash>"],
            "note": (
                "cmdkey /delete matches the whole target; the bare service string never matched the "
                "keyring-composed entry, so a bare-only delete reported 'Element not found' and left "
                "any stored entry in place"
            ),
        },
        "python": sys.version.split()[0],
        "git": git_head(desktop_root),
        "self_test": base.credential_target_self_test(),
        "round_trip": round_trip() if args.round_trip else {"performed": False},
        "origins": [],
    }
    for origin in args.origin:
        record["origins"].append(
            {
                "origin": origin,
                "head": hashlib.sha256(origin.encode("utf-8")).hexdigest()[:32],
                "present_before": presence(origin),
                "cleanup": base.cleanup_credential_store(origin),
                "present_after": presence(origin),
            }
        )
    record["verdict"] = {
        "self_test_ok": bool(record["self_test"]["ok"]),
        "round_trip_ok": bool(record["round_trip"].get("ok")) if args.round_trip else None,
        "origins_recorded": len(record["origins"]),
    }
    text = json.dumps(record, indent=2, ensure_ascii=False)
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text + "\n", encoding="utf-8")
        print(f"wrote {args.output}", file=sys.stderr)
    print(text)
    return 0 if all(
        (record["verdict"]["self_test_ok"],)
        + ((record["round_trip"].get("ok", False),) if args.round_trip else ())
    ) else 1


if __name__ == "__main__":
    raise SystemExit(main())
