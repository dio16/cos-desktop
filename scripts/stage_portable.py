#!/usr/bin/env python3
"""Build, stage, and independently preflight an unsigned internal Windows package.

This is not a distribution or release tool. It builds the clean source commit with locked Cargo,
then atomically publishes only cos-desktop.exe, LICENSE, NOTICE, and a generated manifest.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

BINARY_NAME = "cos-desktop.exe"
LICENSE_NAMES = ("LICENSE", "NOTICE")
MANIFEST_NAME = "manifest.json"
PAYLOAD_NAMES = (BINARY_NAME, *LICENSE_NAMES)
MANIFEST_FORMAT = "cos-desktop-internal-portable-v1"
UNSIGNED_STATUS = "UNSIGNED_INTERNAL_STAGING"
MACHINE_NAMES = {0x014C: "x86", 0x8664: "x86_64", 0xAA64: "aarch64"}
MAX_PE_BYTES = 128 * 1024 * 1024
MAX_MANIFEST_BYTES = 1024 * 1024


class StageError(RuntimeError):
    pass


def file_stamp(info: os.stat_result) -> tuple:
    # Windows stat/fstat disagree on creation time and suffix-derived executable bits.
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns,
            info.st_nlink, stat.S_IFMT(info.st_mode), info.st_file_attributes if os.name == "nt" else 0)


def is_owned(path: Path, identity: tuple) -> bool:
    try:
        return file_stamp(path.stat(follow_symlinks=False))[:2] == identity
    except OSError:
        return False


def read_snapshot(path: Path, limit: int) -> tuple[bytes, str, tuple]:
    before = path.stat(follow_symlinks=False)
    stamp = file_stamp(before)
    if not regular_file(path):
        raise StageError(f"package contains non-regular file or link: {path.name}")
    if before.st_size > limit:
        raise StageError(f"package file exceeds {limit}-byte limit: {path.name}")
    with path.open("rb") as stream:
        if file_stamp(os.fstat(stream.fileno())) != stamp:
            raise StageError(f"package file changed before reading: {path.name}")
        data = stream.read(before.st_size + 1)
        digest = hashlib.sha256(data).hexdigest()
        if len(data) != before.st_size or file_stamp(os.fstat(stream.fileno())) != stamp:
            raise StageError(f"package file changed while reading: {path.name}")
    if file_stamp(path.stat(follow_symlinks=False)) != stamp:
        raise StageError(f"package file changed after reading: {path.name}")
    return data, digest, stamp


def regular_file(path: Path, *, single_link: bool = True) -> bool:
    try:
        info = path.stat(follow_symlinks=False)
    except OSError:
        return False
    if os.name == "nt" and info.st_file_attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT:
        return False
    return stat.S_ISREG(info.st_mode) and (not single_link or info.st_nlink == 1) and not path.is_symlink()


def run(command: list[str], cwd: Path) -> str:
    # Independent checks may have no PATH; keep tool paths out of the manifest.
    if command[0] in ("cargo", "git"):
        command = [os.environ.get(command[0].upper()) or command[0], *command[1:]]
    try:
        result = subprocess.run(command, cwd=cwd, check=False, capture_output=True, text=True, encoding="utf-8", errors="replace")
    except OSError as exc:
        raise StageError(f"could not run {command[0]}: {exc}") from exc
    if result.returncode:
        raise StageError(f"command failed ({result.returncode}): {command[0]} {command[1:]}\n{result.stderr.strip()}")
    return result.stdout


def read_source_identity(root: Path) -> tuple[str, str]:
    status = run(["git", "status", "--porcelain", "--untracked-files=all"], root)
    if status.strip():
        raise StageError("source checkout is not clean; refusing to build an unpinned tree")
    commit = run(["git", "rev-parse", "--verify", "HEAD^{commit}"], root).strip()
    tree = run(["git", "rev-parse", "--verify", "HEAD^{tree}"], root).strip()
    if len(commit) != 40 or len(tree) != 40:
        raise StageError("git did not return full source commit/tree identities")
    return commit, tree


def cargo_package(root: Path) -> tuple[dict, str]:
    try:
        metadata = json.loads(run(["cargo", "metadata", "--locked", "--format-version", "1", "--no-deps"], root))
    except (json.JSONDecodeError, TypeError) as exc:
        raise StageError(f"invalid cargo metadata: {exc}") from exc
    packages = [package for package in metadata.get("packages", []) if package.get("name") == "cos-desktop"]
    if len(packages) != 1:
        raise StageError("cargo metadata must identify exactly one cos-desktop package")
    package = packages[0]
    manifest = Path(package["manifest_path"])
    if manifest.resolve() != (root / "apps" / "desktop" / "Cargo.toml").resolve():
        raise StageError("cos-desktop package metadata points outside the expected desktop manifest")
    if package.get("version") is None or not package.get("id"):
        raise StageError("cargo metadata omitted Desktop package version or identity")
    return package, package["id"]


def build_executable(root: Path, package_id: str) -> tuple[Path, str, str, str]:
    cargo_version = run(["cargo", "--version"], root).strip()
    rustc_program = os.environ.get("RUSTC") or "rustc"
    rustc_version = run([rustc_program, "--version"], root).strip()
    command = ["cargo", "build", "--locked", "--package", "cos-desktop", "--bin", "cos-desktop", "--message-format=json-render-diagnostics"]
    output = run(command, root)
    artifacts: list[Path] = []
    for line in output.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        target = event.get("target") or {}
        if (event.get("reason") == "compiler-artifact" and event.get("package_id") == package_id
                and target.get("name") == "cos-desktop" and "bin" in target.get("kind", [])):
            executable = event.get("executable")
            if executable:
                artifacts.append(Path(executable))
    if len(artifacts) != 1:
        raise StageError(f"Cargo reported {len(artifacts)} compiler-artifact executables for cos-desktop; expected exactly one")
    executable = artifacts[0]
    # Cargo normally hard-links its reported executable to the deps artifact.
    # Copying creates an independent staged file, whose verifier requires one link.
    if not regular_file(executable, single_link=False):
        raise StageError("Cargo compiler-artifact executable is not a regular file")
    return executable, cargo_version, rustc_version, " ".join(command)


def _u16(data: bytes, offset: int) -> int:
    if offset < 0 or offset + 2 > len(data):
        raise StageError("truncated PE header")
    return struct.unpack_from("<H", data, offset)[0]


def _u32(data: bytes, offset: int) -> int:
    if offset < 0 or offset + 4 > len(data):
        raise StageError("truncated PE header")
    return struct.unpack_from("<I", data, offset)[0]


def inspect_pe(data: bytes) -> tuple[str, list[str]]:
    if len(data) < 64 or data[:2] != b"MZ":
        raise StageError("binary is not a valid PE image (missing DOS header)")
    pe_offset = _u32(data, 0x3C)
    if pe_offset + 24 > len(data) or data[pe_offset:pe_offset + 4] != b"PE\0\0":
        raise StageError("binary is not a valid PE image")
    machine = _u16(data, pe_offset + 4)
    arch = MACHINE_NAMES.get(machine)
    if arch is None:
        raise StageError(f"unsupported PE machine type: 0x{machine:04x}")
    section_count = _u16(data, pe_offset + 6)
    optional_size = _u16(data, pe_offset + 20)
    optional = pe_offset + 24
    magic = _u16(data, optional)
    if magic == 0x10B:
        directory_offset = optional + 96
    elif magic == 0x20B:
        directory_offset = optional + 112
    else:
        raise StageError("unsupported PE optional-header format")
    if optional + optional_size > len(data):
        raise StageError("truncated PE optional header")
    import_rva = _u32(data, directory_offset + 8) if directory_offset + 16 <= optional + optional_size else 0
    sections_offset = optional + optional_size
    sections = []
    for index in range(section_count):
        offset = sections_offset + index * 40
        if offset + 40 > len(data):
            raise StageError("truncated PE section table")
        virtual_size, virtual_address, raw_size, raw_offset = struct.unpack_from("<IIII", data, offset + 8)
        sections.append((virtual_address, max(virtual_size, raw_size), raw_offset, raw_size))

    def rva_offset(rva: int) -> int:
        for va, span, raw, raw_size in sections:
            if va <= rva < va + span:
                result = raw + rva - va
                if result >= raw + raw_size or result >= len(data):
                    break
                return result
        raise StageError("PE import directory refers outside file-backed sections")

    imports: set[str] = set()
    if import_rva:
        cursor = rva_offset(import_rva)
        for _ in range(4096):
            if cursor + 20 > len(data):
                raise StageError("truncated PE import table")
            descriptor = struct.unpack_from("<IIIII", data, cursor)
            if not any(descriptor):
                break
            name_offset = rva_offset(descriptor[3])
            end = data.find(b"\0", name_offset, min(len(data), name_offset + 4096))
            if end < 0:
                raise StageError("unterminated PE import name")
            try:
                name = data[name_offset:end].decode("ascii")
            except UnicodeDecodeError as exc:
                raise StageError("non-ASCII PE import name") from exc
            if not name or "/" in name or "\\" in name:
                raise StageError("invalid PE import name")
            imports.add(name)
            cursor += 20
        else:
            raise StageError("PE import table exceeds safety bound")
        if cursor + 20 > len(data) or any(struct.unpack_from("<IIIII", data, cursor)):
            raise StageError("PE import table has no terminator")
    return arch, sorted(imports, key=lambda name: (name.casefold(), name))


def make_manifest(package: dict, commit: str, tree: str, cargo_version: str, rustc_version: str, build_command: str, payload: dict[str, Path]) -> dict:
    snapshots = {name: read_snapshot(source, MAX_PE_BYTES if name == BINARY_NAME else MAX_MANIFEST_BYTES)
                 for name, source in sorted(payload.items())}
    binary_data, binary_digest, _ = snapshots[BINARY_NAME]
    arch, imports = inspect_pe(binary_data)
    entries = [{"path": name, "sha256": snapshot[1]} for name, snapshot in snapshots.items()]
    return {
        "format": MANIFEST_FORMAT,
        "desktop_version": package["version"],
        "protocol_version": {"status": "UNKNOWN_NOT_NEGOTIATED", "version": None},
        "schema_version": {"status": "UNKNOWN_NOT_NEGOTIATED", "version": None},
        "platform": "windows",
        "architecture": arch,
        "source": {"commit": commit, "tree": tree, "worktree": "CLEAN"},
        "build": {"profile": "dev", "command": build_command, "cargo_version": cargo_version, "rustc_version": rustc_version},
        "binary": {"path": BINARY_NAME, "sha256": binary_digest},
        "files": entries,
        "licenses": {"LICENSE": "INCLUDED", "NOTICE": "INCLUDED"},
        "external_runtime_dependencies": {
            "observed_direct_pe_imports": imports,
            "delay_load_imports": "NOT_INSPECTED",
            "transitive_runtime_dependencies": "UNKNOWN_NOT_INFERRED",
            "redistributable_supply_policy": "UNRESOLVED_DD21",
            "dependency_classification": "NOT_CLASSIFIED_SYSTEM_VS_REDISTRIBUTABLE",
        },
        "signing_status": UNSIGNED_STATUS,
        "packaging_status": UNSIGNED_STATUS,
        "verification_status": "VERIFIED_INTERNAL_ONLY",
        "distribution_ready": False,
        "limitations": ["UNSIGNED", "INSTALLER_NOT_SELECTED", "PUBLISHER_NOT_SELECTED", "DISTRIBUTION_CHANNEL_NOT_SELECTED", "UNINSTALL_POLICY_UNRESOLVED", "CLEAN_USER_NOT_MEASURED", "TRANSITIVE_DLL_CLOSURE_UNKNOWN", "THIRD_PARTY_LICENSE_CLOSURE_UNASSESSED", "DELAY_LOAD_IMPORTS_NOT_INSPECTED", "DYNAMIC_LOAD_DEPENDENCIES_NOT_ENUMERATED"],
    }


def verify_directory(directory: Path, require_signed: bool = False) -> dict:
    if require_signed:
        raise StageError("distribution/require-signed preflight is unsupported and fails closed")
    if (not directory.is_dir() or directory.is_symlink()
            or (os.name == "nt" and directory.stat(follow_symlinks=False).st_file_attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT)):
        raise StageError("package path must be a real directory")
    directory_stamp = file_stamp(directory.stat(follow_symlinks=False))
    try:
        children = list(directory.iterdir())
    except OSError as exc:
        raise StageError(f"cannot inventory package directory: {exc}") from exc
    expected = set(PAYLOAD_NAMES) | {MANIFEST_NAME}
    names = {child.name for child in children}
    if names != expected:
        raise StageError(f"package inventory mismatch (missing={sorted(expected - names)}, extra={sorted(names - expected)})")
    stamps = {}
    try:
        manifest_data, _, stamps[MANIFEST_NAME] = read_snapshot(directory / MANIFEST_NAME, MAX_MANIFEST_BYTES)
        manifest = json.loads(manifest_data.decode("utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise StageError(f"cannot read package manifest: {exc}") from exc
    if not isinstance(manifest, dict) or manifest.get("format") != MANIFEST_FORMAT:
        raise StageError("unsupported package manifest")
    if set(manifest) != {
        "format", "desktop_version", "protocol_version", "schema_version", "platform", "architecture",
        "source", "build", "binary", "files", "licenses", "external_runtime_dependencies",
        "signing_status", "packaging_status", "verification_status", "distribution_ready", "limitations",
    }:
        raise StageError("manifest has missing or unexpected top-level fields")
    if not isinstance(manifest.get("desktop_version"), str) or not re.fullmatch(
            r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", manifest["desktop_version"]):
        raise StageError("manifest Desktop version is missing or malformed")
    source = manifest.get("source")
    if (not isinstance(source, dict) or set(source) != {"commit", "tree", "worktree"}
            or source.get("worktree") != "CLEAN"
            or any(not isinstance(source.get(key), str) or len(source[key]) != 40 for key in ("commit", "tree"))):
        raise StageError("manifest source identity is malformed or not recorded clean")
    if any(any(char not in "0123456789abcdef" for char in source[key]) for key in ("commit", "tree")):
        raise StageError("manifest source identity is malformed")
    build = manifest.get("build")
    expected_build_fields = {"profile", "command", "cargo_version", "rustc_version"}
    expected_build_command = "cargo build --locked --package cos-desktop --bin cos-desktop --message-format=json-render-diagnostics"
    if (not isinstance(build, dict) or set(build) != expected_build_fields
            or build.get("profile") != "dev" or build.get("command") != expected_build_command
            or not isinstance(build.get("cargo_version"), str) or not build["cargo_version"].startswith("cargo ")
            or not isinstance(build.get("rustc_version"), str) or not build["rustc_version"].startswith("rustc ")):
        raise StageError("manifest build identity is malformed")
    limitations = manifest.get("limitations")
    required_limits = {
        "UNSIGNED", "INSTALLER_NOT_SELECTED", "PUBLISHER_NOT_SELECTED", "DISTRIBUTION_CHANNEL_NOT_SELECTED",
        "UNINSTALL_POLICY_UNRESOLVED", "CLEAN_USER_NOT_MEASURED", "TRANSITIVE_DLL_CLOSURE_UNKNOWN",
        "THIRD_PARTY_LICENSE_CLOSURE_UNASSESSED", "DELAY_LOAD_IMPORTS_NOT_INSPECTED",
        "DYNAMIC_LOAD_DEPENDENCIES_NOT_ENUMERATED",
    }
    if not isinstance(limitations, list) or not all(isinstance(item, str) for item in limitations) or set(limitations) != required_limits or len(limitations) != len(required_limits):
        raise StageError("manifest omits required distribution/runtime limitations")
    fixed_states = {
        "signing_status": UNSIGNED_STATUS,
        "packaging_status": UNSIGNED_STATUS,
        "verification_status": "VERIFIED_INTERNAL_ONLY",
        "distribution_ready": False,
        "platform": "windows",
    }
    for key, expected_value in fixed_states.items():
        if manifest.get(key) != expected_value:
            raise StageError(f"manifest has invalid protected field: {key}")
    if manifest.get("licenses") != {name: "INCLUDED" for name in LICENSE_NAMES}:
        raise StageError("manifest does not attest both required license files")
    files = manifest.get("files")
    if (not isinstance(files, list) or len(files) != len(PAYLOAD_NAMES)
            or any(not isinstance(entry, dict) or set(entry) != {"path", "sha256"} for entry in files)
            or any(not isinstance(entry["path"], str) for entry in files)
            or {entry["path"] for entry in files} != set(PAYLOAD_NAMES)):
        raise StageError("manifest payload allowlist must be exactly binary, LICENSE, and NOTICE")
    binary = manifest.get("binary")
    if not isinstance(binary, dict) or set(binary) != {"path", "sha256"} or binary.get("path") != BINARY_NAME:
        raise StageError("manifest binary identity must use the fixed executable name")
    snapshots = {name: read_snapshot(directory / name, MAX_PE_BYTES if name == BINARY_NAME else MAX_MANIFEST_BYTES)
                 for name in PAYLOAD_NAMES}
    actual = {}
    for entry in files:
        if not isinstance(entry, dict) or set(entry) != {"path", "sha256"}:
            raise StageError("manifest file entry has unexpected fields")
        name, digest = entry["path"], entry["sha256"]
        if name not in PAYLOAD_NAMES or Path(name).name != name or not isinstance(digest, str) or len(digest) != 64:
            raise StageError("manifest contains unsafe path or malformed digest")
        try:
            int(digest, 16)
        except ValueError as exc:
            raise StageError("manifest contains malformed digest") from exc
        _, actual[name], stamps[name] = snapshots[name]
        if actual[name] != digest:
            raise StageError(f"payload digest mismatch: {name}")
    if manifest.get("binary", {}).get("sha256") != actual[BINARY_NAME]:
        raise StageError("binary digest does not match manifest binary identity")
    arch, imports = inspect_pe(snapshots[BINARY_NAME][0])
    if manifest.get("architecture") != arch:
        raise StageError("manifest architecture does not match PE binary")
    runtime = manifest.get("external_runtime_dependencies")
    if (not isinstance(runtime, dict)
            or set(runtime) != {"observed_direct_pe_imports", "delay_load_imports", "transitive_runtime_dependencies", "redistributable_supply_policy", "dependency_classification"}
            or runtime.get("observed_direct_pe_imports") != imports
            or runtime.get("delay_load_imports") != "NOT_INSPECTED"
            or runtime.get("transitive_runtime_dependencies") != "UNKNOWN_NOT_INFERRED"
            or runtime.get("redistributable_supply_policy") != "UNRESOLVED_DD21"
            or runtime.get("dependency_classification") != "NOT_CLASSIFIED_SYSTEM_VS_REDISTRIBUTABLE"):
        raise StageError("manifest runtime dependency claims do not match observed imports/unknown scope")
    if (manifest.get("protocol_version") != {"status": "UNKNOWN_NOT_NEGOTIATED", "version": None}
            or manifest.get("schema_version") != {"status": "UNKNOWN_NOT_NEGOTIATED", "version": None}):
        raise StageError("wire protocol/schema must remain unknown until actually negotiated")
    for name, stamp in stamps.items():
        if file_stamp((directory / name).stat(follow_symlinks=False)) != stamp:
            raise StageError(f"package file changed during verification: {name}")
    if ({child.name for child in directory.iterdir()} != expected
            or file_stamp(directory.stat(follow_symlinks=False)) != directory_stamp):
        raise StageError("package directory changed during verification")
    return manifest


def stage(root: Path, output: Path) -> dict:
    root = root.resolve()
    output = output.absolute()
    if output.exists() or output.is_symlink():
        raise StageError("output already exists; refusing replacement")
    try:
        output.resolve().relative_to(root)
    except ValueError:
        pass
    else:
        raise StageError("output must be outside the source checkout")
    if output.parent.exists() and not output.parent.is_dir():
        raise StageError("output parent is not a directory")
    commit, tree = read_source_identity(root)
    package, package_id = cargo_package(root)
    binary, cargo_version, rustc_version, build_command = build_executable(root, package_id)
    if not regular_file(root / "LICENSE") or not regular_file(root / "NOTICE"):
        raise StageError("required root LICENSE and NOTICE must be regular files")
    output.parent.mkdir(parents=True, exist_ok=True)
    # The package is assembled in a sibling temp directory and published with one rename.
    temporary = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    owned_identity = file_stamp(temporary.stat(follow_symlinks=False))[:2]
    try:
        payload = {BINARY_NAME: binary, "LICENSE": root / "LICENSE", "NOTICE": root / "NOTICE"}
        for name, source in payload.items():
            shutil.copyfile(source, temporary / name)
        manifest = make_manifest(package, commit, tree, cargo_version, rustc_version, build_command,
                                 {name: temporary / name for name in PAYLOAD_NAMES})
        (temporary / MANIFEST_NAME).write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        verify_directory(temporary)
        if read_source_identity(root) != (commit, tree):
            raise StageError("source commit/tree changed during staging")
        if output.exists() or output.is_symlink():
            raise StageError("output appeared during staging; refusing replacement")
        os.rename(temporary, output)
        try:
            return verify_directory(output)
        except BaseException:
            # Withdraw only a directory this run provably created. A replacement that
            # appeared between the identity check and the rename is moved back to its
            # own path instead of being deleted.
            if is_owned(output, owned_identity):
                os.rename(output, temporary)
                if not is_owned(temporary, owned_identity):
                    os.rename(temporary, output)
                    raise
            raise
    except BaseException as error:
        # This tool never deletes a staging directory. A path-based recursive delete
        # cannot be proven to hit the directory this run created rather than a
        # replacement, so the directory is left in place and named in the failure.
        if temporary.exists() and not isinstance(error, StageError):
            raise StageError(f"{error} (retained staging directory: {temporary.name})") from error
        if temporary.exists():
            error.add_note(f"retained staging directory: {temporary}")
        raise


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description="Internal unsigned Windows portable staging/preflight; never a release tool.")
    subparsers = result.add_subparsers(dest="command", required=True)
    staging = subparsers.add_parser("stage", help="locked clean-source build and atomic internal staging")
    staging.add_argument("--source-root", type=Path, default=Path(__file__).resolve().parents[1])
    staging.add_argument("--output", type=Path, required=True)
    staging.add_argument("--require-signed", action="store_true", help="unsupported distribution preflight; always fails closed")
    verification = subparsers.add_parser("verify", help="independently verify a staged package inventory and manifest")
    verification.add_argument("package", type=Path)
    verification.add_argument("--require-signed", action="store_true", help="unsupported distribution preflight; always fails closed")
    return result


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "stage":
            if args.require_signed:
                raise StageError("distribution/require-signed staging is unsupported and fails closed")
            manifest = stage(args.source_root, args.output)
        else:
            manifest = verify_directory(args.package, args.require_signed)
        print(json.dumps({"result": "OK", "signing_status": manifest["signing_status"], "package": str(args.output if args.command == "stage" else args.package)}, sort_keys=True))
        return 0
    except (StageError, OSError) as exc:
        print(f"preflight error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
