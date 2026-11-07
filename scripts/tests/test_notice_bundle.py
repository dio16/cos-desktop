"""Hermetic cases for the deterministic, offline third-party notice bundle."""
from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "notice_bundle.py"
sys.path.insert(0, str(SCRIPT.parent))
import notice_bundle

LOCK = "a" * 64
GIT_REV = "0" * 40


class Graph:
    """A synthetic Cargo workspace/cargo-home tree with declared texts."""

    def __init__(self, root: Path):
        self.root = Path(root)
        self.packages = []
        self.edges = {}
        self.root_id = "path+file://<workspace>#cos-desktop@0.1.0"
        root_manifest = self.root / "apps" / "desktop" / "Cargo.toml"
        root_manifest.parent.mkdir(parents=True, exist_ok=True)
        root_manifest.write_bytes(b"[package]\nname = \"cos-desktop\"\n")
        self.packages.append({"id": self.root_id, "name": "cos-desktop", "version": "0.1.0",
                              "source": None, "manifest_path": str(root_manifest),
                              "license": "Apache-2.0", "license_file": None})
        self.edges[self.root_id] = []

    def _add(self, package_id, name, version, source, directory, license, license_file, kind):
        directory = Path(directory)
        manifest = directory / "Cargo.toml"
        manifest.parent.mkdir(parents=True, exist_ok=True)
        manifest.write_bytes(b"[package]\n")
        self.packages.append({"id": package_id, "name": name, "version": version, "source": source,
                              "manifest_path": str(manifest), "license": license,
                              "license_file": license_file})
        self.edges.setdefault(package_id, [])
        self.edges[self.root_id].append({"pkg": package_id,
                                         "dep_kinds": [{"kind": kind, "target": None}]})
        return directory

    def dependency(self, name, version, *, license="MIT OR Apache-2.0", license_file=None,
                   files=None, source="registry", directory=None, kind=None):
        directory = Path(directory) if directory is not None else self.root / "deps" / f"{name}-{version}"
        if source == "registry":
            package_id = f"registry+https://github.com/rust-lang/crates.io-index#{name}@{version}"
            source_value = "registry+https://github.com/rust-lang/crates.io-index"
        elif source == "git":
            package_id = f"git+https://github.com/zed-industries/zed?rev={GIT_REV}#{name}@{version}"
            source_value = f"git+https://github.com/zed-industries/zed?rev={GIT_REV}"
        else:
            package_id = f"path+file://<local>/{directory.name}#{name}@{version}"
            source_value = None
        self._add(package_id, name, version, source_value, directory, license, license_file, kind)
        for relative, content in (files or {}).items():
            target = directory / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(content)
        return package_id, directory

    def workspace_member(self, name, version="0.1.0", *, license="Apache-2.0"):
        directory = self.root / "crates" / name
        package_id = f"path+file://<workspace>/crates/{name}#{name}@{version}"
        self._add(package_id, name, version, None, directory, license, None, None)
        return package_id, directory

    def metadata(self) -> dict:
        return {"packages": self.packages,
                "resolve": {"root": self.root_id,
                            "nodes": [{"id": key, "deps": value} for key, value in self.edges.items()]},
                "workspace_root": str(self.root)}


class NoticeBundleTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="notice-bundle-")
        self.graph = Graph(Path(self.temporary.name) / "source")

    def tearDown(self):
        self.temporary.cleanup()

    def build(self, *, cargo_home=None):
        return notice_bundle.build(self.graph.metadata(), LOCK, cargo_home=cargo_home)

    def test_bundle_bytes_are_deterministic(self):
        self.graph.dependency("alpha", "1.0.0", files={"LICENSE-MIT": b"MIT text\n"})
        first, first_claim = self.build()
        second, second_claim = self.build()
        self.assertEqual(first, second)
        self.assertEqual(first_claim["bundle_sha256"], hashlib.sha256(first).hexdigest())
        self.assertEqual(first_claim["bundle_sha256"], second_claim["bundle_sha256"])

    def test_bundle_bytes_are_stable_across_processes_and_hash_seeds(self):
        self.graph.dependency("alpha", "1.0.0", files={"LICENSE-MIT": b"MIT text\n",
                                                       "NOTICE": b"An attribution\n"})
        metadata_path = Path(self.temporary.name) / "metadata.json"
        metadata_path.write_text(json.dumps(self.graph.metadata()), encoding="utf-8")
        program = (
            "import json, sys;"
            "sys.path.insert(0, sys.argv[2]);"
            "import notice_bundle;"
            "data, claim = notice_bundle.build(json.load(open(sys.argv[1], encoding='utf-8')), sys.argv[3]);"
            "print(claim['bundle_sha256'])"
        )
        digests = set()
        for seed in ("1", "2", "3"):
            result = subprocess.run(
                [sys.executable, "-B", "-c", program, str(metadata_path), str(SCRIPT.parent), LOCK],
                env={**os.environ, "PYTHONHASHSEED": seed},
                capture_output=True, text=True, encoding="utf-8",
            )
            self.assertEqual(0, result.returncode, result.stderr)
            digests.add(result.stdout.strip())
        self.assertEqual(1, len(digests))
        self.assertEqual(hashlib.sha256(self.build()[0]).hexdigest(), digests.pop())

    def test_missing_local_text_is_unassessed_not_pass(self):
        self.graph.dependency("alpha", "1.0.0")
        data, claim = self.build()
        self.assertEqual(1, claim["dependencies"])
        self.assertEqual(0, claim["text_found"])
        self.assertEqual(1, claim["text_not_found"])
        self.assertEqual("NOT_ASSESSED", claim["legal_compliance"])
        row = notice_bundle.parse_bundle(data).rows[0]
        self.assertEqual("TEXT_NOT_FOUND", row["status"])
        self.assertEqual([], row["texts"])

    def test_declared_license_file_inside_the_package_is_bound(self):
        self.graph.dependency("alpha", "1.0.0", license="Custom", license_file="legal/COPYING.txt",
                              files={"legal/COPYING.txt": b"custom terms\n"})
        data, claim = self.build()
        self.assertEqual(1, claim["text_found"])
        row = notice_bundle.parse_bundle(data).rows[0]
        self.assertEqual("legal/COPYING.txt", row["declared_license_file"])
        self.assertEqual(["legal/COPYING.txt"], [text["path"] for text in row["texts"]])

    def test_declared_license_file_outside_the_package_is_not_read(self):
        outside = Path(self.temporary.name) / "outside-secret.txt"
        outside.write_bytes(b"must never be bundled\n")
        self.graph.dependency("alpha", "1.0.0", license_file="../outside-secret.txt")
        data, claim = self.build()
        self.assertEqual(1, claim["text_not_found"])
        self.assertIsNone(notice_bundle.parse_bundle(data).rows[0]["declared_license_file"])
        self.assertNotIn(b"must never be bundled", data)
        self.assertIn("TEXT_OUTSIDE_PACKAGE", data.decode("utf-8"))

    def test_dev_only_dependency_is_not_noticed(self):
        self.graph.dependency("dev-only", "1.0.0", kind="dev", files={"LICENSE": b"dev text\n"})
        self.graph.dependency("shipped", "1.0.0", files={"LICENSE-MIT": b"shipped text\n"})
        data, claim = self.build()
        self.assertEqual(1, claim["dependencies"])
        self.assertNotIn(b"dev-only", data)
        self.assertEqual(["shipped"], [row["name"] for row in notice_bundle.parse_bundle(data).rows])

    def test_build_only_dependency_is_noticed_with_its_role(self):
        self.graph.dependency("builder", "1.0.0", kind="build", files={"LICENSE": b"build text\n"})
        data, _ = self.build()
        row = notice_bundle.parse_bundle(data).rows[0]
        self.assertEqual("build-only", row["role"])

    def test_duplicate_dependency_identity_is_refused(self):
        first_id, _ = self.graph.workspace_member("alpha")
        second_id = "path+file://<workspace>/dup#alpha@0.1.0"
        metadata = self.graph.metadata()
        metadata["packages"].append(dict(metadata["packages"][1], id=second_id))
        metadata["resolve"]["nodes"][0]["deps"].append(
            {"pkg": second_id, "dep_kinds": [{"kind": None, "target": None}]})
        metadata["resolve"]["nodes"].append({"id": second_id, "deps": []})
        with self.assertRaises(notice_bundle.NoticeError):
            notice_bundle.build(metadata, LOCK)

    @unittest.skipIf(os.name == "nt", "case-variant file names are not creatable on Windows")
    def test_case_variant_license_names_are_refused(self):
        self.graph.dependency("alpha", "1.0.0", files={"LICENSE": b"one\n", "license": b"two\n"})
        with self.assertRaises(notice_bundle.NoticeError):
            self.build()

    def test_identical_texts_are_stored_once(self):
        text = b"the same MIT terms\n"
        self.graph.dependency("alpha", "1.0.0", files={"LICENSE": text})
        self.graph.dependency("beta", "2.0.0", files={"LICENSE": text})
        data, claim = self.build()
        self.assertEqual(2, claim["dependencies"])
        self.assertEqual(2, claim["text_found"])
        self.assertEqual(1, len(notice_bundle.parse_bundle(data).blobs))

    def test_non_utf8_text_is_unassessed(self):
        self.graph.dependency("alpha", "1.0.0", files={"LICENSE": b"\xff\xfe\x00"})
        data, claim = self.build()
        self.assertEqual(1, claim["text_not_found"])
        row = notice_bundle.parse_bundle(data).rows[0]
        self.assertEqual("TEXT_NOT_FOUND", row["status"])
        self.assertIn("TEXT_NOT_UTF8", data.decode("utf-8"))

    def test_oversized_text_is_unassessed(self):
        self.graph.dependency("alpha", "1.0.0",
                              files={"LICENSE": b"x" * (notice_bundle.MAX_TEXT_BYTES + 1)})
        data, claim = self.build()
        self.assertEqual(1, claim["text_not_found"])
        self.assertIn("TEXT_OVERSIZED", data.decode("utf-8"))

    def test_absent_source_directory_is_unassessed(self):
        _, directory = self.graph.dependency("alpha", "1.0.0")
        for child in directory.iterdir():
            child.unlink()
        directory.rmdir()
        data, claim = self.build()
        self.assertEqual(1, claim["text_not_found"])
        self.assertIn("SOURCE_UNAVAILABLE", data.decode("utf-8"))

    def test_workspace_members_bind_the_shared_root_texts(self):
        (self.graph.root / "LICENSE").write_bytes(b"Apache License 2.0 workspace\n")
        (self.graph.root / "NOTICE").write_bytes(b"workspace notice\n")
        self.graph.workspace_member("desktop-gpui")
        data, claim = self.build()
        self.assertEqual(1, claim["text_found"])
        row = notice_bundle.parse_bundle(data).rows[0]
        self.assertEqual("workspace", row["source_kind"])
        self.assertEqual(["workspace/LICENSE", "workspace/NOTICE"],
                         [text["path"] for text in row["texts"]])

    def test_pointer_text_is_resolved_inside_the_git_checkout_tree(self):
        cargo_home = Path(self.temporary.name) / "cargo"
        checkout = cargo_home / "git" / "checkouts" / "zed-abc" / "rev"
        crate = checkout / "crates" / "gpui"
        crate.mkdir(parents=True)
        (crate / "LICENSE-APACHE").write_bytes(b"../../LICENSE-APACHE\n")
        (checkout / "LICENSE-APACHE").write_bytes(b"shared Apache terms\n")
        self.graph.dependency("gpui", "0.2.2", source="git", directory=crate,
                              files={"LICENSE-APACHE": b"../../LICENSE-APACHE\n"})
        data, claim = self.build(cargo_home=cargo_home)
        self.assertEqual(1, claim["text_found"])
        row = notice_bundle.parse_bundle(data).rows[0]
        self.assertEqual(1, len(row["texts"]))
        self.assertTrue(row["texts"][0]["path"].endswith("zed-abc/rev/LICENSE-APACHE"),
                        row["texts"][0]["path"])
        self.assertIn(b"shared Apache terms", data)

    def test_pointer_escaping_the_checkout_tree_is_not_resolved(self):
        cargo_home = Path(self.temporary.name) / "cargo"
        checkout = cargo_home / "git" / "checkouts" / "zed-abc" / "rev"
        crate = checkout / "crates" / "gpui"
        crate.mkdir(parents=True)
        outside = cargo_home / "outside-license.txt"
        outside.write_bytes(b"outside terms must not be bundled\n")
        pointer = b"../../../../../../outside-license.txt\n"
        (crate / "LICENSE-APACHE").write_bytes(pointer)
        self.graph.dependency("gpui", "0.2.2", source="git", directory=crate)
        data, claim = self.build(cargo_home=cargo_home)
        self.assertEqual(1, claim["text_not_found"])
        self.assertIn("POINTER_NOT_RESOLVED", data.decode("utf-8"))
        self.assertNotIn(b"outside terms must not be bundled", data)

    def test_registry_pointer_only_package_is_unassessed(self):
        # A registry package that ships only "see ../LICENSE" pointers has no licence
        # text inside it; counting the pointer as a text would overclaim.
        self.graph.dependency("pointer-only", "2.22.0", files={
            "LICENSE-APACHE": b"../LICENSE-APACHE\n",
            "LICENSE-MIT": b"../LICENSE-MIT\n",
        })
        data, claim = self.build()
        self.assertEqual(1, claim["text_not_found"])
        self.assertEqual(0, claim["text_found"])
        row = notice_bundle.parse_bundle(data).rows[0]
        self.assertEqual("TEXT_NOT_FOUND", row["status"])
        self.assertEqual(["POINTER_NOT_RESOLVED", "POINTER_NOT_RESOLVED"],
                         [item["reason"] for item in row["unresolved"]])
        self.assertEqual({}, notice_bundle.parse_bundle(data).blobs)

    def test_claim_and_bundle_are_cross_checked(self):
        self.graph.dependency("alpha", "1.0.0", files={"LICENSE-MIT": b"MIT text\n"})
        data, claim = self.build()
        parsed = notice_bundle.check_manifest(claim, data)
        self.assertEqual(claim["dependencies"], len(parsed.rows))
        self.assertEqual(claim["dependency_set_sha256"], parsed.dependency_set_sha256)
        other_graph = Graph(Path(self.temporary.name) / "other")
        other_graph.dependency("alpha", "2.0.0", files={"LICENSE-MIT": b"other text\n"})
        other_data, other_claim = notice_bundle.build(other_graph.metadata(), LOCK)
        with self.assertRaises(notice_bundle.NoticeError):
            notice_bundle.check_manifest(claim, other_data)
        with self.assertRaises(notice_bundle.NoticeError):
            notice_bundle.check_manifest(other_claim, data)

    def test_bundle_does_not_embed_an_absolute_path(self):
        self.graph.dependency("alpha", "1.0.0", files={"LICENSE-MIT": b"MIT text\n"})
        data, _ = self.build()
        self.assertNotIn(str(self.graph.root).encode("utf-8"), data)
        self.assertNotIn(str(Path(self.temporary.name)).encode("utf-8"), data)

    def test_build_performs_no_network_io(self):
        self.graph.dependency("alpha", "1.0.0", files={"LICENSE-MIT": b"MIT text\n"})
        import socket

        with (
            patch.object(socket, "socket", side_effect=AssertionError("network use")),
            patch.object(socket, "create_connection", side_effect=AssertionError("network use")),
        ):
            notice_bundle.build(self.graph.metadata(), LOCK)

    def test_tampered_bundle_is_refused(self):
        self.graph.dependency("alpha", "1.0.0", files={"LICENSE-MIT": b"MIT text\n"})
        data, claim = self.build()
        tampered = data.replace(b"MIT text\n", b"forged text\n")
        with self.assertRaises(notice_bundle.NoticeError):
            notice_bundle.check_manifest(claim, tampered)


if __name__ == "__main__":
    unittest.main(verbosity=2)
