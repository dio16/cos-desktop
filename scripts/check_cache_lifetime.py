"""Replay REC2 sample identities and gauge trends; never certify atlas/GPU residency."""
from __future__ import annotations

import argparse
import hashlib
import json
import math
from datetime import datetime
from pathlib import Path
from statistics import median


def gauge_trend(samples: list[tuple[float, int]]) -> dict:
    """Keep correlated gauge observations separate from run-level certification."""
    if not samples:
        return {"status": "NOT_MEASURED", "reason": "No valid samples"}
    times, values = zip(*samples)
    if any(not math.isfinite(t) for t in times) or any(v < 0 for v in values):
        raise ValueError("Non-finite time or negative resource gauge")
    if any(b <= a for a, b in zip(times, times[1:])):
        raise ValueError("Gauge sample clock is not strictly ordered")
    differences = [b - a for a, b in zip(values, values[1:])]
    mean_time = sum(times) / len(times)
    mean_value = sum(values) / len(values)
    denominator = sum((t - mean_time) ** 2 for t in times)
    slope = (
        sum((t - mean_time) * (v - mean_value) for t, v in samples) / denominator
        if denominator else None
    )
    thirds = [
        values[len(values) * i // 3:len(values) * (i + 1) // 3]
        for i in range(3)
    ]
    tail = thirds[-1]
    return {
        "status": "OBSERVED",
        "unit": "bytes",
        "sample_unit": "correlated process/app gauge, not independent frame or run",
        "observed": len(values),
        "min": min(values),
        "max": max(values),
        "first": values[0],
        "last": values[-1],
        "last_minus_first": values[-1] - values[0],
        "least_squares_bytes_per_second": slope,
        "nondecreasing_growth": bool(differences) and all(d >= 0 for d in differences) and any(d > 0 for d in differences),
        "strictly_increasing": bool(differences) and all(d > 0 for d in differences),
        "final_third_sampled_range": max(tail) - min(tail),
        "thirds_mean_bytes": [sum(part) / len(part) if part else None for part in thirds],
        "p50": median(values),
        "p95": None,
        "p99": None,
        "validity_reason": "Gauge spread only; exact final-third equality is a sampled plateau observation, not an accepted no-growth threshold or GPU lifetime proof",
    }


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def analyze_campaign(directory: Path) -> dict:
    failures: list[str] = []
    runs: list[dict] = []
    manifest_path = directory / "manifest.json"
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8-sig"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        return {
            "schema_version": 1, "task": "MP-REC2", "contract": "DC-MP-REC2",
            "status": "FAILED", "acceptance": "HOLD",
            "reason": "Manifest is missing or invalid; no evidence accepted",
            "manifest_sha256": None, "runs": [],
            "failures": [f"Manifest is missing or invalid: {exc}"],
        }

    if not isinstance(manifest, dict):
        manifest = {}
        failures.append("Manifest root must be an object")
    if (not isinstance(manifest.get("manifest_version"), int)
            or isinstance(manifest.get("manifest_version"), bool)
            or manifest.get("manifest_version") != 1
            or manifest.get("task") != "MP-REC2"
            or manifest.get("contract") != "DC-MP-REC2"):
        failures.append("Manifest version/task/contract mismatch")
    if (manifest.get("variant") not in ("shared", "independent", "distinct", "windows2")
            or not isinstance(manifest.get("recovery"), bool)):
        failures.append("Campaign workload variant/recovery identity is missing or invalid")
    if (not isinstance(manifest.get("tracked_dirty"), bool)
            or not isinstance(manifest.get("tracked_dirty_after"), bool)
            or manifest.get("tracked_dirty") or manifest.get("tracked_dirty_after")):
        failures.append("Source cleanliness identity is missing or tracked-dirty")
    source = manifest.get("source_commit")
    if (not isinstance(source, str) or len(source) != 40
            or any(c not in "0123456789abcdef" for c in source.lower())
            or source != manifest.get("source_commit_after")):
        failures.append("Source commit identity is missing, malformed or changed during measurement")
    binary_hash = manifest.get("executable_sha256")
    if (not isinstance(binary_hash, str) or len(binary_hash) != 64
            or any(c not in "0123456789abcdef" for c in binary_hash.lower())
            or binary_hash.lower() != str(manifest.get("executable_sha256_after", "")).lower()):
        failures.append("Executable SHA-256 identity is missing, malformed or changed")
    driver_hash = manifest.get("driver_sha256")
    if (not isinstance(driver_hash, str) or len(driver_hash) != 64
            or any(c not in "0123456789abcdef" for c in driver_hash.lower())
            or driver_hash != manifest.get("driver_sha256_after")):
        failures.append("Observer driver SHA-256 identity is missing, malformed or changed")
    if manifest.get("campaign_status") != "OBSERVED" or manifest.get("integrity_failures") != []:
        failures.append("Observer campaign did not finish with intact source and capture identities")
    frequency = manifest.get("qpc_frequency")
    if not isinstance(frequency, int) or isinstance(frequency, bool) or frequency <= 0:
        failures.append("Manifest monotonic clock frequency is missing or invalid")

    records = manifest.get("runs")
    if not isinstance(records, list) or not records:
        records = []
        failures.append("Campaign contains no run records")
    if len(records) < 5:
        failures.append("At least five independent runs are required; a short/single run cannot close REC2")
    identities: set[tuple[int, int]] = set()
    for entry in records:
        run_failures: list[str] = []
        run_id = entry.get("run_id") if isinstance(entry, dict) else None
        try:
            if not isinstance(entry, dict):
                raise ValueError("Run manifest entry must be an object")
            record = json.loads((directory / entry["observations"]).read_text(encoding="utf-8-sig"))
            if not isinstance(record, dict):
                raise ValueError("Run observations root must be an object")
            exit_code = record.get("exit_code")
            if (record.get("status") != "OBSERVED" or record.get("timed_out") is not False
                    or not isinstance(exit_code, int) or isinstance(exit_code, bool) or exit_code != 0):
                run_failures.append("Native process failed, timed out or did not exit normally")
            if (record.get("process_started") is not True
                    or record.get("native_stdout_truncated") is not False
                    or record.get("native_stderr_truncated") is not False):
                run_failures.append("Native process start or complete bounded output capture is unproven")
            if record.get("variant") != manifest.get("variant"):
                run_failures.append("Campaign variants cannot be mixed")
            pid, start_ticks = record.get("pid"), record.get("start_utc_ticks")
            if (not isinstance(pid, int) or isinstance(pid, bool) or pid <= 0
                    or not isinstance(start_ticks, int) or isinstance(start_ticks, bool) or start_ticks <= 0):
                run_failures.append("Process PID/lifetime identity is missing or invalid")
                identity = None
            else:
                identity = (pid, start_ticks)
                if identity in identities:
                    run_failures.append("Process lifetime was reused across independent runs")
                identities.add(identity)

            stdout_path, stderr_path = directory / entry["stdout"], directory / entry["stderr"]
            if sha256(stdout_path) != record.get("native_stdout_sha256") or sha256(stderr_path) != record.get("native_stderr_sha256"):
                run_failures.append("Native output digest mismatch")
            lines = [line.removeprefix("REC2_RESULT ") for line in stdout_path.read_text(encoding="utf-8-sig").splitlines() if line.startswith("REC2_RESULT ")]
            if len(lines) != 1:
                raise ValueError("Exactly one bounded native REC2_RESULT is required")
            native = json.loads(lines[0])
            if not isinstance(native, dict):
                raise ValueError("Native REC2_RESULT root must be an object")
            if (not isinstance(native.get("schema_version"), int)
                    or isinstance(native.get("schema_version"), bool)
                    or native.get("schema_version") != 1
                    or native.get("task") != "MP-REC2" or native.get("contract") != "DC-MP-REC2"):
                run_failures.append("Native task/schema/contract mismatch")
            native_pid = native.get("pid")
            if not isinstance(native_pid, int) or isinstance(native_pid, bool) or native_pid != pid:
                run_failures.append("Native and observed PID mismatch")
            if (native.get("variant") not in ("shared", "independent", "distinct", "windows2")
                    or not isinstance(native.get("recovery"), bool)
                    or native.get("variant") != manifest.get("variant")
                    or native.get("recovery") != manifest.get("recovery")):
                run_failures.append("Native workload variant/recovery does not match the observer")
            native_start = native.get("run_start_unix_ms")
            launch = record.get("launch_utc")
            launch_ms = None
            clock_offset_us = None
            if (not isinstance(native_start, (int, float)) or isinstance(native_start, bool)
                    or not math.isfinite(native_start) or not isinstance(launch, str)):
                run_failures.append("Native and observer wall-clock anchors are missing")
            else:
                try:
                    launch_ms = datetime.fromisoformat(launch.replace("Z", "+00:00")).timestamp() * 1000
                    if not math.isfinite(launch_ms) or abs(launch_ms - native_start) > 10_000:
                        run_failures.append("Native run start does not bind to observer launch time")
                    else:
                        clock_offset_us = (launch_ms - native_start) * 1000
                except (ValueError, OverflowError):
                    run_failures.append("Observer launch wall-clock anchor is invalid")

            start, end = native.get("stress_start_elapsed_us"), native.get("stress_end_elapsed_us")
            if (not isinstance(start, int) or isinstance(start, bool) or not isinstance(end, int)
                    or isinstance(end, bool) or end <= start or end - start < 60_000_000):
                run_failures.append("Actual native stress window is shorter than 60 seconds or unobserved")
                start, end = -1, -1
            native_samples = native.get("samples")
            if not isinstance(native_samples, list) or not native_samples:
                native_samples = []
                run_failures.append("Native sample ledger is missing or empty")
            if len(native_samples) > 64:
                run_failures.append("Native sample ledger exceeds the 64-observation campaign bound")
            if any(not isinstance(sample, dict) for sample in native_samples):
                run_failures.append("Native sample ledger contains a non-object sample")
                native_samples = [s for s in native_samples if isinstance(s, dict)]
            required_sample_fields = (
                "elapsed_us", "phase", "step", "action", "result", "windows",
                "viewer_open", "compare_open", "device_generation", "device_state",
                "counters", "views", "identities",
            )
            if any(any(key not in sample for key in required_sample_fields) for sample in native_samples):
                run_failures.append("Native sample schema is missing required observation fields")
            if any(not isinstance(s.get("phase"), str) or not isinstance(s.get("action"), str)
                   or not isinstance(s.get("result"), str) or not isinstance(s.get("counters"), dict)
                   or not isinstance(s.get("views"), list) or not isinstance(s.get("identities"), list)
                   or len(s.get("views")) != 3 or len(s.get("identities")) != 6
                   or not isinstance(s.get("device_state"), str)
                   or any(not isinstance(v, dict) or any(k not in v for k in (
                       "scene_serial", "view_serial", "phase", "complete_in_scene",
                       "current_origin", "retained_origin", "retiring_frames",
                   )) or any(not isinstance(v.get(k), int) or isinstance(v.get(k), bool) for k in (
                       "scene_serial", "view_serial", "phase", "retiring_frames",
                   )) or not isinstance(v.get("complete_in_scene"), bool)
                   or any(value is not None and (not isinstance(value, int) or isinstance(value, bool))
                          for value in (v.get("current_origin"), v.get("retained_origin")))
                   for v in s.get("views", []))
                   or any(value is not None and
                          (not isinstance(value, str) or len(value) != 64
                           or any(ch not in "0123456789abcdef" for ch in value.lower()))
                          for value in s.get("identities", []))
                   or not isinstance(s.get("step"), int) or isinstance(s.get("step"), bool)
                   or not isinstance(s.get("windows"), int) or isinstance(s.get("windows"), bool)
                   or not isinstance(s.get("viewer_open"), bool)
                   or not isinstance(s.get("compare_open"), bool)
                   or (s.get("device_generation") is not None and
                       (not isinstance(s.get("device_generation"), int) or isinstance(s.get("device_generation"), bool)))
                   for s in native_samples):
                run_failures.append("Native sample schema has invalid phase/action/result or observation types")
            if any(not isinstance(s.get("elapsed_us"), int) or isinstance(s.get("elapsed_us"), bool) for s in native_samples):
                run_failures.append("Native sample elapsed clock is missing or invalid")
            elif any(b["elapsed_us"] <= a["elapsed_us"] for a, b in zip(native_samples, native_samples[1:])):
                run_failures.append("Native sample clock is not strictly ordered")
            stress = [
                s for s in native_samples
                if s.get("phase") == "stress"
                and isinstance(s.get("elapsed_us"), int)
                and not isinstance(s.get("elapsed_us"), bool)
                and start <= s["elapsed_us"] <= end
            ]
            if len(stress) < 60:
                run_failures.append("Fewer than 60 stress action observations")

            valid_process = []
            missing_process = 0
            qpc_frequency = record.get("qpc_frequency")
            launch_qpc = record.get("launch_qpc")
            observer_clock_valid = (
                qpc_frequency == frequency and isinstance(qpc_frequency, int)
                and not isinstance(qpc_frequency, bool) and qpc_frequency > 0
                and isinstance(launch_qpc, int) and not isinstance(launch_qpc, bool)
            )
            if not observer_clock_valid:
                run_failures.append("Observer QPC clock is not bound to the manifest clock")
            process_rows = record.get("samples")
            if not isinstance(process_rows, list):
                run_failures.append("Observer process sample ledger is missing or malformed")
                process_rows = []
            qpc_starts = [s.get("qpc_start") for s in process_rows if isinstance(s, dict)]
            ordered_qpc = [q for q in qpc_starts if isinstance(q, int) and not isinstance(q, bool)]
            if (len(qpc_starts) != len(process_rows) or len(ordered_qpc) != len(qpc_starts)
                    or any(b <= a for a, b in zip(ordered_qpc, ordered_qpc[1:]))):
                run_failures.append("Observer process QPC sample clock is missing or not strictly ordered")
            for sample in process_rows:
                if not isinstance(sample, dict) or identity is None:
                    run_failures.append("Process sample identity is malformed")
                    continue
                if sample.get("pid") != identity[0] or sample.get("start_utc_ticks") != identity[1]:
                    run_failures.append("Sample PID/lifetime mismatch")
                    continue
                if not sample.get("alive_after") or sample.get("identity_after_ticks") != identity[1]:
                    missing_process += 1
                    continue
                qpc_start, qpc_end = sample.get("qpc_start"), sample.get("qpc_end")
                if not observer_clock_valid:
                    continue
                if (not isinstance(qpc_start, int) or isinstance(qpc_start, bool)
                        or not isinstance(qpc_end, int) or isinstance(qpc_end, bool)
                        or qpc_end < qpc_start):
                    run_failures.append("Process sample monotonic clock is invalid")
                    continue
                if clock_offset_us is None:
                    continue
                if (not isinstance(launch_qpc, int) or isinstance(launch_qpc, bool)
                        or not isinstance(qpc_frequency, int) or isinstance(qpc_frequency, bool)
                        or qpc_frequency <= 0):
                    continue
                relative_us = (qpc_start - launch_qpc) / qpc_frequency * 1_000_000 + clock_offset_us
                if start <= relative_us <= end:
                    valid_process.append((relative_us / 1_000_000, sample))
            if len(valid_process) < 50:
                run_failures.append("Insufficient PID-bound process samples in native stress window")
            resources = {}
            for key in ("private_bytes", "working_set_bytes"):
                resources[key] = gauge_trend([(t, int(s[key])) for t, s in valid_process if isinstance(s.get(key), int) and s[key] >= 0])
            for key in ("dedicated_bytes", "shared_bytes", "committed_bytes"):
                resources["gpu_" + key] = gauge_trend([(t, int(s["gpu"][key])) for t, s in valid_process if isinstance(s.get("gpu"), dict) and s["gpu"].get("status") == "OBSERVED" and isinstance(s["gpu"].get(key), int) and s["gpu"][key] >= 0])

            counter_keys = ("cpu_retained_bytes", "decoded_retained_bytes", "packed_retained_bytes", "staging_reservation_bytes", "in_flight_reservation_bytes", "resident_reservation_bytes", "quarantined_payload_bytes")
            for key in counter_keys:
                resources["application_" + key] = gauge_trend([
                    (s["elapsed_us"] / 1_000_000, int(s["counters"][key]))
                    for s in stress if isinstance(s.get("counters"), dict)
                    and isinstance(s["counters"].get(key), int) and s["counters"][key] >= 0
                ])
            required_counters = (
                "retained_frames", "backup_claims", "backup_origins", "quarantined_frames",
                "pool_count", "consumers", "consumer_unknown_frames", "quarantine_capacity_bytes",
                *counter_keys,
            )
            for sample in native_samples:
                counters = sample.get("counters", {})
                if (not isinstance(counters, dict) or any(
                        not isinstance(counters.get(key), int) or isinstance(counters.get(key), bool)
                        or counters[key] < 0 for key in required_counters)):
                    run_failures.append("Native sample resource counters are missing or invalid")
                    continue
                if isinstance(counters.get("quarantined_payload_bytes"), int) and isinstance(counters.get("quarantine_capacity_bytes"), int) and counters["quarantined_payload_bytes"] > counters["quarantine_capacity_bytes"]:
                    run_failures.append("Quarantine charge exceeded the configured process bound")
                if isinstance(counters.get("cpu_retained_bytes"), int) and counters["cpu_retained_bytes"] > 64 * 1024 * 1024:
                    run_failures.append("Retained CPU payload exceeded the existing 64 MiB implementation bound")
                if all(isinstance(counters.get(k), int) for k in ("decoded_retained_bytes", "packed_retained_bytes", "cpu_retained_bytes")) and counters["decoded_retained_bytes"] + counters["packed_retained_bytes"] != counters["cpu_retained_bytes"]:
                    run_failures.append("Distinct decoded/packed accounting does not reconcile")

            if any(not isinstance(s.get("action"), str) or not isinstance(s.get("result"), str) for s in stress):
                run_failures.append("Stress samples require string action/result evidence")
            actions: dict[str, dict] = {}
            for sample in stress:
                action = sample.get("action", "<missing>")
                result = sample.get("result", "<missing>")
                summary = actions.setdefault(action, {
                    "observations": 0, "scene_complete_observations": 0,
                    "pending_observations": 0, "refused_observations": 0,
                    "dispatch_only_observations": 0, "results": {},
                    "scene_observations": [],
                })
                summary["observations"] += 1
                summary["results"][result] = summary["results"].get(result, 0) + 1
                if result == "scene_complete":
                    summary["scene_complete_observations"] += 1
                elif result == "pending_or_unavailable":
                    summary["pending_observations"] += 1
                elif isinstance(result, str) and result.startswith("refused"):
                    summary["refused_observations"] += 1
                elif result == "requested":
                    summary["dispatch_only_observations"] += 1
                if result in ("scene_complete", "pending_or_unavailable"):
                    summary["scene_observations"].append({
                        "elapsed_us": sample["elapsed_us"], "result": result,
                        "views": sample.get("views"),
                        "note": "Scene state observed after bounded action delay; not GPU completion",
                    })
            post_close = [s for s in native_samples if s.get("phase") == "post_close_teardown"]
            peer_closed = [s for s in native_samples if s.get("phase") == "peer_closed"]
            final = post_close[-1] if post_close and native_samples[-1] is post_close[-1] else None
            reconciliation = {
                "status": "HOLD", "post_close_samples": len(post_close),
                "peer_closed_samples": len(peer_closed),
                "reason": "Missing final post-close teardown observation; physical release endpoints remain NOT_MEASURED",
            }
            if final and isinstance(final.get("counters"), dict):
                c = final["counters"]
                unknown, consumers = c.get("consumer_unknown_frames"), c.get("consumers")
                ownership_keys = ("retained_frames", "backup_claims", "backup_origins")
                known_empty_ownership = all(c.get(k) == 0 for k in ownership_keys)
                resource_keys = ("pool_count", "staging_reservation_bytes",
                                 "in_flight_reservation_bytes", "resident_reservation_bytes",
                                 "quarantined_payload_bytes", "cpu_retained_bytes")
                known_empty_resources = all(c.get(k) == 0 for k in resource_keys)
                known_consumers = (isinstance(consumers, int) and not isinstance(consumers, bool) and consumers == 0
                                   and isinstance(unknown, int) and not isinstance(unknown, bool) and unknown == 0)
                views = final.get("views")
                retirement_known = isinstance(views, list) and len(views) == 3 and all(
                    isinstance(v, dict) and isinstance(v.get("retiring_frames"), int)
                    and not isinstance(v.get("retiring_frames"), bool) for v in views
                )
                retiring = sum(v["retiring_frames"] for v in views) if retirement_known and views is not None else None
                final_device_ready = final.get("device_state") == "ready"
                final_scene_closed = final.get("compare_open") is False and final.get("viewer_open") is False and final.get("windows") == 1
                final_references_empty = (
                    retirement_known and final.get("identities") == [None] * 6
                    and all(v.get("current_origin") is None and v.get("retained_origin") is None
                            and v.get("complete_in_scene") is False for v in views)
                )
                consumer_release = bool(
                    known_consumers and known_empty_ownership and retiring == 0
                    and final_references_empty and len(peer_closed) > 0 and final_scene_closed
                    and final_device_ready and not run_failures
                )
                reconciliation = {
                    "status": "HOLD", "post_close_samples": len(post_close),
                    "peer_closed_samples": len(peer_closed),
                    "consumers": consumers, "consumer_unknown_frames": unknown,
                    "backup_claims": c.get("backup_claims"), "backup_origins": c.get("backup_origins"),
                    "retained_frames": c.get("retained_frames"), "pool_count": c.get("pool_count"),
                    "staging_reservation_bytes": c.get("staging_reservation_bytes"),
                    "in_flight_reservation_bytes": c.get("in_flight_reservation_bytes"),
                    "resident_reservation_bytes": c.get("resident_reservation_bytes"),
                    "quarantined_payload_bytes": c.get("quarantined_payload_bytes"),
                    "retiring_frames": retiring, "final_scene_closed": final_scene_closed,
                    "final_device_state": final.get("device_state"),
                    "consumer_release_observed": consumer_release,
                    "application_resources_empty": bool(consumer_release and known_empty_resources),
                    "cpu_retained_bytes": c.get("cpu_retained_bytes"),
                    "physical_atlas_release": "NOT_MEASURED", "gpu_completion": "NOT_MEASURED",
                    "reason": "Post-close consumer, backup, registry and retirement counters are observations only; unknown consumers or missing endpoints cannot count as release",
                }
            runs.append({
                "run_id": run_id, "pair_id": record.get("pair_id"),
                "scenario_id": record.get("scenario_id"), "variant": record.get("variant"),
                "status": "FAILED" if run_failures else "OBSERVED", "reason": "Measured subset; full resource-lifetime acceptance remains HOLD",
                "endpoints": manifest.get("endpoints", {}),
                "metrics": resources,
                "resources": {"first": native_samples[0].get("counters") if native_samples else None, "last": native_samples[-1].get("counters") if native_samples else None},
                "actions": actions, "post_close_reconciliation": reconciliation,
                "frames": {"status": "NOT_MEASURED", "reason": "Scene serial is not GPU Present or scanout"},
                "instrumentation": {"process_samples": len(record.get("samples", [])), "stress_process_samples": len(valid_process), "censored_exit_samples": missing_process, "native_samples": len(native_samples), "stress_samples": len(stress), "on_off_overhead": "NOT_MEASURED"},
                "failures": sorted(set(run_failures)), "outliers": [],
                "evidence": {"observations_sha256": sha256(directory / entry["observations"]), "native_stdout_sha256": sha256(stdout_path), "native_stderr_sha256": sha256(stderr_path)},
            })
        except (OSError, UnicodeError, KeyError, TypeError, ValueError, json.JSONDecodeError) as exc:
            run_failures.append(f"Run evidence is incomplete or malformed: {exc}")
            runs.append({"run_id": run_id, "status": "FAILED", "failures": sorted(set(run_failures))})

    if any(run["status"] == "FAILED" for run in runs):
        failures.append("One or more independent runs failed evidence validation")
    if not runs:
        failures.append("No valid native run evidence was analyzed")
    return {
        "schema_version": 1, "task": "MP-REC2", "contract": "DC-MP-REC2",
        "status": "FAILED" if failures else "OBSERVED",
        "acceptance": "HOLD",
        "reason": "No supported public per-atlas allocation/removal or GPU completion endpoint; gauge trends and bounded refusal do not prove full residency/release acceptance",
        "manifest_sha256": sha256(manifest_path),
        "campaign_identity": {
            "source_commit": manifest.get("source_commit"),
            "executable_sha256": manifest.get("executable_sha256"),
            "driver_sha256": manifest.get("driver_sha256"),
            "qpc_frequency": frequency,
        },
        "runs": runs, "failures": failures,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    result = analyze_campaign(args.directory)
    text = json.dumps(result, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        with args.output.open("x", encoding="utf-8") as stream:
            stream.write(text)
        print(json.dumps({"status": result["status"], "acceptance": result["acceptance"], "runs": len(result["runs"]), "failures": result["failures"]}))
    else:
        print(text, end="")
    return int(result["status"] == "FAILED")


if __name__ == "__main__":
    raise SystemExit(main())
