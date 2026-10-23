"""MP-R0A-S1 offline contract validator (development-only).

Usage:
  python -B validate.py manifest FILE
  python -B validate.py result FILE
  python -B validate.py pair MANIFEST RESULT

Reads bounded UTF-8 JSON, validates its structure and exits 0 on success, 1 on a
contract failure. Performs no measurement, launch, fetch or execution of any value.
Error output contains only stable codes and JSON paths, never input text or paths.
"""

from __future__ import annotations

import json
import sys

import contracts


def _emit(payload: dict) -> None:
    sys.stdout.write(json.dumps(payload, sort_keys=True) + "\n")


def _fail(errors: list[dict]) -> int:
    _emit({"ok": False, "errors": errors})
    return 1


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        return _fail([{"code": "INVALID_VALUE", "path": "$"}])
    kind = argv[1]
    try:
        if kind == "manifest" and len(argv) == 3:
            document = contracts.load_document(argv[2], contracts.MAX_MANIFEST_BYTES)
            manifest = contracts.validate_manifest(document)
            _emit(
                {
                    "ok": True,
                    "kind": "manifest",
                    "manifest_version": contracts.MANIFEST_VERSION,
                    "scenario_id": manifest["scenario"]["id"],
                    "definition_hash": manifest["identity"]["definition_hash"],
                }
            )
            return 0
        if kind == "result" and len(argv) == 3:
            document = contracts.load_document(argv[2], contracts.MAX_RESULT_BYTES)
            result = contracts.validate_result(document)
            _emit(
                {
                    "ok": True,
                    "kind": "result",
                    "result_version": contracts.RESULT_VERSION,
                    "runs": len(result["runs"]),
                }
            )
            return 0
        if kind == "pair" and len(argv) == 4:
            manifest = contracts.validate_manifest(
                contracts.load_document(argv[2], contracts.MAX_MANIFEST_BYTES)
            )
            result = contracts.validate_result(
                contracts.load_document(argv[3], contracts.MAX_RESULT_BYTES)
            )
            contracts.check_identity(manifest, result)
            _emit({"ok": True, "kind": "pair"})
            return 0
        return _fail([{"code": "INVALID_VALUE", "path": "$"}])
    except contracts.ContractError as error:
        return _fail(error.errors)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
