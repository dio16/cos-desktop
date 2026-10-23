"""Validate S2B wire data or independently replay a committed source-bound bundle."""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

import contracts
import re
import s2b_contracts as s2b


MAX_ZIP_ENTRIES = 64
MAX_ZIP_FILE = 1024 * 1024
MAX_ZIP_TOTAL = 6 * 1024 * 1024
ZIP_ROOTS = {"n02d", "n04b", "producer"}
ZIP_FILES = {"feature-manifest.json", "default-manifest.json", "build-record.json",
             "source-identity.json", "digests.json", "replay-n02d.json",
             "replay-n04b.json", "replay-swapped.json"}


def safe_extract(zip_path: str, target: Path) -> None:
    """Pin the archive, validate every member, then extract with bounded reads."""
    import zipfile
    from windows_owner import Pin
    with Pin(Path(zip_path)) as pin:
        if pin.file is None:
            s2b.fail("ZIP_MEMBER")
        archive = zipfile.ZipFile(pin.file)
        infos = archive.infolist()
        if len(infos) > MAX_ZIP_ENTRIES:
            s2b.fail("ZIP_ENTRY_LIMIT")
        seen = set()
        total = 0
        plans = []
        for info in infos:
            name = info.filename.replace("\\", "/")
            parts = [part for part in name.split("/") if part]
            if (not parts or name.startswith("/") or ".." in parts or "." in parts
                    or ":" in name or "\0" in name
                    or any(part.endswith(('.', ' ')) for part in parts)
                    or (info.external_attr >> 16) & 0o170000 == 0o120000):
                s2b.fail("ZIP_MEMBER")
            if len(parts) == 1:
                if not ((info.is_dir() and parts[0] in ZIP_ROOTS) or parts[0] in ZIP_FILES):
                    s2b.fail("ZIP_UNEXPECTED")
            elif parts[0] not in ZIP_ROOTS:
                s2b.fail("ZIP_UNEXPECTED")
            alias = "/".join(part.lower().rstrip('. ') for part in parts)
            if alias in seen:
                s2b.fail("ZIP_DUPLICATE")
            seen.add(alias)
            if not info.is_dir():
                if info.file_size > MAX_ZIP_FILE or total + info.file_size > MAX_ZIP_TOTAL:
                    s2b.fail("ZIP_SIZE")
                total += info.file_size
            plans.append((info, parts))
        for info, parts in plans:
            destination = target.joinpath(*parts)
            if info.is_dir():
                destination.mkdir(parents=True, exist_ok=True)
                continue
            destination.parent.mkdir(parents=True, exist_ok=True)
            with archive.open(info) as source:
                data = source.read(MAX_ZIP_FILE + 1)
            if len(data) > MAX_ZIP_FILE:
                s2b.fail("ZIP_SIZE")
            destination.write_bytes(data)
        pin.verify()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="operation", required=True)
    capture_parser = commands.add_parser("capture", help="wire contract only; not Native proof")
    capture_parser.add_argument("--capture", required=True)
    bundle_parser = commands.add_parser("bundle", help="S1/S2A/approval/producer/bundle replay")
    bundle_parser.add_argument("--manifest")
    bundle_parser.add_argument("--output-dir")
    bundle_parser.add_argument("--bundle-zip")
    bundle_parser.add_argument("--bundle-tag", default="n02d")
    bundle_parser.add_argument("--timeout-ms", type=int, required=True)
    args = parser.parse_args(argv)
    try:
        if args.operation == "capture":
            document = s2b.validate_capture(contracts.load_document(args.capture, s2b.MAX_BYTES))
            result = {"ok": True, "scope": "WIRE_CONTRACT_ONLY", "native_proven": False,
                      "retained": document["summary"]["retained"],
                      "endpoints": s2b.endpoint_matrix(document), "full_coverage": False}
        else:
            import s2b_capture, tempfile, shutil
            if not 1 <= args.timeout_ms <= 120_000:
                s2b.fail("TIMEOUT_RANGE")
            directory = None
            try:
                if not re.fullmatch(r"n0[24][bd]", args.bundle_tag):
                    s2b.fail("REPLAY_TARGET")
                if bool(args.output_dir) == bool(args.bundle_zip):
                    s2b.fail("REPLAY_TARGET")
                if args.bundle_zip:
                    directory = tempfile.mkdtemp(prefix="s2b-bundle-")
                    safe_extract(args.bundle_zip, Path(directory))
                    root = Path(directory)
                    manifest = root / "feature-manifest.json"
                    output = root / args.bundle_tag
                else:
                    manifest, output = Path(args.manifest), Path(args.output_dir)
                result = s2b_capture.verify_bundle(manifest, output, args.timeout_ms)
            finally:
                if directory:
                    shutil.rmtree(directory, ignore_errors=True)
        print(json.dumps(result, separators=(",", ":")))
        return 0
    except s2b.ObservationError as error:
        print(json.dumps({"ok": False, "error": str(error)}, separators=(",", ":")))
    except contracts.ContractError as error:
        print(json.dumps({"ok": False, "error": error.errors[0]["code"]}, separators=(",", ":")))
    except (RuntimeError, OSError, subprocess.SubprocessError):
        print('{"ok":false,"error":"S2B_REPLAY_INCOMPLETE"}')
    return 1


if __name__ == "__main__":
    sys.exit(main())
