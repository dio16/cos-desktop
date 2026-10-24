"""Regression cases for scripts/check_dependency_rules.py.

Executable negative/positive cases requested by
docs/architecture/DEPENDENCY_RULES.md and docs/development/TESTING.md section 4:
a renamed forbidden cos-client dependency from desktop-gpui, a target-specific
forbidden edge, a dev-edge cycle, and an accepted direct DTO dependency, plus the
path/GPUI/pinning rules the checker enforces beyond metadata.

Run directly or through the checker:
    python scripts/tests/test_check_dependency_rules.py
    python scripts/check_dependency_rules.py --self-test
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIR))

import check_dependency_rules as cdr  # noqa: E402

ZED_REV = "395def7a067b32ed598fcb3f048969ada9b54942"
OTHER_REV = "a" * 40
LAYOUT = {
    "cos-desktop": "apps/desktop",
    "desktop-gpui": "crates/desktop-gpui",
    "ui-model": "crates/ui-model",
    "app-core": "crates/app-core",
    "cos-client": "crates/cos-client",
    "desktop-protocol": "crates/desktop-protocol",
    "media": "crates/media",
}


def dep(name, kind=None, target=None, path=None, rename=None, source=None, req="*"):
    return {
        "name": name,
        "source": source,
        "req": req,
        "kind": kind,
        "rename": rename,
        "optional": False,
        "uses_default_features": True,
        "features": [],
        "target": target,
        "registry": None,
        "path": path,
    }


def registry_dep(name):
    return dep(name, source=cdr.CRATES_IO_SOURCE, req="^1")


def git_dep(name, rev=ZED_REV, branch=None, url=cdr.ZED_GIT_URL):
    query = f"branch={branch}" if branch else f"rev={rev}"
    return dep(name, source=f"git+{url}?{query}", req="*")


class DependencyRulesTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory(prefix="cos-desktop-rules-")
        self.root = Path(self._tmp.name).resolve()

    def tearDown(self):
        self._tmp.cleanup()

    def member_dir(self, name):
        return self.root / LAYOUT.get(name, f"crates/{name}")

    def member(self, name, deps=()):
        return {
            "name": name,
            "id": f"path+file:///{name}#{name}@0.1.0",
            "source": None,
            "manifest_path": str(self.member_dir(name) / "Cargo.toml"),
            "dependencies": list(deps),
        }

    def member_dep(self, dependent, dependency, **kwargs):
        return dep(dependency, path=self.path_to_member(dependent, dependency), **kwargs)

    def path_to_member(self, dependent, dependency):
        relative = os.path.relpath(self.member_dir(dependency), self.member_dir(dependent))
        return relative.replace(os.sep, "/")

    def workspace(self, packages):
        return {
            "version": 1,
            "workspace_root": str(self.root),
            "workspace_members": [package["id"] for package in packages],
            "workspace_default_members": [package["id"] for package in packages],
            "target_directory": str(self.root / "target"),
            "packages": packages,
            "resolve": None,
        }

    def clean_workspace(self):
        """Documented allowlist edges, including accepted direct desktop-protocol imports."""
        packages = [
            self.member(
                "cos-desktop",
                [
                    self.member_dep("cos-desktop", "desktop-gpui"),
                    self.member_dep("cos-desktop", "ui-model"),
                    self.member_dep("cos-desktop", "app-core"),
                    self.member_dep("cos-desktop", "cos-client"),
                    self.member_dep("cos-desktop", "desktop-protocol"),
                    self.member_dep("cos-desktop", "media", kind="dev"),
                    git_dep("gpui"),
                    git_dep("gpui_platform"),
                ],
            ),
            self.member(
                "desktop-gpui",
                [
                    self.member_dep("desktop-gpui", "ui-model"),
                    self.member_dep("desktop-gpui", "app-core"),
                    self.member_dep("desktop-gpui", "desktop-protocol"),
                ],
            ),
            self.member(
                "ui-model",
                [self.member_dep("ui-model", "app-core"), self.member_dep("ui-model", "desktop-protocol")],
            ),
            self.member(
                "app-core",
                [self.member_dep("app-core", "cos-client"), self.member_dep("app-core", "desktop-protocol")],
            ),
            self.member(
                "cos-client",
                [self.member_dep("cos-client", "desktop-protocol"), registry_dep("serde")],
            ),
            self.member("desktop-protocol"),
            self.member("media", [self.member_dep("media", "desktop-protocol"), registry_dep("sha2")]),
        ]
        return self.workspace(packages)

    def package(self, metadata, name):
        return next(package for package in metadata["packages"] if package["name"] == name)

    def codes(self, violations):
        return sorted({violation.code for violation in violations})

    def replace_dependency(self, metadata, package_name, dependency_name, replacement):
        dependencies = self.package(metadata, package_name)["dependencies"]
        index = next(i for i, dependency in enumerate(dependencies) if dependency["name"] == dependency_name)
        dependencies[index] = replacement

    def test_clean_workspace_passes(self):
        self.assertEqual([], cdr.check_metadata(self.clean_workspace(), str(self.root)))

    def test_accepted_direct_dto_dependency_passes(self):
        metadata = self.workspace(
            [
                self.member("desktop-protocol"),
                self.member("desktop-gpui", [self.member_dep("desktop-gpui", "desktop-protocol")]),
                self.member("ui-model", [self.member_dep("ui-model", "desktop-protocol")]),
            ]
        )
        self.assertEqual([], cdr.check_metadata(metadata, str(self.root)))

    def test_renamed_forbidden_cos_client_edge_detected(self):
        metadata = self.clean_workspace()
        self.package(metadata, "desktop-gpui")["dependencies"].append(
            dep("cos-client", path=self.path_to_member("desktop-gpui", "cos-client"), rename="client")
        )
        violations = cdr.check_metadata(metadata, str(self.root))
        self.assertIn("forbidden-edge", self.codes(violations))
        message = next(v.message for v in violations if v.code == "forbidden-edge")
        self.assertIn("cos-client", message)
        self.assertIn("renamed as 'client'", message)

    def test_target_specific_forbidden_edge_detected(self):
        metadata = self.clean_workspace()
        self.package(metadata, "app-core")["dependencies"].append(
            self.member_dep("app-core", "ui-model", target='cfg(windows)')
        )
        violations = cdr.check_metadata(metadata, str(self.root))
        forbidden = [v for v in violations if v.code == "forbidden-edge"]
        self.assertTrue(any('cfg(windows)' in v.message for v in forbidden), violations)

    def test_dev_and_build_edge_cycles_detected(self):
        for kind in ("dev", "build"):
            with self.subTest(kind=kind):
                metadata = self.clean_workspace()
                self.package(metadata, "app-core")["dependencies"].append(
                    self.member_dep("app-core", "desktop-gpui", kind=kind)
                )
                violations = cdr.check_metadata(metadata, str(self.root))
                cycles = [v for v in violations if v.code == "cycle"]
                self.assertTrue(cycles, violations)
                self.assertTrue(any("desktop-gpui" in v.message and "app-core" in v.message for v in cycles))
                self.assertTrue(any(kind in v.message for v in cycles), cycles)

    def test_external_path_dependency_detected(self):
        metadata = self.clean_workspace()
        self.package(metadata, "app-core")["dependencies"].append(
            dep("contract_core", path="C:/AI/orchestrator/rust/crates/contract_core")
        )
        violations = cdr.check_metadata(metadata, str(self.root))
        external = [v for v in violations if v.code == "external-path"]
        self.assertTrue(external, violations)
        self.assertTrue(any("C:/AI" in v.message for v in external), external)

    def test_repo_internal_non_member_path_detected(self):
        metadata = self.clean_workspace()
        self.package(metadata, "app-core")["dependencies"].append(
            dep("contract_core", path=str(self.root / "vendor" / "copied-runtime"))
        )
        violations = cdr.check_metadata(metadata, str(self.root))
        self.assertIn("non-member-path", self.codes(violations))

    def test_gpui_outside_binary_and_ui_detected(self):
        metadata = self.clean_workspace()
        self.package(metadata, "cos-client")["dependencies"].append(git_dep("gpui"))
        violations = cdr.check_metadata(metadata, str(self.root))
        self.assertIn("gpui-isolation", self.codes(violations))

    def test_unknown_workspace_member_detected(self):
        metadata = self.clean_workspace()
        metadata["packages"].append(self.member("unapproved-layer"))
        metadata["workspace_members"].append(metadata["packages"][-1]["id"])
        violations = cdr.check_metadata(metadata, str(self.root))
        unknown = [v for v in violations if v.code == "unknown-package"]
        self.assertTrue(unknown, violations)
        self.assertIn("DEPENDENCY_RULES.md", unknown[0].message)

    def test_media_cannot_import_client_core_or_gpui(self):
        for dependency in ("cos-client", "app-core", "desktop-gpui"):
            with self.subTest(dependency=dependency):
                metadata = self.clean_workspace()
                self.package(metadata, "media")["dependencies"].append(
                    self.member_dep("media", dependency)
                )
                violations = cdr.check_metadata(metadata, str(self.root))
                self.assertIn("forbidden-edge", self.codes(violations))

    def test_floating_git_ref_detected(self):
        metadata = self.clean_workspace()
        self.replace_dependency(metadata, "cos-desktop", "gpui", git_dep("gpui", branch="main"))
        violations = cdr.check_metadata(metadata, str(self.root))
        self.assertIn("git-pin", self.codes(violations))

    def test_unreviewed_external_source_detected(self):
        metadata = self.clean_workspace()
        dependencies = self.package(metadata, "desktop-gpui")["dependencies"]
        dependencies.append(git_dep("other_gui", url="https://github.com/example/other-gui"))
        dependencies.append(dep("elsewhere", source="registry+https://example.com/alternate-index"))
        violations = cdr.check_metadata(metadata, str(self.root))
        external = [v for v in violations if v.code == "external-source"]
        self.assertEqual(2, len(external), violations)

    def test_zed_revision_mismatch_detected(self):
        metadata = self.clean_workspace()
        self.replace_dependency(metadata, "cos-desktop", "gpui_platform", git_dep("gpui_platform", rev=OTHER_REV))
        violations = cdr.check_metadata(metadata, str(self.root))
        self.assertIn("zed-revision-mismatch", self.codes(violations))

    def test_cli_reports_violations_and_exit_codes(self):
        script = str(SCRIPTS_DIR / "check_dependency_rules.py")

        clean_file = self.root / "clean.json"
        clean_file.write_text(json.dumps(self.clean_workspace()), encoding="utf-8")
        clean = subprocess.run(
            [sys.executable, "-B", script, "--metadata", str(clean_file)],
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        self.assertEqual(0, clean.returncode, clean.stderr)
        self.assertIn("OK", clean.stdout)

        failing = self.clean_workspace()
        self.package(failing, "desktop-gpui")["dependencies"].append(
            self.member_dep("desktop-gpui", "cos-client")
        )
        failing_file = self.root / "failing.json"
        failing_file.write_text(json.dumps(failing), encoding="utf-8")
        failed = subprocess.run(
            [sys.executable, "-B", script, "--metadata", str(failing_file)],
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        self.assertEqual(1, failed.returncode)
        self.assertIn("forbidden-edge", failed.stderr)


class CargoResolutionTests(unittest.TestCase):
    """The cargo program resolution rule: CARGO when set (cargo's convention), otherwise "cargo".

    Every case is hermetic: it asserts on the built argv or on the closed launch-failure path, so it
    never depends on this machine's PATH and never needs a real cargo executable.
    """

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory(prefix="cos-desktop-cargo-")
        self.root = Path(self._tmp.name).resolve()
        self._original_cargo = os.environ.get("CARGO")

    def tearDown(self):
        self.set_cargo(self._original_cargo)
        self._tmp.cleanup()

    @staticmethod
    def set_cargo(value):
        if value is None:
            os.environ.pop("CARGO", None)
        else:
            os.environ["CARGO"] = value

    def metadata(self, packages):
        return {
            "version": 1,
            "workspace_root": str(self.root),
            "workspace_members": [package["id"] for package in packages],
            "workspace_default_members": [package["id"] for package in packages],
            "target_directory": str(self.root / "target"),
            "packages": packages,
            "resolve": None,
        }

    def member(self, name):
        return {
            "name": name,
            "id": f"path+file:///{name}#{name}@0.1.0",
            "source": None,
            "manifest_path": str(self.root / "crates" / name / "Cargo.toml"),
            "dependencies": [],
        }

    def test_cargo_program_comes_from_environment(self):
        # sys.executable is an existing executable on every machine, so the argv assertion needs
        # neither PATH nor a real cargo, and nothing is launched.
        self.set_cargo(sys.executable)
        self.assertEqual(sys.executable, cdr._cargo_program())
        self.assertEqual(
            [sys.executable, "metadata", "--locked", "--no-deps", "--format-version", "1"],
            cdr._cargo_metadata_command(),
        )

    def test_cargo_program_falls_back_to_bare_cargo(self):
        self.set_cargo(None)
        self.assertEqual("cargo", cdr._cargo_program())
        self.assertEqual(
            ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
            cdr._cargo_metadata_command(),
        )
        # An empty value is not a usable program name, so it falls back as well.
        self.set_cargo("")
        self.assertEqual("cargo", cdr._cargo_program())

    def test_unlaunchable_cargo_is_a_closed_input_error(self):
        missing = self.root / "no-such-cargo.exe"
        self.set_cargo(str(missing))
        with self.assertRaises(RuntimeError) as raised:
            cdr.run_cargo_metadata(str(self.root))
        self.assertIn(str(missing), str(raised.exception))
        self.assertIn("CARGO", str(raised.exception))

        # The closed CLI path: exit 2 with the resolved program named, never a crash or a result.
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            code = cdr.main([])
        self.assertEqual(2, code)
        self.assertIn("input error", stderr.getvalue())
        self.assertIn(str(missing), stderr.getvalue())

    def test_resolution_does_not_change_rule_outcomes_or_exit_codes(self):
        # --metadata bypasses the cargo invocation, so a CARGO value must never change rule
        # outcomes, the clean exit code (0) or the violation exit code (1).
        self.set_cargo(str(self.root / "no-such-cargo.exe"))
        clean = self.metadata([self.member("desktop-protocol")])
        failing = self.metadata([self.member("unapproved-layer")])
        self.assertEqual([], cdr.check_metadata(clean, str(self.root)))

        clean_file = self.root / "clean.json"
        clean_file.write_text(json.dumps(clean), encoding="utf-8")
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            self.assertEqual(0, cdr.main(["--metadata", str(clean_file)]))
        self.assertIn("OK", stdout.getvalue())

        failing_file = self.root / "unapproved.json"
        failing_file.write_text(json.dumps(failing), encoding="utf-8")
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            self.assertEqual(1, cdr.main(["--metadata", str(failing_file)]))
        self.assertIn("unknown-package", stderr.getvalue())


if __name__ == "__main__":
    unittest.main(verbosity=2)
