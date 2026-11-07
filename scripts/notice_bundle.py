#!/usr/bin/env python3
"""Deterministic, offline third-party notice bundle for the internal Windows package.

The bundle is an evidence artefact, not a legal opinion. Every dependency that the
Cargo graph reaches over normal/build edges is bound to a text that is already on
this machine (the extracted Cargo source, the workspace files or a shared local
Git checkout). Nothing is fetched, inferred or invented:

* a package with no local text is recorded as TEXT_NOT_FOUND (UNASSESSED);
* a short relative-path pointer to a shared text is resolved only inside the
  local Cargo Git checkout tree, and the resolved text is included verbatim;
* a declared ``license-file`` is read only when it stays inside its package;
* the rendered bytes contain no absolute path, timestamp, machine name or
  hash-ordered data, so identical inputs always produce identical bytes.

``parse_bundle`` and ``check_manifest`` re-derive the dependency-set digest and
the counts from the bundle itself, so a staged package whose manifest disagrees
with its bundle fails closed.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import stat
from dataclasses import dataclass
from pathlib import Path

FORMAT = "cos-desktop-third-party-notices-v1"
PAYLOAD_NAME = "THIRD_PARTY_NOTICES.md"
SCOPE = "cargo-normal-build-reachable-x86_64-pc-windows-msvc"
CARGO_TARGET = "x86_64-pc-windows-msvc"
LEGAL_POSTURE = "NOT_ASSESSED"
MAX_TEXT_BYTES = 1024 * 1024
MAX_BUNDLE_BYTES = 16 * 1024 * 1024
MAX_POINTER_BYTES = 512
MAX_DECLARATION_CHARS = 256
TITLE = "COS Desktop third-party notice bundle"
INDEX_BEGIN = "<!-- dependency-index: begin -->"
INDEX_END = "<!-- dependency-index: end -->"
TEXT_SECTION = "## License texts"
BUNDLE_END = "<!-- end of third-party notice bundle -->"
HEADER_KEYS = ("format", "scope", "cargo-lock-sha256", "dependency-set-sha256",
               "dependencies", "text-found", "text-not-found", "legal-compliance")

LICENSE_TEXT = re.compile(r"(?i)^(?:licen[cs]e|copying|unlicense)(?:[._-][^\s]*)?$")
NOTICE_TEXT = re.compile(r"(?i)^notice(?:[._-][^\s]*)?$")
TEXT_KINDS = frozenset({"LICENSE", "NOTICE"})
POINTER_PATH = re.compile(r"(?:\.\./)+[A-Za-z0-9._-]+")
SHA256 = re.compile(r"[0-9a-f]{64}")
TEXT_FOUND = "TEXT_FOUND"
TEXT_NOT_FOUND = "TEXT_NOT_FOUND"
STATUSES = frozenset({TEXT_FOUND, TEXT_NOT_FOUND})
ROLES = frozenset({"normal-only", "build-only", "normal+build"})
SOURCE_KINDS = frozenset({"registry", "git", "workspace", "path"})
UNRESOLVED_REASONS = frozenset({
    "TEXT_UNREADABLE", "TEXT_NOT_UTF8", "TEXT_OVERSIZED", "TEXT_LINK", "TEXT_MISSING",
    "TEXT_OUTSIDE_PACKAGE", "POINTER_NOT_RESOLVED", "SOURCE_UNAVAILABLE",
})
ROLE_KEYS = {"normal-only": "normal_only", "build-only": "build_only", "normal+build": "normal_build"}

ROW_KEYS = frozenset({"name", "version", "source", "source_kind", "role", "declared_license",
                      "declared_license_file", "status", "texts", "unresolved"})
TEXT_KEYS = frozenset({"path", "sha256", "bytes", "kind"})
UNRESOLVED_KEYS = frozenset({"path", "reason"})
MANIFEST_KEYS = frozenset({
    "format", "payload_path", "scope", "cargo_lock_sha256", "dependency_set_sha256",
    "bundle_sha256", "bundle_bytes", "dependencies", "text_found", "text_not_found",
    "unresolved_texts", "roles", "metadata_packages", "reachable", "not_reachable",
    "legal_compliance",
})


class NoticeError(RuntimeError):
    pass


@dataclass(frozen=True)
class Text:
    path: str
    sha256: str
    data: bytes
    kind: str


@dataclass(frozen=True)
class Row:
    name: str
    version: str
    source: str
    source_kind: str
    role: str
    declared_license: str | None
    declared_license_file: str | None
    status: str
    texts: tuple[Text, ...]
    unresolved: tuple[tuple[str, str], ...]

    def payload(self) -> dict:
        return {
            "name": self.name,
            "version": self.version,
            "source": self.source,
            "source_kind": self.source_kind,
            "role": self.role,
            "declared_license": self.declared_license,
            "declared_license_file": self.declared_license_file,
            "status": self.status,
            "texts": [{"path": text.path, "sha256": text.sha256, "bytes": len(text.data),
                       "kind": text.kind} for text in self.texts],
            "unresolved": [{"path": path, "reason": reason} for path, reason in self.unresolved],
        }


@dataclass(frozen=True)
class ParsedBundle:
    cargo_lock_sha256: str
    dependency_set_sha256: str
    rows: tuple[dict, ...]
    blobs: dict[str, bytes]
    counts: dict


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _canonical(value) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def _regular(path: Path):
    try:
        info = path.stat(follow_symlinks=False)
    except OSError:
        return None
    if os.name == "nt" and info.st_file_attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT:
        return None
    if path.is_symlink() or not stat.S_ISREG(info.st_mode):
        return None
    return info


def _is_link(path: Path) -> bool:
    try:
        if path.is_symlink():
            return True
        info = path.stat(follow_symlinks=False)
    except OSError:
        return False
    return bool(os.name == "nt" and info.st_file_attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT)


def _require_logical_path(value) -> None:
    if not isinstance(value, str) or not value or len(value) > 256:
        raise NoticeError("notice bundle contains an unusable logical path")
    if "\\" in value or ":" in value or value.startswith("/"):
        raise NoticeError("notice bundle must not embed a filesystem path")
    if any(part in ("", ".", "..") for part in value.split("/")):
        raise NoticeError("notice bundle contains an unsafe logical path")


def _safe_relative(value) -> str | None:
    """Return a POSIX relative path that provably stays inside its package."""
    if not isinstance(value, str) or not value or len(value) > MAX_DECLARATION_CHARS:
        return None
    if value.startswith("/") or "\\" in value or ":" in value:
        return None
    candidate = Path(value)
    if candidate.is_absolute() or any(part in ("", ".", "..") for part in candidate.parts):
        return None
    return candidate.as_posix()


def _pointer_target(data: bytes) -> str | None:
    """A tiny single-line relative-path file is a pointer, not a licence text."""
    if len(data) > MAX_POINTER_BYTES:
        return None
    try:
        text = data.decode("utf-8").strip()
    except UnicodeDecodeError:
        return None
    if not text or "\n" in text or "\r" in text:
        return None
    return text if POINTER_PATH.fullmatch(text) else None


def _source(package: dict) -> tuple[str, str]:
    name, version = package["name"], package["version"]
    fragment = f"{name}@{version}"
    source = package.get("source")
    if source is None:
        return "workspace", f"workspace#{fragment}"
    if isinstance(source, str) and source.startswith("registry+"):
        return "registry", package["id"]
    if isinstance(source, str) and source.startswith("git+"):
        return "git", package["id"]
    # A local path source must never reach the staged bundle.
    return "path", f"path+<local>#{fragment}"


def _role(kinds: set[str]) -> str:
    if kinds == {"normal"}:
        return "normal-only"
    if kinds == {"build"}:
        return "build-only"
    return "normal+build"


def _package_root(package: dict) -> Path | None:
    manifest = package.get("manifest_path")
    if not isinstance(manifest, str) or not manifest:
        return None
    root = Path(manifest).parent
    return root if root.is_dir() else None


def _inside(path: Path, parent: Path) -> bool:
    try:
        Path(path).resolve().relative_to(Path(parent).resolve())
    except (OSError, ValueError):
        return False
    return True


def _read_text(path: Path) -> tuple[bytes | None, str | None]:
    info = _regular(path)
    if info is None:
        return None, "TEXT_LINK" if _is_link(path) else ("TEXT_MISSING" if not path.exists() else "TEXT_UNREADABLE")
    if info.st_size > MAX_TEXT_BYTES:
        return None, "TEXT_OVERSIZED"
    try:
        data = path.read_bytes()
    except OSError:
        return None, "TEXT_UNREADABLE"
    if len(data) > MAX_TEXT_BYTES:
        return None, "TEXT_OVERSIZED"
    try:
        data.decode("utf-8")
    except UnicodeDecodeError:
        return None, "TEXT_NOT_UTF8"
    return data, None


def _resolve_pointer(origin: Path, target: str, cargo_home: Path | None) -> tuple[str, bytes] | None:
    """Resolve a declared relative pointer only inside the local Cargo Git checkouts."""
    if cargo_home is None:
        return None
    try:
        checkouts = (cargo_home / "git" / "checkouts").resolve(strict=True)
        candidate = (origin / target).resolve(strict=True)
    except OSError:
        return None
    if not candidate.is_file():
        return None
    try:
        relative = candidate.relative_to(checkouts)
    except ValueError:
        return None
    data, reason = _read_text(candidate)
    if data is None or reason is not None:
        return None
    return relative.as_posix(), data


def _bind(package: dict, kind: str, workspace_root: Path, cargo_home: Path | None):
    texts: list[Text] = []
    unresolved: list[tuple[str, str]] = []
    declared = package.get("license_file")
    declared_file: str | None = None
    if declared is not None:
        declared_file = _safe_relative(declared)
        if declared_file is None:
            unresolved.append(("declared", "TEXT_OUTSIDE_PACKAGE"))
    if kind == "workspace":
        base, prefix = workspace_root, "workspace"
    else:
        base, prefix = _package_root(package), ""
    if base is None:
        return (), tuple(unresolved + [("source", "SOURCE_UNAVAILABLE")]), TEXT_NOT_FOUND, declared_file
    candidates: dict[str, tuple[Path, str]] = {}
    if declared_file is not None:
        logical = f"{prefix}/{declared_file}" if prefix else declared_file
        candidates[logical] = (base / declared_file, "LICENSE")
    try:
        entries = sorted(base.iterdir(), key=lambda entry: entry.name.casefold())
    except OSError:
        entries = []
    seen: dict[str, str] = {}
    for entry in entries:
        text_kind = "LICENSE" if LICENSE_TEXT.fullmatch(entry.name) else (
            "NOTICE" if NOTICE_TEXT.fullmatch(entry.name) else None)
        if text_kind is None:
            continue
        folded = entry.name.casefold()
        if folded in seen:
            raise NoticeError(f"ambiguous license text names differ only by case: {seen[folded]} / {entry.name}")
        seen[folded] = entry.name
        logical = f"{prefix}/{entry.name}" if prefix else entry.name
        candidates[logical] = (entry, text_kind)
    for logical in sorted(candidates):
        path, text_kind = candidates[logical]
        data, reason = _read_text(path)
        if data is None:
            unresolved.append((logical, reason))
            continue
        if kind == "git":
            target = _pointer_target(data)
            if target is not None:
                resolved = _resolve_pointer(path.parent, target, cargo_home)
                if resolved is None:
                    unresolved.append((logical, "POINTER_NOT_RESOLVED"))
                    continue
                relative, blob = resolved
                texts.append(Text(f"{logical} -> {relative}", _sha256(blob), blob, text_kind))
                continue
        texts.append(Text(logical, _sha256(data), data, text_kind))
    status = TEXT_FOUND if any(text.kind == "LICENSE" for text in texts) else TEXT_NOT_FOUND
    return tuple(texts), tuple(unresolved), status, declared_file


def _rows(metadata, cargo_home: Path | None):
    if not isinstance(metadata, dict):
        raise NoticeError("cargo metadata must be a JSON object")
    packages_list = metadata.get("packages")
    resolve = metadata.get("resolve")
    if not isinstance(packages_list, list) or not isinstance(resolve, dict):
        raise NoticeError("cargo metadata lacks the packages or resolve graph")
    packages: dict[str, dict] = {}
    workspaces = []
    for package in packages_list:
        if not isinstance(package, dict) or not isinstance(package.get("id"), str):
            raise NoticeError("cargo metadata contains a malformed package entry")
        packages[package["id"]] = package
        if package.get("name") == "cos-desktop" and package.get("source") is None:
            workspaces.append(package)
    if len(workspaces) != 1:
        raise NoticeError("cargo metadata must identify exactly one cos-desktop workspace package")
    product = workspaces[0]
    nodes = resolve.get("nodes")
    if not isinstance(nodes, list):
        raise NoticeError("cargo metadata resolve graph has no node list")
    edges: dict[str, list] = {}
    for node in nodes:
        if not isinstance(node, dict) or not isinstance(node.get("id"), str):
            raise NoticeError("cargo metadata resolve graph contains a malformed node")
        deps = node.get("deps") or []
        if not isinstance(deps, list):
            raise NoticeError("cargo metadata resolve graph contains malformed dependencies")
        edges[node["id"]] = deps
    reached: set[str] = set()
    kinds_by_id: dict[str, set[str]] = {}
    stack = [product["id"]]
    while stack:
        current = stack.pop()
        if current in reached:
            continue
        reached.add(current)
        for dep in edges.get(current, []):
            if not isinstance(dep, dict) or not isinstance(dep.get("pkg"), str):
                raise NoticeError("cargo metadata resolve edge is malformed")
            kinds = {entry.get("kind") or "normal" for entry in dep.get("dep_kinds") or []
                     if isinstance(entry, dict)} & {"normal", "build"}
            if not kinds:
                continue
            kinds_by_id.setdefault(dep["pkg"], set()).update(kinds)
            stack.append(dep["pkg"])
    workspace_root = metadata.get("workspace_root")
    if not isinstance(workspace_root, str) or not workspace_root:
        raise NoticeError("cargo metadata has no workspace root")
    workspace = Path(workspace_root)
    if cargo_home is None:
        configured = os.environ.get("CARGO_HOME")
        cargo_home = Path(configured) if configured else Path.home() / ".cargo"
    rows: list[Row] = []
    for package_id in sorted(kinds_by_id):
        package = packages.get(package_id)
        if package is None:
            raise NoticeError("cargo metadata resolve graph references an unknown package")
        if package_id == product["id"]:
            raise NoticeError("the product workspace member must not be bound as a third-party dependency")
        name, version = package.get("name"), package.get("version")
        if not isinstance(name, str) or not name or not isinstance(version, str) or not version:
            raise NoticeError("cargo metadata package identity is malformed")
        declared = package.get("license")
        if declared is not None and (not isinstance(declared, str) or not declared
                                     or len(declared) > MAX_DECLARATION_CHARS):
            raise NoticeError("cargo metadata package license declaration is unusable")
        kind, label = _source(package)
        package_root = _package_root(package)
        if kind == "workspace" and package_root is not None and not _inside(package_root, workspace):
            kind, label = "path", f"path+<local>#{name}@{version}"
        texts, unresolved, status, declared_file = _bind(package, kind, workspace, cargo_home)
        rows.append(Row(name, version, label, kind, _role(kinds_by_id[package_id]), declared,
                        declared_file, status, texts, unresolved))
    rows.sort(key=lambda row: (row.name.casefold(), row.name, row.version, row.source))
    identities = [(row.name, row.version, row.source) for row in rows]
    if len(set(identities)) != len(identities):
        raise NoticeError("duplicate dependency identity in the notice graph")
    if not reached:
        raise NoticeError("cargo metadata reachability walk produced no packages")
    if len(rows) != len(reached) - 1:
        raise NoticeError("cargo metadata reachability accounting is inconsistent")
    metadata_packages = len(packages)
    if metadata_packages < len(reached):
        raise NoticeError("cargo metadata package accounting is inconsistent")
    return rows, {"metadata_packages": metadata_packages, "reachable": len(reached),
                  "not_reachable": metadata_packages - len(reached)}


def _counts(rows: list[dict]) -> dict:
    found = sum(1 for row in rows if row["status"] == TEXT_FOUND)
    roles = {key: 0 for key in ROLE_KEYS.values()}
    for row in rows:
        roles[ROLE_KEYS[row["role"]]] += 1
    return {
        "dependencies": len(rows),
        "text_found": found,
        "text_not_found": len(rows) - found,
        "unresolved_texts": sum(len(row["unresolved"]) for row in rows),
        "roles": roles,
    }


def _render(rows: list[Row], cargo_lock_sha256: str) -> bytes:
    payloads = []
    for row in rows:
        entry = row.payload()
        for text in entry["texts"]:
            _require_logical_path(text["path"])
        for item in entry["unresolved"]:
            _require_logical_path(item["path"])
        if entry["declared_license_file"] is not None:
            _require_logical_path(entry["declared_license_file"])
        payloads.append(entry)
    lines = [_canonical(entry) for entry in payloads]
    digest = _sha256(("\n".join(lines) + "\n").encode("ascii"))
    blobs: dict[str, bytes] = {}
    for row in rows:
        for text in row.texts:
            blobs.setdefault(text.sha256, text.data)
    found = sum(1 for row in rows if row.status == TEXT_FOUND)
    header = [
        TITLE,
        f"format: {FORMAT}",
        f"scope: {SCOPE}",
        f"cargo-lock-sha256: {cargo_lock_sha256}",
        f"dependency-set-sha256: {digest}",
        f"dependencies: {len(rows)}",
        f"text-found: {found}",
        f"text-not-found: {len(rows) - found}",
        f"legal-compliance: {LEGAL_POSTURE}",
    ]
    parts = [("\n".join(header) + "\n").encode("utf-8"),
             (INDEX_BEGIN + "\n").encode("ascii")]
    parts.extend(line.encode("ascii") + b"\n" for line in lines)
    parts.append((INDEX_END + "\n").encode("ascii"))
    parts.append((TEXT_SECTION + "\n").encode("ascii"))
    for hexdigest in sorted(blobs):
        data = blobs[hexdigest]
        parts.append(f"### {hexdigest} {len(data)}\n".encode("ascii"))
        parts.append(data)
        parts.append(b"\n")
    parts.append((BUNDLE_END + "\n").encode("ascii"))
    bundle = b"".join(parts)
    if len(bundle) > MAX_BUNDLE_BYTES:
        raise NoticeError("third-party notice bundle exceeds its size bound")
    return bundle


class _Cursor:
    __slots__ = ("data", "position")

    def __init__(self, data: bytes):
        self.data = data
        self.position = 0

    def line(self) -> str:
        end = self.data.find(b"\n", self.position)
        if end < 0:
            raise NoticeError("third-party notice bundle is truncated")
        chunk = self.data[self.position:end]
        self.position = end + 1
        try:
            return chunk.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise NoticeError("third-party notice bundle header is not UTF-8") from exc

    def take(self, size: int) -> bytes:
        if size < 0 or self.position + size > len(self.data):
            raise NoticeError("third-party notice bundle is truncated")
        chunk = self.data[self.position:self.position + size]
        self.position += size
        return chunk


def _validated_row(entry) -> dict:
    if not isinstance(entry, dict) or set(entry) != ROW_KEYS:
        raise NoticeError("third-party notice dependency entry has missing or unexpected fields")
    for key in ("name", "version", "source"):
        if not isinstance(entry[key], str) or not entry[key]:
            raise NoticeError("third-party notice dependency identity is malformed")
    if entry["source_kind"] not in SOURCE_KINDS:
        raise NoticeError("third-party notice dependency source kind is unsupported")
    if entry["role"] not in ROLES:
        raise NoticeError("third-party notice dependency role is unsupported")
    if entry["status"] not in STATUSES:
        raise NoticeError("third-party notice dependency status is unsupported")
    declared = entry["declared_license"]
    if declared is not None and (not isinstance(declared, str) or not declared
                                 or len(declared) > MAX_DECLARATION_CHARS):
        raise NoticeError("third-party notice dependency license declaration is unusable")
    if entry["declared_license_file"] is not None:
        _require_logical_path(entry["declared_license_file"])
    if not isinstance(entry["texts"], list):
        raise NoticeError("third-party notice dependency texts are malformed")
    for text in entry["texts"]:
        if not isinstance(text, dict) or set(text) != TEXT_KEYS:
            raise NoticeError("third-party notice text entry has unexpected fields")
        _require_logical_path(text["path"])
        if not isinstance(text["sha256"], str) or SHA256.fullmatch(text["sha256"]) is None:
            raise NoticeError("third-party notice text digest is malformed")
        if not isinstance(text["bytes"], int) or isinstance(text["bytes"], bool) or not 0 <= text["bytes"] <= MAX_TEXT_BYTES:
            raise NoticeError("third-party notice text length is malformed")
        if text["kind"] not in TEXT_KINDS:
            raise NoticeError("third-party notice text kind is unsupported")
    if not isinstance(entry["unresolved"], list):
        raise NoticeError("third-party notice unresolved material is malformed")
    for item in entry["unresolved"]:
        if not isinstance(item, dict) or set(item) != UNRESOLVED_KEYS:
            raise NoticeError("third-party notice unresolved entry has unexpected fields")
        _require_logical_path(item["path"])
        if item["reason"] not in UNRESOLVED_REASONS:
            raise NoticeError("third-party notice unresolved reason is unsupported")
    return dict(entry)


def parse_bundle(data: bytes) -> ParsedBundle:
    if not isinstance(data, (bytes, bytearray)):
        raise NoticeError("third-party notice bundle must be bytes")
    data = bytes(data)
    if not data or len(data) > MAX_BUNDLE_BYTES:
        raise NoticeError("third-party notice bundle is empty or exceeds its size bound")
    cursor = _Cursor(data)
    if cursor.line() != TITLE:
        raise NoticeError("third-party notice bundle has an unexpected title")
    header: dict[str, str] = {}
    order: list[str] = []
    while True:
        line = cursor.line()
        if line == INDEX_BEGIN:
            break
        key, separator, value = line.partition(": ")
        if not separator or key in header:
            raise NoticeError("third-party notice bundle header is malformed")
        order.append(key)
        header[key] = value
    if tuple(order) != HEADER_KEYS:
        raise NoticeError("third-party notice bundle header is missing or reorders required fields")
    rows = []
    while True:
        line = cursor.line()
        if line == INDEX_END:
            break
        try:
            entry = json.loads(line)
        except json.JSONDecodeError as exc:
            raise NoticeError("third-party notice dependency index is not valid JSON") from exc
        rows.append(_validated_row(entry))
    if not rows:
        raise NoticeError("third-party notice bundle has an empty dependency index")
    if cursor.line() != TEXT_SECTION:
        raise NoticeError("third-party notice bundle has no license text section")
    blobs: dict[str, bytes] = {}
    while True:
        line = cursor.line()
        if line == BUNDLE_END:
            break
        match = re.fullmatch(r"### ([0-9a-f]{64}) (\d+)", line)
        if match is None:
            raise NoticeError("third-party notice bundle has a malformed text header")
        hexdigest, size = match.group(1), int(match.group(2))
        if size > MAX_TEXT_BYTES:
            raise NoticeError("third-party notice text exceeds its size bound")
        blob = cursor.take(size)
        if cursor.take(1) != b"\n":
            raise NoticeError("third-party notice text length does not match its header")
        if _sha256(blob) != hexdigest:
            raise NoticeError("third-party notice text does not match its digest")
        if hexdigest in blobs:
            raise NoticeError("third-party notice bundle repeats a text block")
        blobs[hexdigest] = blob
    if cursor.position != len(data):
        raise NoticeError("third-party notice bundle has trailing data")
    digest = _sha256(("\n".join(_canonical(entry) for entry in rows) + "\n").encode("ascii"))
    if header["dependency-set-sha256"] != digest:
        raise NoticeError("third-party notice dependency-set digest does not match the index")
    counts = _counts(rows)
    if (header["dependencies"] != str(counts["dependencies"])
            or header["text-found"] != str(counts["text_found"])
            or header["text-not-found"] != str(counts["text_not_found"])):
        raise NoticeError("third-party notice header counts disagree with the index")
    if (header["format"] != FORMAT or header["scope"] != SCOPE
            or header["legal-compliance"] != LEGAL_POSTURE):
        raise NoticeError("third-party notice bundle has an unsupported format, scope or legal posture")
    if SHA256.fullmatch(header["cargo-lock-sha256"]) is None:
        raise NoticeError("third-party notice bundle Cargo.lock digest is malformed")
    referenced: set[str] = set()
    identities: set[tuple] = set()
    for row in rows:
        identity = (row["name"], row["version"], row["source"])
        if identity in identities:
            raise NoticeError("third-party notice bundle repeats a dependency identity")
        identities.add(identity)
        for text in row["texts"]:
            if text["sha256"] not in blobs:
                raise NoticeError("third-party notice bundle references a missing text block")
            referenced.add(text["sha256"])
        if row["status"] == TEXT_FOUND and not any(text["kind"] == "LICENSE" for text in row["texts"]):
            raise NoticeError("third-party notice bundle marks an unbound dependency as found")
        if row["status"] == TEXT_NOT_FOUND and any(text["kind"] == "LICENSE" for text in row["texts"]):
            raise NoticeError("third-party notice bundle marks a bound dependency as unbound")
    for hexdigest in blobs:
        if hexdigest not in referenced:
            raise NoticeError("third-party notice bundle contains an unreferenced text block")
    return ParsedBundle(header["cargo-lock-sha256"], digest, tuple(rows), blobs, counts)


def _summary(parsed: ParsedBundle, data: bytes, stats: dict) -> dict:
    return {
        "format": FORMAT,
        "payload_path": PAYLOAD_NAME,
        "scope": SCOPE,
        "cargo_lock_sha256": parsed.cargo_lock_sha256,
        "dependency_set_sha256": parsed.dependency_set_sha256,
        "bundle_sha256": _sha256(data),
        "bundle_bytes": len(data),
        "dependencies": parsed.counts["dependencies"],
        "text_found": parsed.counts["text_found"],
        "text_not_found": parsed.counts["text_not_found"],
        "unresolved_texts": parsed.counts["unresolved_texts"],
        "roles": dict(parsed.counts["roles"]),
        "metadata_packages": stats["metadata_packages"],
        "reachable": stats["reachable"],
        "not_reachable": stats["not_reachable"],
        "legal_compliance": LEGAL_POSTURE,
    }


def build(metadata, cargo_lock_sha256: str, cargo_home=None) -> tuple[bytes, dict]:
    """Render the bundle from local metadata/texts and return (bytes, claim)."""
    if not isinstance(cargo_lock_sha256, str) or SHA256.fullmatch(cargo_lock_sha256) is None:
        raise NoticeError("Cargo.lock digest must be 64 lowercase hexadecimal characters")
    rows, stats = _rows(metadata, Path(cargo_home) if cargo_home is not None else None)
    data = _render(rows, cargo_lock_sha256)
    parsed = parse_bundle(data)
    return data, _summary(parsed, data, stats)


def check_manifest(claim, data: bytes) -> ParsedBundle:
    """Fail closed unless the staged manifest claim matches the staged bundle."""
    parsed = parse_bundle(data)
    if not isinstance(claim, dict) or set(claim) != MANIFEST_KEYS:
        raise NoticeError("third_party_notices claim has missing or unexpected fields")
    for key, expected in (("format", FORMAT), ("payload_path", PAYLOAD_NAME),
                          ("scope", SCOPE), ("legal_compliance", LEGAL_POSTURE)):
        if claim[key] != expected:
            raise NoticeError(f"third_party_notices claim has an invalid {key}")
    if (not isinstance(claim["bundle_sha256"], str) or claim["bundle_sha256"] != _sha256(data)
            or claim["bundle_bytes"] != len(data)):
        raise NoticeError("third-party notice bundle does not match the claimed bytes")
    if claim["cargo_lock_sha256"] != parsed.cargo_lock_sha256:
        raise NoticeError("third-party notice bundle was built from a different Cargo.lock")
    if claim["dependency_set_sha256"] != parsed.dependency_set_sha256:
        raise NoticeError("third-party notice bundle covers a different dependency set")
    for key in ("dependencies", "text_found", "text_not_found", "unresolved_texts",
                "bundle_bytes", "metadata_packages", "reachable", "not_reachable"):
        value = claim[key]
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            raise NoticeError(f"third-party notice claim field is malformed: {key}")
    for key in ("dependencies", "text_found", "text_not_found", "unresolved_texts"):
        if claim[key] != parsed.counts[key]:
            raise NoticeError(f"third-party notice counts disagree with the bundle: {key}")
    roles = claim["roles"]
    if not isinstance(roles, dict) or set(roles) != {"normal_only", "build_only", "normal_build"}:
        raise NoticeError("third-party notice role counts are malformed")
    for key, value in roles.items():
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            raise NoticeError(f"third-party notice role count is malformed: {key}")
        if value != parsed.counts["roles"][key]:
            raise NoticeError(f"third-party notice role counts disagree with the bundle: {key}")
    if claim["reachable"] < 1 or claim["dependencies"] != claim["reachable"] - 1:
        raise NoticeError("third-party notice reachability accounting is inconsistent")
    if claim["not_reachable"] != claim["metadata_packages"] - claim["reachable"]:
        raise NoticeError("third-party notice metadata accounting is inconsistent")
    return parsed
