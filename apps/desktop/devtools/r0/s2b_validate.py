"""Validate S2B wire data or independently replay a committed source-bound bundle."""
from __future__ import annotations

import argparse
import json
import re
import struct
import subprocess
import sys
import zipfile
import zlib
from pathlib import Path

import contracts
import s2b_contracts as s2b


MAX_ZIP_ENTRIES = 64
MAX_ZIP_FILE = 1024 * 1024
MAX_ZIP_TOTAL = 6 * 1024 * 1024
ZIP_ROOTS = {"n02d", "n04b", "producer"}
ZIP_FILES = {"feature-manifest.json", "default-manifest.json", "build-record.json",
             "source-identity.json", "digests.json", "replay-n02d.json",
             "replay-n04b.json", "replay-swapped.json"}
# Archive-format failures are one closed input category, never parser text or a name.
ZIP_FAILURES = (zipfile.BadZipFile, zipfile.LargeZipFile, zlib.error, struct.error,
                EOFError, NotImplementedError, RuntimeError, ValueError)
# A directory member may carry only an empty compression stream: nothing stored, or
# zlib's canonical two-byte empty deflate stream. Anything longer can hide data.
EMPTY_DIRECTORY_STREAM = {zipfile.ZIP_STORED: 0, zipfile.ZIP_DEFLATED: 2}
# Win32 resolves these names to devices even below a directory (observed: a member
# named NUL "writes" successfully without creating any file), so such a component is
# never a materialised file; refuse it before any write.
WINDOWS_DEVICES = frozenset(
    ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"]
    + [f"COM{index}" for index in range(1, 10)]
    + [f"LPT{index}" for index in range(1, 10)]
)


def member_parts(info: zipfile.ZipInfo) -> list[str]:
    """Normalized relative components; only a directory's single terminal slash is allowed."""
    name = info.filename.replace("\\", "/")
    if name.endswith("/"):
        if not info.is_dir():
            s2b.fail("ZIP_MEMBER")
        name = name[:-1]
    parts = name.split("/")
    if (not name or any(part == "" for part in parts) or ".." in parts or "." in parts
            or ":" in name or "\0" in name
            or any(part.endswith(('.', ' ')) for part in parts)
            or any(part.split(".", 1)[0].upper() in WINDOWS_DEVICES for part in parts)
            or (info.external_attr >> 16) & 0o170000 == 0o120000):
        s2b.fail("ZIP_MEMBER")
    return parts


def safe_extract(zip_path: str, target: Path) -> None:
    """Pin the archive, validate every member, then extract with bounded reads."""
    from windows_owner import Pin
    with Pin(Path(zip_path)) as pin:
        if pin.file is None:
            s2b.fail("ZIP_MEMBER")
        try:
            archive = zipfile.ZipFile(pin.file)
            infos = archive.infolist()
            if len(infos) > MAX_ZIP_ENTRIES:
                s2b.fail("ZIP_ENTRY_LIMIT")
            seen = set()
            total = 0
            plans = []
            for info in infos:
                parts = member_parts(info)
                if len(parts) == 1:
                    if not ((info.is_dir() and parts[0] in ZIP_ROOTS) or parts[0] in ZIP_FILES):
                        s2b.fail("ZIP_UNEXPECTED")
                elif parts[0] not in ZIP_ROOTS:
                    s2b.fail("ZIP_UNEXPECTED")
                alias = "/".join(part.lower().rstrip('. ') for part in parts)
                if alias in seen:
                    s2b.fail("ZIP_DUPLICATE")
                seen.add(alias)
                if info.is_dir():
                    # Directory data is never extracted. A nonzero declared size or a
                    # payload larger than an empty stream would consume the bounded
                    # total without being written, so both are refused.
                    empty = EMPTY_DIRECTORY_STREAM.get(info.compress_type)
                    if info.file_size != 0 or empty is None or info.compress_size > empty:
                        s2b.fail("ZIP_SIZE")
                else:
                    if info.file_size > MAX_ZIP_FILE or total + info.file_size > MAX_ZIP_TOTAL:
                        s2b.fail("ZIP_SIZE")
                    total += info.file_size
                plans.append((info, parts))
            written = 0
            for info, parts in plans:
                destination = target.joinpath(*parts)
                if info.is_dir():
                    with archive.open(info) as source:
                        if source.read(MAX_ZIP_FILE + 1):
                            s2b.fail("ZIP_SIZE")
                    destination.mkdir(parents=True, exist_ok=True)
                    continue
                destination.parent.mkdir(parents=True, exist_ok=True)
                with archive.open(info) as source:
                    data = source.read(MAX_ZIP_FILE + 1)
                # The declared sizes above are a cheap pre-check; the caps are also
                # enforced on the bytes actually expanded, so a false header cannot
                # understate the archive.
                if len(data) > MAX_ZIP_FILE or written + len(data) > MAX_ZIP_TOTAL:
                    s2b.fail("ZIP_SIZE")
                written += len(data)
                destination.write_bytes(data)
        except s2b.ObservationError:
            raise
        except ZIP_FAILURES:
            # Corrupt, truncated, encrypted, unsupported or CRC-invalid input keeps
            # the closed code; no member name or parser detail is published.
            s2b.fail("ZIP_INVALID")
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
