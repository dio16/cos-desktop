"""Replay the six published Native S2B journals, not displayed frames or full bundles.

S3's four ledger counters cannot express these event populations. Keep that schema
unchanged; reuse its canonical encoding, S1 pair validation and strict S2B contracts.
No child, network request, temporary file or output file is created by this reader.
"""
from __future__ import annotations

import argparse
import hashlib
import math
import re
import sys
from pathlib import Path

import contracts
import driver
import s2b_contracts as s2b
import s3_ledger
from s2b_capture import read_pinned
from windows_owner import Pin

SCENARIOS = ("keyboard", "held-key", "pointer", "scroll", "navigation", "combined")
MAX_OUTPUT_BYTES = 1024 * 1024
MAX_TOTAL_BYTES = 4 * 1024 * 1024
MISSING_INPUTS = (
    "original-feature-manifest", "build-record", "source-identity-and-source-files",
    "original-s2a-result", "original-s2a-completion-and-commit-markers",
    "executable-bytes", "helper-bytes-and-source", "fixture-bytes",
    "original-launch-timeout-declaration",
)


def sha(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def require(condition: bool, code: str) -> None:
    if not condition:
        s2b.fail(code)


def input_names() -> set[str]:
    names = {"README.md"}
    for name in SCENARIOS:
        names.update(f"capture-run/{name}/{file}" for file in
                     ("s2b-capture.json", "s2b-observation.json", "ledger.json"))
        names.update(f"r0c/{name}-{kind}.json" for kind in ("manifest", "result"))
        names.add(f"raw/{name}-driver.json")
    return names


def read_inputs(root: Path) -> tuple[dict[str, bytes], bytes]:
    """Only fixed layout members; all digests cover the same bounded pinned bytes."""
    root = root.absolute()
    with Pin(root, directory=True) as pin:
        index = read_pinned(root / "SHA256SUMS", 16 * 1024)
        try:
            lines = index.decode("ascii").splitlines()
        except UnicodeDecodeError:
            s2b.fail("CHECKSUM_INDEX")
        expected = input_names()
        hashes = {}
        for line in lines:
            parts = line.split("  ")
            require(len(parts) == 2, "CHECKSUM_INDEX")
            value, name = parts
            require(name in expected and name not in hashes, "CHECKSUM_INDEX")
            hashes[name] = s2b.digest("sha256:" + value)
        require(set(hashes) == expected, "CHECKSUM_INDEX")
        data = {}
        total = len(index)
        for name in sorted(expected):
            raw = read_pinned(root / name, s2b.MAX_BYTES)
            total += len(raw)
            require(total <= MAX_TOTAL_BYTES, "INPUT_LIMIT")
            require(sha(raw) == hashes[name], "CHECKSUM_MISMATCH")
            data[name] = raw
        pin.verify()
    return data, index


def parse(data: dict[str, bytes], name: str) -> dict:
    value = s2b.parse_bytes(data[name], s2b.MAX_BYTES)
    if not isinstance(value, dict):
        s2b.fail("SCHEMA")
    return value


def evidence_bindings(data: dict[str, bytes], result: dict) -> list[dict]:
    """Accept CRLF only as the documented, recomputed historical reference form."""
    bindings = {}
    for evidence in [result["evidence"], result["runs"][0]["evidence"]]:
        require(len(evidence) == len(set(evidence)), "DUPLICATE_EVIDENCE")
        for item in evidence:
            if " sha256:" not in item:
                continue
            name, digest = item.split(" ", 1)
            require(name in data and name != "README.md", "EVIDENCE_REFERENCE")
            s2b.digest(digest)
            raw = data[name]
            if digest == sha(raw):
                form = "PUBLISHED_BYTES"
            else:
                require(b"\r" not in raw and digest == sha(raw.replace(b"\n", b"\r\n")),
                        "EVIDENCE_DIGEST")
                form = "DOCUMENTED_CRLF_RESTORATION"
            bindings[name] = {"path": name, "recorded_sha256": digest, "form": form}
    return [bindings[name] for name in sorted(bindings)]


def validate_scenario(data: dict[str, bytes], name: str) -> tuple[dict, dict, dict, list]:
    base = f"capture-run/{name}/"
    capture_raw = data[base + s2b.FILE_NAME]
    observation = parse(data, base + s2b.OBSERVATION_NAME)
    ledger = parse(data, base + "ledger.json")
    manifest = contracts.validate_manifest(parse(data, f"r0c/{name}-manifest.json"))
    result = contracts.validate_result(parse(data, f"r0c/{name}-result.json"))
    contracts.check_identity(manifest, result)
    identity = manifest["identity"]
    require(identity["build"]["features"] == [s2b.FEATURE]
            and identity["build"]["profile"] == "dev"
            and identity["repository"]["dirty"] is False
            and identity["gpui_revision"] == s2b.GPUI_PIN
            and identity["toolchain"]["target"] == "x86_64-pc-windows-msvc",
            "INSTRUMENTATION_IDENTITY")
    # The original timeout declaration is absent. This upper contract bound checks
    # structure only; it does not reconstruct that invocation or prove its deadline.
    execution = ledger.get("execution_id")
    if not isinstance(execution, str):
        s2b.fail("EXECUTION")
    driver.validate_ledger(ledger, identity, execution,
                           manifest["workload"]["source"]["sha256"], driver.MAX_TIMEOUT_MS)
    capture = s2b.validate_capture(s2b.parse_bytes(capture_raw, s2b.MAX_BYTES),
        execution=ledger["execution_id"], pid=ledger["pid"],
        executable=identity["executable_sha256"])
    s2b.object_fields(observation, {"observation_version", "identity", "capture", "s2a",
        "endpoints", "coverage", "frame_identity", "cross_domain_latency", "loss_scope",
        "unobserved_source_loss"})
    require(type(observation["observation_version"]) is int
            and observation["observation_version"] == 1, "VERSION")
    bound = s2b.object_fields(observation["identity"], {"source_commit", "manifest_sha256",
        "build_sha256", "observer_source_sha256", "executable_sha256", "helper_sha256",
        "helper_source_sha256", "fixture_sha256", "execution_id", "pid", "gpui_pin", "feature"})
    for key in ("manifest_sha256", "build_sha256", "observer_source_sha256",
                "executable_sha256", "helper_sha256", "helper_source_sha256", "fixture_sha256"):
        s2b.digest(bound[key])
    require(bound["source_commit"] == identity["repository"]["sha"]
            and bound["execution_id"] == capture["execution_id"]
            and type(bound["pid"]) is int and bound["pid"] == capture["pid"]
            and bound["executable_sha256"] == capture["executable_sha256"]
            and bound["fixture_sha256"] == ledger["fixture_sha256"]
            and bound["gpui_pin"] == capture["gpui_pin"]
            and bound["feature"] == capture["feature"], "OBSERVATION_BINDING")
    capture_ref = s2b.object_fields(observation["capture"], {"name", "bytes", "sha256"})
    s2b.integer(capture_ref["bytes"], high=s2b.MAX_BYTES)
    require(observation["capture"] == {"name": s2b.FILE_NAME, "bytes": len(capture_raw),
                                      "sha256": sha(capture_raw)}, "CAPTURE_DIGEST")
    s2a = s2b.object_fields(observation["s2a"], {"ledger.json", "result.json", "completion.json"})
    for digest in s2a.values():
        s2b.digest(digest)
    require(s2a["ledger.json"] == sha(data[base + "ledger.json"]), "LEDGER_DIGEST")
    require(observation["endpoints"] == s2b.endpoint_matrix(capture)
            and observation["coverage"] == "LIMITED_SOURCE_OBSERVATIONS_ONLY"
            and observation["frame_identity"] == "NOT_MEASURED"
            and observation["cross_domain_latency"] == "NOT_MEASURED"
            and observation["loss_scope"] == "OFFERED_FINALIZED_JOURNAL_PLUS_LOCAL_QUEUE"
            and observation["unobserved_source_loss"] == "UNKNOWN", "OBSERVATION_BINDING")
    raw = parse(data, f"raw/{name}-driver.json")
    s2b.object_fields(raw, {"scenario", "label", "requested_hz", "window", "close", "input",
        "messages", "spans_s", "achieved_hz", "wrapper_exit_code", "wrapper_stdout", "wrapper_stderr"})
    categories = {
        "keyboard": {"key"}, "held-key": {"held-keydown"}, "pointer": {"move", "click"},
        "scroll": {"wheel"}, "navigation": {"ctrl-tab"}, "combined": {"key", "move", "wheel"},
    }[name]
    for key in ("messages", "spans_s", "achieved_hz"):
        values = s2b.object_fields(raw[key], categories)
        for value in values.values():
            if key == "messages":
                s2b.integer(value)
            else:
                require(type(value) in (int, float) and math.isfinite(value) and value > 0,
                        "DRIVER_CADENCE")
    require(type(raw["requested_hz"]) in (int, float)
            and math.isfinite(raw["requested_hz"]) and raw["requested_hz"] > 0, "DRIVER_CADENCE")
    require(raw.get("scenario") == name and type(raw.get("wrapper_exit_code")) is int
            and raw["wrapper_exit_code"] == 0 and raw.get("wrapper_stderr") == "",
            "DRIVER_BINDING")
    require(isinstance(raw.get("wrapper_stdout"), str), "DRIVER_BINDING")
    wrapper = s2b.parse_bytes(raw["wrapper_stdout"].encode(), s2b.MAX_BYTES)
    require(isinstance(wrapper, dict) and wrapper.get("ok") is True
            and wrapper.get("execution_id") == capture["execution_id"]
            and wrapper.get("retained") == len(capture["records"])
            and wrapper.get("retention_dropped") == capture["summary"]["retention_dropped"]
            and wrapper.get("endpoints") == observation["endpoints"], "DRIVER_BINDING")
    require(len(result["runs"]) == 1, "RESULT_BINDING")
    run = result["runs"][0]
    require(run["run_id"] == f"run-r0c-{name}"
            and run["frames"]["processed"] == capture["summary"]["kind_counts"][2]
            and run["frames"]["submitted"] == capture["summary"]["kind_counts"][3], "RESULT_BINDING")
    for field in ("present", "displayed", "missed", "dropped", "stalled"):
        require(run["frames"][field] is None, "UNOBSERVED_ENDPOINT")
    endpoints = run["endpoints"]
    require(len({item["id"] for item in endpoints}) == len(endpoints), "RESULT_BINDING")
    for item in endpoints:
        require(item["id"] in s2b.SUPPORTED or item["id"] == "process-launch"
                or item["status"] != "OBSERVED", "UNOBSERVED_ENDPOINT")
    mapped = {item["id"]: item for item in endpoints}
    for endpoint, kind in s2b.SUPPORTED.items():
        item = mapped.get(endpoint)
        if item is None:
            s2b.fail("RESULT_BINDING")
        require(item["status"] == observation["endpoints"][endpoint]["status"], "RESULT_BINDING")
        if capture["summary"]["kind_counts"][kind]:
            require(item["clock"] == capture["clock"]["domain"]
                    and item["version"] == capture["execution_id"][:12], "RESULT_BINDING")
    bindings = evidence_bindings(data, result)
    require({item["path"] for item in bindings} == {
        base + s2b.FILE_NAME, base + s2b.OBSERVATION_NAME, base + "ledger.json",
        f"raw/{name}-driver.json", f"r0c/{name}-manifest.json"}, "EVIDENCE_REFERENCE")
    return capture, observation, raw, bindings


def metric(samples: list[list[int]], *, population: int, excluded: int = 0) -> dict:
    values = [sample[-1] for sample in samples]
    return {"status": "OBSERVED" if values else "NOT_MEASURED", "unit": "ns",
            "population": population, "count": len(values), "excluded": excluded,
            "sum_ns": sum(values) if values else None,
            "min_ns": min(values) if values else None, "max_ns": max(values) if values else None,
            "samples": samples}


def interval_metrics(capture: dict, kind: int) -> dict:
    records = [r for r in capture["records"] if r["kind"] == kind]
    groups = {}
    for record in records:
        groups.setdefault((record["window"], record["lifetime"]), []).append(record)
    starts, gaps, excluded = [], [], 0
    barriers = [r["seq"] for r in capture["records"] if r["kind"] in (5, 7)]
    for group in groups.values():
        group.sort(key=lambda r: (r["start_ns"], r["end_ns"], r["seq"]))
        for previous, current in zip(group, group[1:]):
            lo, hi = sorted((previous["seq"], current["seq"]))
            if any(lo < barrier < hi for barrier in barriers):
                excluded += 1
                continue
            starts.append([previous["seq"], current["seq"], current["start_ns"] - previous["start_ns"]])
            gaps.append([previous["seq"], current["seq"], current["start_ns"] - previous["end_ns"]])
    population = sum(max(0, len(group) - 1) for group in groups.values())
    dirty = [[r["seq"], r["start_ns"] - r["dirty_ns"]]
             for r in records if r["dirty_ns"] is not None]
    return {"record_seqs": [r["seq"] for r in records],
            "duration": metric([[r["seq"], r["end_ns"] - r["start_ns"]] for r in records], population=len(records)),
            "start_to_start": metric(starts, population=population, excluded=excluded),
            "end_to_start": metric(gaps, population=population, excluded=excluded),
            "dirty_to_start": metric(dirty, population=len(records), excluded=len(records)-len(dirty)),
            "invalidations": {"record_seqs": [r["seq"] for r in records if r["invalidations"] is not None],
                "count": sum(r["invalidations"] is not None for r in records),
                "sum": sum(r["invalidations"] or 0 for r in records),
                "multiple_record_seqs": [r["seq"] for r in records if (r["invalidations"] or 0) > 1],
                "max": max((r["invalidations"] for r in records if r["invalidations"] is not None), default=None)}}


def aggregate(capture: dict) -> dict:
    """All sample references are seq IDs in this one capture, never cross-run clocks."""
    inputs = [r for r in capture["records"] if r["kind"] == 0]
    paired = [r for r in capture["records"] if r["kind"] == 3 and r["paired_draw_start_ns"] is not None]
    categories = []
    for category in sorted({r["category"] for r in inputs}):
        selected = [r for r in inputs if r["category"] == category]
        categories.append({"category": category, "record_seqs": [r["seq"] for r in selected],
                           "count": len(selected), "caused_invalidation": sum(r["caused_invalidation"] for r in selected)})
    return {"kind_counts": [sum(r["kind"] == kind for r in capture["records"]) for kind in range(10)],
        "accounting": capture["summary"], "clock": capture["clock"],
        "scope_start_ns": capture["scope_start_ns"], "scope_end_ns": capture["scope_end_ns"],
        "draw": interval_metrics(capture, 2), "submission": interval_metrics(capture, 3),
        "paired_draw_end_to_submission_start": metric(
            [[r["seq"], r["start_ns"] - r["paired_draw_end_ns"]] for r in paired],
            population=capture["summary"]["kind_counts"][3],
            excluded=capture["summary"]["kind_counts"][3]-len(paired)),
        "input_categories": categories, "input_count": len(inputs),
        "input_window_association": "NOT_MEASURED",
        "input_to_draw_correlation": "NOT_MEASURED",
        "full_population": False, "visible_stall": "UNVERIFIED",
        "endpoints": s2b.endpoint_matrix(capture)}


def readme_comparison(readme: str, name: str, capture: dict, aggregate: dict) -> dict:
    """Check the published table, preserving its record-versus-pair ambiguity."""
    rows = [line for line in readme.splitlines() if line.startswith(f"| `{name}` |")]
    require(len(rows) == 1, "README_SCENARIO")
    cells = [cell.strip() for cell in rows[0].strip("|").split("|")]
    require(len(cells) == 11 and cells[10] == f'`{capture["execution_id"]}`', "README_SCENARIO")
    counts = aggregate["kind_counts"]
    summary = capture["summary"]
    require(cells[4] == f'{len(capture["records"])}/{summary["eligible"]}'
            and cells[5] == f'{summary["retention_dropped"]}/{summary["input_unassociated"]}',
            "README_COUNTS")
    require(cells[6] == ", ".join(f"{label} {counts[kind]}" for label, kind in (
        ("input", 0), ("dirty", 1), ("draw", 2), ("submit", 3),
        ("surface-mutation", 8), ("notify", 9))), "README_COUNTS")
    gap = aggregate["draw"]["end_to_start"]
    if counts[2]:
        match = re.fullmatch(r"max ([0-9]+\.[0-9]{3}) ms / ([0-9]+) intervals", cells[7])
        if match is None:
            s2b.fail("README_GAP")
        require(match[1] == f'{gap["max_ns"] / 1_000_000:.3f}'
                and int(match[2]) == counts[2], "README_GAP")
    else:
        require(cells[7] == "no draw intervals", "README_GAP")
    invalidations = [aggregate[kind]["invalidations"] for kind in ("draw", "submission")]
    multiple = sum(len(item["multiple_record_seqs"]) for item in invalidations)
    population = sum(item["count"] for item in invalidations)
    maximum = max((item["max"] or 0 for item in invalidations), default=0)
    require(cells[8] == f"{multiple}/{population} (max {maximum})", "README_COUNTS")
    return {"counts_match": True, "gap_max_matches": bool(counts[2]),
            "published_draw_record_denominator": counts[2], "adjacent_gap_denominator": gap["count"],
            "gap_denominator_agrees": counts[2] == gap["count"],
            "scope_initial_gap_included": False,
            "gap_definition_limit": "README interval denominator names n records, not n-1 adjacent pairs; no hidden sample inferred"}


def replay(root: Path) -> dict:
    data, index = read_inputs(root)
    try:
        readme = data["README.md"].decode("utf-8")
    except UnicodeDecodeError:
        s2b.fail("INVALID_UTF8")
    scenarios, executions, identities = [], set(), set()
    for name in SCENARIOS:
        capture, observation, raw, bindings = validate_scenario(data, name)
        require(capture["execution_id"] not in executions, "DUPLICATE_EXECUTION")
        executions.add(capture["execution_id"])
        bound = observation["identity"]
        identities.add(tuple(bound[k] for k in ("source_commit", "executable_sha256", "helper_sha256", "fixture_sha256", "gpui_pin", "feature")))
        require(all(str(bound[k]) in readme for k in ("source_commit", "executable_sha256", "helper_sha256", "fixture_sha256", "gpui_pin", "feature")), "README_IDENTITY")
        value = aggregate(capture)
        comparison = readme_comparison(readme, name, capture, value)
        scenarios.append({"scenario": name, "execution_id": capture["execution_id"],
            "declared_identity": bound, "verification_scope": "PUBLISHED_RECORD_CONSISTENCY_ONLY",
            "unavailable_verification_inputs": list(MISSING_INPUTS), "evidence_bindings": bindings,
            "driver_cadence": {"status": "RECORDED_SELF_REPORT", "clock_mapping": "NONE",
                "requested_hz": raw.get("requested_hz"), "achieved_hz": raw.get("achieved_hz"),
                "messages": raw.get("messages")}, "aggregate": value, "readme_comparison": comparison})
    require(len(identities) == 1, "SOURCE_IDENTITY")
    for path, raw in data.items():
        if path == "README.md":
            continue
        require(b"\r" not in raw, "PUBLISHED_BYTE_FORM")
        restored = sha(raw.replace(b"\n", b"\r\n"))
        row = f"| `{path}` | `{sha(raw)[7:]}` | `{restored[7:]}` |"
        require(row in readme, "README_DIGEST_TABLE")
    return {"replay_version": 1, "scope": "RETAINED_NATIVE_JOURNAL_ONLY",
        "checksum_index_sha256": sha(index),
        "inputs": [{"path": name, "bytes": len(data[name]), "sha256": sha(data[name])} for name in sorted(data)],
        "scenarios": scenarios, "full_native_acceptance": False,
        "formula": "duration=end-start; gaps=adjacent chronological same-window/lifetime end-to-start or start-to-start; no scope-initial sample; loss/power barriers exclude pairs; sample=[seq,ns] or [previous_seq,next_seq,ns]; sum/count is the exact mean numerator/denominator",
        "limits": ["retained records are not the whole event population", "draw is not displayed; platform submission is not Present",
            "no cross-run or driver/native clock mapping", "invalidation sums include draw and submission separately, not distinct changes or queue depth",
            "README gap denominators name draw records, whereas adjacent gaps have n-1 samples; compare maxima separately",
            "README historical self-digest row is not the final README digest; SHA256SUMS binds final bytes",
            "README backlog prose mentions 428 eligible; validated capture accounting is authoritative",
            "original observation manifest/build/source/completion and binary bytes are unavailable; full source-bound bundle replay NOT_VERIFIED",
            "navigation journal gaps occur under ~500 ms action spacing, not evidence of visible stalls"]}


def encoded_report(report: dict, limit: int) -> bytes:
    require(type(limit) is int and 1024 <= limit <= MAX_OUTPUT_BYTES, "OUTPUT_LIMIT")
    body = s3_ledger.canonical({"ok": True, "report_sha256": s3_ledger.record_digest(report), "report": report}) + b"\n"
    require(len(body) <= limit, "OUTPUT_LIMIT")
    return body


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input-dir", required=True)
    parser.add_argument("--max-output-bytes", type=int, default=MAX_OUTPUT_BYTES)
    args = parser.parse_args(argv)
    try:
        body = encoded_report(replay(Path(args.input_dir)), args.max_output_bytes)
        sys.stdout.buffer.write(body)
        return 0
    except s2b.ObservationError as error:
        code = str(error)
    except contracts.ContractError as error:
        code = error.errors[0]["code"]
    except (OSError, RuntimeError):
        code = "REPLAY_IO_OR_LAUNCHER_CONTRACT"
    sys.stdout.buffer.write(s3_ledger.canonical({"ok": False, "error": code}) + b"\n")
    return 1


if __name__ == "__main__":
    sys.exit(main())
