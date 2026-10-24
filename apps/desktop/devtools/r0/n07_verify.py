"""New-baseline N07 identity-chain verifier.

Deterministic, stdlib-only verification of the 2026-10-25 approved Native baseline.
Every identity item is recomputed from bytes on this host and compared with the value the
baseline records claim; a mismatch is reported as MISMATCH, never smoothed. The record has
no timestamp, host or random content, so identical inputs (same paths and bytes) produce
identical record bytes. It never writes outside `--output` and never touches historical
records, bundle copies, approvals or Gate records.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

import contracts  # noqa: E402
import driver  # noqa: E402
import s2b_contracts  # noqa: E402

RECORD_VERSION = 1
BUNDLE_CLAIM = "sha256:3a5bc6dabca7fcd804f51e888ec54ccfd87e119d4d3d78cf6b475f4314625bd9"
A0_CLAIM = "sha256:e98814d8ebbae17864682b378fd85f8eef7ce78ccac5194a539bfa86f6dd7f8d"
A1_CLAIM = "sha256:6b989485efe87856f85fe2e8e31a15d473947f1b7a729e038180655110ead88f"


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            hasher.update(chunk)
    return "sha256:" + hasher.hexdigest()


def canonical(value: object) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def sha_command(path: Path) -> str:
    return "sha256sum " + str(path)


def item(name: str, expected: object, observed: object, command: str, claim: str,
         reason: str = "") -> dict:
    if observed is None:
        status = "NOT_MEASURED"
        reason = reason or "INPUT_UNAVAILABLE"
    elif expected is None:
        status = "NOT_MEASURED"
        reason = reason or "CLAIM_ABSENT"
    else:
        status = "MATCH" if expected == observed else "MISMATCH"
    entry = {
        "item": name,
        "status": status,
        "expected": expected,
        "observed": observed,
        "command": command,
        "claim": claim,
    }
    if status == "NOT_MEASURED":
        entry["reason"] = reason
    return entry


def verdict_for(items: list[dict]) -> dict:
    """A record is VERIFIED only when every item is MATCH.

    MISMATCH takes precedence over UNVERIFIED; both are named item-by-item so a reader
    cannot mistake an incomplete or contradicted check for a passing one.
    """
    mismatched = [entry["item"] for entry in items if entry["status"] == "MISMATCH"]
    unmeasured = [entry["item"] for entry in items if entry["status"] == "NOT_MEASURED"]
    if mismatched:
        return {"status": "MISMATCH", "reason": "MISMATCH items: " + ", ".join(mismatched)}
    if unmeasured:
        return {"status": "UNVERIFIED",
                "reason": "NOT_MEASURED items: " + ", ".join(unmeasured)}
    return {"status": "VERIFIED", "reason": ""}


def read_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def approval_entry(approvals: dict, commit: str, executable: str) -> dict | None:
    matches = [
        entry for entry in approvals.get("approved", [])
        if entry.get("product_commit") == commit
        and entry.get("executable_sha256") == executable
    ]
    return matches[0] if len(matches) == 1 else None


def verify(args: argparse.Namespace) -> dict:
    paths = {
        "bundle": Path(args.bundle),
        "build_record": Path(args.build_record),
        "source_identity": Path(args.source_identity),
        "manifest_a0": Path(args.manifest_a0),
        "manifest_a1": Path(args.manifest_a1),
        "approvals": Path(args.approvals),
        "repo_root": Path(args.repo_root),
        "helper": Path(args.helper),
        "helper_copy": Path(args.helper_copy) if args.helper_copy else None,
        "a0": Path(args.a0),
        "a1": Path(args.a1),
        "baseline_record": Path(args.baseline_record),
    }
    record = read_json(paths["baseline_record"])
    build = read_json(paths["build_record"])
    identity = read_json(paths["source_identity"])
    manifests = {"a0": read_json(paths["manifest_a0"]),
                 "a1": read_json(paths["manifest_a1"])}
    approvals = read_json(paths["approvals"])
    source = record.get("source", {})
    product_commit = source.get("product_source_commit")
    observer = s2b_contracts.source_digest()
    helper_source = driver.helper_source_digest()
    current_identity = s2b_contracts.source_identity()

    artifacts = record.get("artifacts", {})
    feature_approval = approval_entry(approvals, product_commit, A1_CLAIM)
    relative = feature_approval.get("helper_relative_path") if feature_approval else None
    approved_delivery = (
        (Path(args.repo_root).resolve() / relative).resolve() if relative else None)
    supplied_delivery = Path(args.helper).resolve()
    items = [
        item("helper.path_is_approved_delivery_point",
             os.path.normcase(str(approved_delivery)) if approved_delivery is not None else None,
             os.path.normcase(str(supplied_delivery)),
             "resolve --helper and the approval helper_relative_path from --repo-root; "
             "compare normalized paths",
             "approval entry helper_relative_path"),
        item("bundle.file_sha256", BUNDLE_CLAIM, digest(paths["bundle"]),
             sha_command(paths["bundle"]), "Lead task-96 retained bundle claim"),
        item("baseline_a0.executable_sha256", A0_CLAIM, digest(paths["a0"]),
             sha_command(paths["a0"]), "Lead task-96 / baseline record artifacts.a0"),
        item("baseline_a1.executable_sha256", A1_CLAIM, digest(paths["a1"]),
             sha_command(paths["a1"]), "Lead task-96 / baseline record artifacts.a1"),
        item("baseline_record.artifacts.a0", artifacts.get("a0", {}).get("sha256"),
             digest(paths["a0"]), sha_command(paths["a0"]), "baseline record"),
        item("baseline_record.artifacts.a1", artifacts.get("a1", {}).get("sha256"),
             digest(paths["a1"]), sha_command(paths["a1"]), "baseline record"),
        item("baseline_record.artifacts.helper", artifacts.get("helper", {}).get("sha256"),
             digest(paths["helper_copy"]) if paths["helper_copy"] is not None
             and paths["helper_copy"].is_file() else None,
             sha_command(paths["helper_copy"]) if paths["helper_copy"] is not None else "n/a",
             "baseline record artifacts.helper (build-scratch copy)"),
        item("helper.executable_sha256", artifacts.get("helper", {}).get("sha256"),
             digest(paths["helper"]), sha_command(paths["helper"]),
             "approval entry / baseline record; helper must live at the approved relative path"),
        item("helper.source_sha256", artifacts.get("helper", {}).get("source_sha256"),
             helper_source, "python -B -c \"import driver; print(driver.helper_source_digest())\"",
             "approval entry / baseline record"),
    ]
    if paths["helper_copy"] is not None:
        items.append(item(
            "helper.approved_copy_sha256", artifacts.get("helper", {}).get("sha256"),
            digest(paths["helper_copy"]) if paths["helper_copy"].is_file() else None,
            sha_command(paths["helper_copy"]), "approved copy preserved in the build scratch"))
    else:
        items.append(item("helper.approved_copy_sha256", None, None, "n/a",
                          "no approved helper copy supplied", "INPUT_UNAVAILABLE"))

    items.extend([
        item("build_record.file_sha256",
             record.get("live_build_record", {}).get("sha256"),
             digest(paths["build_record"]), sha_command(paths["build_record"]),
             "baseline record live_build_record"),
        item("build_record.executable_sha256", artifacts.get("a1", {}).get("sha256"),
             build.get("executable_sha256"), "jq .executable_sha256 s2b-build.json",
             "live build record"),
        item("build_record.baseline_executable_sha256", artifacts.get("a0", {}).get("sha256"),
             build.get("baseline_executable_sha256"), "jq .baseline_executable_sha256 s2b-build.json",
             "live build record"),
        item("build_record.source_commit", product_commit, build.get("source_commit"),
             "jq .source_commit s2b-build.json", "live build record"),
        item("build_record.baseline_source_commit", product_commit,
             build.get("baseline_source_commit"), "jq .baseline_source_commit s2b-build.json",
             "live build record"),
        item("build_record.observer_source_sha256", observer,
             build.get("observer_source_sha256"), "jq .observer_source_sha256 s2b-build.json",
             "s2b_contracts.source_digest()"),
        item("build_record.feature", s2b_contracts.FEATURE, build.get("feature"),
             "jq .feature s2b-build.json", "live build record"),
        item("source_identity.file_sha256",
             record.get("source", {}).get("source_identity_record", {}).get("sha256"),
             digest(paths["source_identity"]), sha_command(paths["source_identity"]),
             "baseline record source.source_identity_record"),
        item("source_identity.observer_source_sha256", observer,
             identity.get("observer_source_sha256"),
             "jq .observer_source_sha256 source-identity.json", "s2b_contracts.source_digest()"),
        item("source_identity.content_matches_current_source", canonical(current_identity),
             canonical(identity), "python -B -c \"import s2b_contracts; print(source_identity())\"",
             "recomputed source identity"),
    ])

    for label, claim in (("a0", A0_CLAIM), ("a1", A1_CLAIM)):
        manifest = manifests[label]
        ident = manifest.get("identity", {})
        definition = contracts.canonical_definition(manifest)
        try:
            contracts.validate_manifest(manifest)
            validation = "PASS"
        except contracts.ContractError as error:
            validation = canonical([entry["code"] for entry in error.errors])
        items.extend([
            item(f"manifest.{label}.file_sha256",
                 record.get("manifests", {}).get(label, {}).get("sha256"),
                 digest(paths["manifest_" + label]), sha_command(paths["manifest_" + label]),
                 "baseline record manifests"),
            item(f"manifest.{label}.executable_sha256", claim,
                 ident.get("executable_sha256"), f"jq .identity.executable_sha256 manifest-{label}.json",
                 "manifest"),
            item(f"manifest.{label}.repository_sha", product_commit,
                 ident.get("repository", {}).get("sha"),
                 f"jq .identity.repository.sha manifest-{label}.json", "manifest"),
            item(f"manifest.{label}.driver_hash", observer, ident.get("driver_hash"),
                 f"jq .identity.driver_hash manifest-{label}.json", "manifest"),
            item(f"manifest.{label}.definition_hash", definition,
                 ident.get("definition_hash"), "python -B -c \"import contracts; canonical_definition(...)\"",
                 "recomputed canonical definition"),
            item(f"manifest.{label}.contracts_validation", "PASS", validation,
                 "python -B -c \"import contracts; contracts.validate_manifest(...)\"",
                 "contracts schema and self-identity"),
        ])

    expected_approval = {
        "product_commit": product_commit,
        "helper_sha256": artifacts.get("helper", {}).get("sha256"),
        "helper_source_sha256": artifacts.get("helper", {}).get("source_sha256"),
        "helper_protocol": 2,
        "helper_relative_path": "target/debug/examples/r0_owner.exe",
    }
    for label, claim in (("a0", A0_CLAIM), ("a1", A1_CLAIM)):
        observed = approval_entry(approvals, product_commit, claim)
        observed_summary = None
        if observed is not None:
            observed_summary = {key: observed.get(key) for key in expected_approval}
            observed_summary["executable_sha256"] = observed.get("executable_sha256")
        expected_summary = dict(expected_approval)
        expected_summary["executable_sha256"] = claim
        items.append(item(
            f"approvals.{label}.entry", canonical(expected_summary),
            canonical(observed_summary) if observed_summary is not None else None,
            "jq '.approved[] | select(.product_commit==...) | {product_commit,executable_sha256,helper_sha256,helper_source_sha256,helper_protocol,helper_relative_path}' approved-executables.json",
            "approval entry"))
    items.extend([
        item("approvals.file_sha256", record.get("approval", {}).get("sha256"),
             digest(paths["approvals"]), sha_command(paths["approvals"]),
             "baseline record approval"),
        item("approvals.helper_sha256_consistent",
             artifacts.get("helper", {}).get("sha256"),
             next((entry.get("helper_sha256") for entry in approvals.get("approved", [])
                   if entry.get("product_commit") == product_commit), None),
             "jq '.approved[].helper_sha256' approved-executables.json",
             "any new-baseline approval entry"),
    ])

    totals = {"MATCH": 0, "MISMATCH": 0, "NOT_MEASURED": 0}
    for entry in items:
        totals[entry["status"]] += 1
    return {
        "record_version": RECORD_VERSION,
        "record_kind": "N07_NEW_BASELINE_IDENTITY",
        "baseline_id": "native-baseline-2026-10-25",
        "supersedes": ("NOTHING. This is a new-baseline N07 identity record; it does not "
                       "modify or reinterpret the historical identity-n07.json, bundle "
                       "copies, manifests, approvals or Gate records."),
        "authority": "Lead task-96, MP-R0A Native admission",
        "verdict": verdict_for(items),
        "inputs": {name: str(path) for name, path in paths.items() if path is not None},
        "items": items,
        "totals": totals,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("bundle", "build-record", "source-identity", "manifest-a0", "manifest-a1",
                 "approvals", "repo-root", "helper", "a0", "a1", "baseline-record", "output"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--helper-copy")
    args = parser.parse_args(argv)
    result = verify(args)
    output = Path(args.output)
    output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    verdict = result["verdict"]
    print(json.dumps({"ok": verdict["status"] == "VERIFIED",
                      "verdict": verdict,
                      "record": str(output),
                      "totals": result["totals"]}, separators=(",", ":")))
    return 0 if verdict["status"] == "VERIFIED" else 1


if __name__ == "__main__":
    sys.exit(main())
