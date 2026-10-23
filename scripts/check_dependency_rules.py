#!/usr/bin/env python3
"""Check COS Desktop workspace dependency boundaries against DEPENDENCY_RULES.md.

Input: `cargo metadata --locked --no-deps --format-version 1` (invoked directly, or passed
via --metadata). Rules enforced for every workspace member, across normal/dev/build kinds
and every target condition (metadata lists all declared edges; the host-resolved graph is
never sufficient):

1. Every workspace member has an allowlist row; unknown members fail.
2. Every direct dependency edge between workspace members is allowed by that row.
3. Renamed edges are matched by real package name, so a rename cannot hide a forbidden edge.
4. No cycles among workspace members, including dev/build/target-specific edges.
5. GPUI/Zed-source packages appear only in cos-desktop and desktop-gpui (binary/UI boundary).
6. No local path dependency outside the repository (runtime checkout C:/AI included).
7. No local path dependency inside the repository that is not a workspace member
   (submodule/vendored/copied trees).
8. Git dependencies pin a 40-hex rev and all Zed dependencies share one revision (ADR-0001 D1).
9. Only crates.io and the pinned Zed repository are reviewed external sources; any other
   source requires a deliberate checker/ADR update, not an incidental manifest edit.

Metadata cannot prove behavioral prohibitions (direct DB/CAS/config access, main-thread I/O,
imports without declared dependencies); manifest and source review still apply.

Usage (from the repository root):
    python scripts/check_dependency_rules.py                 # runs cargo metadata itself
    python scripts/check_dependency_rules.py --metadata m.json
    python scripts/check_dependency_rules.py --self-test     # regression cases

Exit codes: 0 clean, 1 dependency-rule violations, 2 usage/input error.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import subprocess
import sys
import unittest
import urllib.parse
from dataclasses import dataclass

# DEPENDENCY_RULES.md "Direct workspace dependency allowlist". Rows are dependents; cells are
# the only allowed direct workspace dependencies. MP-IMG1 introduces media with
# real local-input responsibilities; platform remains absent until its own task.
WORKSPACE_ALLOWLIST = {
    "cos-desktop": {"desktop-gpui", "ui-model", "app-core", "cos-client", "desktop-protocol", "media"},
    "desktop-gpui": {"ui-model", "app-core", "desktop-protocol", "media"},
    "ui-model": {"app-core", "desktop-protocol"},
    "app-core": {"cos-client", "desktop-protocol", "media"},
    "cos-client": {"desktop-protocol"},
    "desktop-protocol": set(),
    "media": {"desktop-protocol"},
}

GPUI_PACKAGES = ("gpui", "gpui_platform", "gpui_windows")
GPUI_ALLOWED_DEPENDENTS = {"cos-desktop", "desktop-gpui"}  # Binary and GPUI presentation boundary
ZED_GIT_URL = "https://github.com/zed-industries/zed"
CRATES_IO_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
REV_RE = re.compile(r"^[0-9a-f]{40}$")
RUNTIME_CHECKOUT_RE = re.compile(r"(^|/)c:/ai(/|$)")
WINDOWS_ABSOLUTE_RE = re.compile(r"^[a-zA-Z]:[/\\]")


@dataclass(frozen=True)
class Violation:
    code: str
    message: str


def workspace_members(metadata: dict) -> list[dict]:
    member_ids = set(metadata.get("workspace_members") or [])
    return [pkg for pkg in metadata.get("packages") or [] if pkg.get("id") in member_ids]


def split_source(source: str | None) -> tuple[str | None, str]:
    if not source:
        return None, ""
    scheme, _, rest = source.partition("+")
    return scheme, rest


def git_details(source: str | None) -> tuple[str | None, str | None, str | None]:
    """Return (url, rev, floating_ref) for a git source id."""
    scheme, rest = split_source(source)
    if scheme != "git":
        return None, None, None
    parsed = urllib.parse.urlsplit(rest)
    url = urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, parsed.path, "", ""))
    query = urllib.parse.parse_qs(parsed.query)
    rev = (query.get("rev") or [None])[0]
    floating = (query.get("branch") or [None])[0] or (query.get("tag") or [None])[0]
    return url, rev, floating


def is_gpui_dependency(name: str, source: str | None) -> bool:
    if name in GPUI_PACKAGES or name.startswith("gpui_"):
        return True
    url, _, _ = git_details(source)
    return url == ZED_GIT_URL


def is_absolute_dep_path(path: str) -> bool:
    return os.path.isabs(path) or bool(WINDOWS_ABSOLUTE_RE.match(path))


def resolve_dep_path(manifest_path: str, dep_path: str | None) -> str | None:
    if not dep_path:
        return None
    if is_absolute_dep_path(dep_path):
        return os.path.normpath(dep_path)
    return os.path.normpath(os.path.join(os.path.dirname(manifest_path), dep_path))


def is_within(path: str, root: str) -> bool:
    candidate = os.path.normcase(os.path.abspath(path))
    base = os.path.normcase(os.path.abspath(root))
    return candidate == base or candidate.startswith(base + os.sep)


def is_runtime_checkout(path: str) -> bool:
    normalized = os.path.abspath(path).replace("\\", "/").lower()
    return bool(RUNTIME_CHECKOUT_RE.search(normalized))


def edge_label(kind: str, target: str | None) -> str:
    return f"{kind} target={target}" if target else kind


def describe_edge(dependent: str, name: str, kind: str, target: str | None, rename: str | None) -> str:
    description = f"{dependent} -> {name} [{edge_label(kind, target)}]"
    if rename:
        description += f" [renamed as '{rename}']"
    return description


def find_cycles(adjacency: dict[str, dict[str, set[str]]]) -> list[list[tuple[str, str, str]]]:
    """All cycles (deduplicated by edge set) as lists of (from, to, label) edges."""
    cycles: list[list[tuple[str, str, str]]] = []
    seen: set[frozenset[tuple[str, str, str]]] = set()
    WHITE, GRAY, BLACK = 0, 1, 2
    color = {name: WHITE for name in adjacency}
    for start in sorted(adjacency):
        if color[start] != WHITE:
            continue
        color[start] = GRAY
        path = [start]
        stack = [(start, iter(sorted(adjacency[start].items())))]
        while stack:
            node, iterator = stack[-1]
            advanced = False
            for nxt, labels in iterator:
                if nxt not in adjacency:
                    continue
                if color.get(nxt, WHITE) == WHITE:
                    color[nxt] = GRAY
                    path.append(nxt)
                    stack.append((nxt, iter(sorted(adjacency[nxt].items()))))
                    advanced = True
                    break
                if color.get(nxt) == GRAY:
                    start_index = path.index(nxt)
                    nodes = path[start_index:] + [nxt]
                    edges = [
                        (a, b, "+".join(sorted(adjacency[a][b])))
                        for a, b in zip(nodes, nodes[1:])
                    ]
                    key = frozenset(edges)
                    if key not in seen:
                        seen.add(key)
                        cycles.append(edges)
            if not advanced:
                color[node] = BLACK
                stack.pop()
                path.pop()
    return cycles


def check_metadata(metadata: dict, repo_root: str) -> list[Violation]:
    violations: list[Violation] = []
    members = workspace_members(metadata)
    if not members:
        return [Violation("no-members", "cargo metadata reports no workspace members")]

    member_names = {pkg["name"] for pkg in members}
    adjacency: dict[str, dict[str, set[str]]] = {}
    zed_revisions: dict[str, list[str]] = {}

    for pkg in members:
        dependent = pkg["name"]
        manifest_path = pkg.get("manifest_path", "")
        if not is_within(manifest_path, repo_root):
            violations.append(
                Violation(
                    "external-member",
                    f"workspace member '{dependent}' is outside the repository root: {manifest_path}",
                )
            )
        if dependent not in WORKSPACE_ALLOWLIST:
            violations.append(
                Violation(
                    "unknown-package",
                    f"workspace member '{dependent}' has no allowlist row; update "
                    "docs/architecture/DEPENDENCY_RULES.md and scripts/check_dependency_rules.py together",
                )
            )

        for dep in pkg.get("dependencies") or []:
            name = dep.get("name", "")
            kind = dep.get("kind") or "normal"
            target = dep.get("target")
            rename = dep.get("rename")
            source = dep.get("source")
            where = describe_edge(dependent, name, kind, target, rename)

            if source and name in member_names:
                violations.append(
                    Violation(
                        "shadowed-name",
                        f"{where} resolves from an external source while a workspace member "
                        f"uses the name '{name}'; declare the member with a path to disambiguate",
                    )
                )
                continue

            if name in member_names:
                allowed = WORKSPACE_ALLOWLIST.get(dependent)
                if allowed is not None and name not in allowed:
                    violations.append(
                        Violation(
                            "forbidden-edge",
                            f"forbidden direct dependency: {where}; allowed workspace dependencies "
                            f"for {dependent}: {sorted(allowed) if allowed else 'none'}",
                        )
                    )
                adjacency.setdefault(dependent, {}).setdefault(name, set()).add(edge_label(kind, target))
                continue

            if is_gpui_dependency(name, source) and dependent not in GPUI_ALLOWED_DEPENDENTS:
                violations.append(
                    Violation(
                        "gpui-isolation",
                        f"GPUI/Zed dependency '{name}' declared by {dependent}; only "
                        f"{sorted(GPUI_ALLOWED_DEPENDENTS)} may depend on GPUI",
                    )
                )

            scheme, _ = split_source(source)
            if scheme is None:
                resolved = resolve_dep_path(manifest_path, dep.get("path"))
                if resolved is None:
                    violations.append(
                        Violation(
                            "local-source",
                            f"local dependency '{name}' declared by {dependent} has no path and is not "
                            "a workspace member",
                        )
                    )
                elif not is_within(resolved, repo_root):
                    hint = "; the runtime checkout C:/AI is never a dependency" if is_runtime_checkout(resolved) else ""
                    violations.append(
                        Violation(
                            "external-path",
                            f"local path dependency '{name}' resolves outside the repository: {resolved}{hint}",
                        )
                    )
                else:
                    violations.append(
                        Violation(
                            "non-member-path",
                            f"local path dependency '{name}' resolves inside the repository but is not a "
                            f"workspace member: {resolved}",
                        )
                    )
            elif scheme == "registry":
                if source != CRATES_IO_SOURCE:
                    violations.append(
                        Violation(
                            "external-source",
                            f"unreviewed registry source '{source}' for dependency '{name}' declared by "
                            f"{dependent}; only crates.io is reviewed",
                        )
                    )
            elif scheme == "git":
                url, rev, floating = git_details(source)
                if url != ZED_GIT_URL:
                    violations.append(
                        Violation(
                            "external-source",
                            f"unreviewed git source '{source}' for dependency '{name}' declared by {dependent}; "
                            "reviewed sources live in scripts/check_dependency_rules.py and ADR-0001",
                        )
                    )
                if floating or not rev or not REV_RE.match(rev):
                    violations.append(
                        Violation(
                            "git-pin",
                            f"git dependency '{name}' declared by {dependent} must pin a 40-hex rev "
                            f"(branch/tag/floating refs are forbidden): {source}",
                        )
                    )
                elif url == ZED_GIT_URL:
                    zed_revisions.setdefault(rev, []).append(where)
            else:
                violations.append(
                    Violation(
                        "external-source",
                        f"unreviewed source '{source}' for dependency '{name}' declared by {dependent}",
                    )
                )

    if len(zed_revisions) > 1:
        detail = "; ".join(
            f"{rev}: {', '.join(sorted(edges))}" for rev, edges in sorted(zed_revisions.items())
        )
        violations.append(
            Violation(
                "zed-revision-mismatch",
                f"Zed git dependencies must share one revision (ADR-0001 D1): {detail}",
            )
        )

    for cycle in find_cycles(adjacency):
        rendered = " -> ".join(f"{a} -[{label}]-> {b}" for a, b, label in cycle)
        violations.append(Violation("cycle", f"dependency cycle among workspace members: {rendered}"))

    return sorted(violations, key=lambda violation: (violation.code, violation.message))


def run_cargo_metadata(repo_root: str) -> dict:
    command = ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"]
    completed = subprocess.run(
        command,
        cwd=repo_root,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if completed.returncode != 0:
        sys.stderr.write(completed.stderr)
        raise RuntimeError(f"cargo metadata failed (exit {completed.returncode}): {' '.join(command)}")
    return json.loads(completed.stdout)


def default_repo_root() -> str:
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def run_self_test() -> int:
    sys.dont_write_bytecode = True
    tests_file = os.path.join(os.path.dirname(os.path.abspath(__file__)), "tests", "test_check_dependency_rules.py")
    if not os.path.exists(tests_file):
        print(f"self-test file not found: {tests_file}", file=sys.stderr)
        return 2
    sys.path.insert(0, os.path.dirname(tests_file))
    spec = importlib.util.spec_from_file_location("test_check_dependency_rules", tests_file)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    suite = unittest.defaultTestLoader.loadTestsFromModule(module)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Check COS Desktop dependency boundaries against docs/architecture/DEPENDENCY_RULES.md."
    )
    parser.add_argument("--metadata", metavar="FILE", help="read this cargo-metadata JSON instead of invoking cargo")
    parser.add_argument("--root", metavar="DIR", help="repository root for path checks (default: metadata workspace_root)")
    parser.add_argument("--self-test", action="store_true", help="run the regression cases in scripts/tests and exit")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.self_test:
        return run_self_test()
    try:
        if args.metadata:
            with open(args.metadata, "r", encoding="utf-8") as handle:
                metadata = json.load(handle)
            repo_root = os.path.abspath(args.root or metadata.get("workspace_root") or default_repo_root())
        else:
            repo_root = os.path.abspath(args.root or default_repo_root())
            metadata = run_cargo_metadata(repo_root)
    except (OSError, json.JSONDecodeError, RuntimeError) as error:
        print(f"dependency rules check: input error: {error}", file=sys.stderr)
        return 2

    violations = check_metadata(metadata, repo_root)
    if violations:
        for violation in violations:
            print(f"ERROR [{violation.code}] {violation.message}", file=sys.stderr)
        print(f"dependency rules check FAILED: {len(violations)} violation(s)", file=sys.stderr)
        return 1
    count = len(workspace_members(metadata))
    print(f"dependency rules check OK: allowlist edges verified for {count} workspace member(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
