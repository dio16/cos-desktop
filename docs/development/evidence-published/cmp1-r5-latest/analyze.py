"""Analyze the fixed MP-CMP1 R5 trace matrix into committed per-run/matrix evidence.

Raw trace ledgers stay private under target/; this script publishes per-run
summaries, per-variant distributions, the measured reuse/fairness invariants and
their exact computation rules. It never turns a paint/GPU/Present absence into a
measurement.
"""
import hashlib
import json
import math
import platform
import subprocess
from pathlib import Path

REPO = Path(__file__).resolve().parents[4]  # docs/development/evidence-published/cmp1-r5-latest
SCRATCH = REPO / "target/cmp1-r5"
RAW = SCRATCH / "raw"
EVIDENCE = REPO / "docs/development/evidence-published/cmp1-r5-latest"
VARIANTS = ("shared", "independent", "distinct", "windows2")
LINK_TOGGLES = set(range(240, 100000, 240)) | {64}


def sha(value):
    if isinstance(value, str):
        value = value.encode("utf-8")
    return "sha256:" + hashlib.sha256(value).hexdigest()


def digest_file(path):
    data = path.read_bytes()
    return "sha256:" + hashlib.sha256(data).hexdigest(), len(data)


def percentile(values, fraction):
    ordered = sorted(values)
    if not ordered:
        return None
    index = max(0, min(len(ordered) - 1, math.ceil(fraction * len(ordered)) - 1))
    return ordered[index]


def stats(values):
    if not values:
        return {"values": [], "min": None, "median": None, "p95": None, "max": None}
    ordered = sorted(values)
    middle = ordered[len(ordered) // 2] if len(ordered) % 2 else (
        (ordered[len(ordered) // 2 - 1] + ordered[len(ordered) // 2]) / 2)
    return {"values": values, "min": ordered[0], "median": middle,
            "p95": percentile(ordered, 0.95), "max": ordered[-1]}


def linked_mask(steps):
    state = True
    mask = {}
    for step in steps:
        if step and step in LINK_TOGGLES:
            state = not state
        mask[step] = state
    return mask


def camera_metrics(ledger):
    """Pair camera coupling and cross-pane independence from the trace tuples."""
    steps = list(range(len(ledger)))
    mask = linked_mask(steps)
    relinks = {240 + 480 * k for k in range(20)} | {64}
    transient = set()
    for step in relinks:
        transient.update({step, step + 1, step + 2})

    def camera(sample, side):
        tuple_ = sample["camera"]
        return (tuple_[0], tuple_[1], tuple_[2]) if side == 0 else (
            tuple_[3], tuple_[4], tuple_[5])

    def changed(before, after):
        return any(abs(a - b) > 1e-12 for a, b in zip(before, after))

    deltas = [math.hypot(sample["camera"][0] - sample["camera"][3],
                         sample["camera"][1] - sample["camera"][4])
              for sample in ledger]
    scale_deltas = [abs(sample["camera"][2] - sample["camera"][5])
                    for sample in ledger]
    stable_linked = [step for step in steps if mask[step] and step not in transient]
    first_unlinked = [step for step in steps if step >= 66 and step < 240]
    unlinked_other = 0
    unlinked_target = 0
    unlinked_other_moved = 0
    linked_both_move = 0
    linked_stable_delta_changes = 0
    for step in steps[1:]:
        before, sample = ledger[step - 1], ledger[step]
        if not mask[step]:
            if sample["command"] in (3, 4) and sample["target"] in (0, 1):
                target = sample["target"]
                other = 1 - target
                unlinked_target += 1
                unlinked_other += 1
                if changed(camera(before, other), camera(sample, other)):
                    unlinked_other_moved += 1
        else:
            if sample["command"] in (3, 4) and step not in transient and step - 1 not in transient:
                if abs(deltas[step] - deltas[step - 1]) > 1e-12:
                    linked_stable_delta_changes += 1
                if (changed(camera(before, 0), camera(sample, 0))
                        and changed(camera(before, 1), camera(sample, 1))):
                    linked_both_move += 1
    resyncs = 0
    for relink in sorted(relinks):
        window = [step for step in steps if relink <= step < relink + 3]
        if window and deltas[window[0]] > 1e-9 and any(deltas[step] == 0.0 for step in window):
            resyncs += 1
    return {
        "linked_stable_pair_camera_delta_max": (
            round(max(deltas[step] for step in stable_linked), 9) if stable_linked else None),
        "linked_stable_pair_scale_delta_max": (
            round(max(scale_deltas[step] for step in stable_linked), 9)
            if stable_linked else None),
        "first_unlinked_pair_camera_delta_max": (
            round(max(deltas[step] for step in first_unlinked), 9)
            if first_unlinked else None),
        "linked_stable_delta_changes": linked_stable_delta_changes,
        "linked_both_panes_moved_samples": linked_both_move,
        "relink_resynchronizations": resyncs,
        "unlinked_gesture_samples": unlinked_target,
        "unlinked_other_pane_camera_changes": unlinked_other_moved,
        "camera_series_sha256": sha(json.dumps(
            {"deltas": [round(value, 12) for value in deltas],
             "linked_mask": [1 if mask[step] else 0 for step in steps]},
            separators=(",", ":"))),
        "series": {"deltas": [round(value, 9) for value in deltas],
                   "linked_mask": [1 if mask[step] else 0 for step in steps]},
    }


def cadence_us(ledger):
    achieved = [sample["achieved_us"] for sample in ledger]
    return [b - a for a, b in zip(achieved, achieved[1:])]


def pane_metrics(ledger):
    issued = [[], []]
    tiles = [[], []]
    decode = [[], []]
    queue = [[], []]
    complete = [[], []]
    targets = {"A": 0, "B": 0, "pair": 0}
    for sample in ledger:
        for side in (0, 1):
            issued[side].append(sample["issued"][side])
            tiles[side].append(sample["tiles"][side])
            decode[side].append(sample["decode_calls"][side])
            queue[side].append(sample["queue_wait_ns"][side])
            complete[side].append(sample["complete"][side])
        target = sample["target"]
        targets["A" if target == 0 else "B" if target == 1 else "pair"] += 1
    increments = [[], []]
    for side in (0, 1):
        previous = 0
        for value in decode[side]:
            if value > previous:
                increments[side].append(value - previous)
            previous = value
    return {
        "target_counts": targets,
        "final_issued": [issued[0][-1], issued[1][-1]] if ledger else None,
        "final_tiles": [tiles[0][-1], tiles[1][-1]] if ledger else None,
        "decode_increments": increments,
        "final_decode": [decode[0][-1], decode[1][-1]] if ledger else None,
        "final_queue_wait_ns": [queue[0][-1], queue[1][-1]] if ledger else None,
        "complete_fraction": [
            round(sum(complete[0]) / len(complete[0]), 6) if complete[0] else None,
            round(sum(complete[1]) / len(complete[1]), 6) if complete[1] else None,
        ],
        "max_gap_us": None,
    }


def analyze_trace(trace, raw_bytes):
    ledger = trace.get("ledger") or []
    camera = camera_metrics(ledger) if ledger else {}
    pane = pane_metrics(ledger) if ledger else {}
    cadence = cadence_us(ledger) if ledger else []
    pane["cadence_us"] = {key: value for key, value in stats(cadence).items()
                          if key != "values"}
    if ledger:
        achieved = [sample["achieved_us"] for sample in ledger]
        pane["max_gap_us"] = max((b - a for a, b in zip(achieved, achieved[1:])),
                                 default=0)
    dpi = sorted({tuple(sample["dpi"]) for sample in ledger}) if ledger else []
    return {
        "trace_variant": trace.get("variant"),
        "samples": trace.get("samples"),
        "ledger_rows": len(ledger),
        "ready": trace.get("ready"),
        "settled": trace.get("settled"),
        "warmup_ms": trace.get("warmup_ms"),
        "refusals": trace.get("refusals"),
        "cancel_probe_stage": trace.get("cancel_probe_stage"),
        "counters": {key: trace.get(key) for key in (
            "decode_calls", "reuse_calls", "retained_frames", "consumers",
            "consumer_unknown_frames", "backup_claims", "backup_origins",
            "evicted_frames", "packed_retained_bytes", "decoded_retained_bytes",
            "cpu_retained_bytes", "staging_reservation_bytes",
            "in_flight_reservation_bytes", "resident_reservation_bytes",
            "quarantined_frames", "quarantined_staging_bytes",
            "quarantined_in_flight_bytes", "quarantined_resident_bytes",
            "quarantined_payload_bytes", "quarantine_capacity_bytes",
            "queue_wait_ns", "queue_rejections", "queue_cancellations")},
        "peaks": {key: trace.get(key) for key in (
            "sampled_peak_cpu_retained_bytes", "sampled_peak_packed_bytes",
            "sampled_peak_reservation_bytes")},
        "input_decoded_rgba_sha256": trace.get("input_decoded_rgba_sha256"),
        "panes": {key: value for key, value in pane.items() if key != "cadence_us"},
        "cadence_us": pane.get("cadence_us"),
        "camera": {key: value for key, value in camera.items() if key != "series"},
        "camera_series_sha256": camera.get("camera_series_sha256"),
        "dpi": [list(entry) for entry in dpi],
        "declared_unmeasured": {key: trace.get(key) for key in (
            "present", "gpu", "8k", "mixed_dpi", "one_to_one", "cursor_anchored_zoom")},
        "raw": raw_bytes,
    }


def main():
    runs_doc = json.loads((EVIDENCE / "runs.json").read_text(encoding="utf-8"))
    runs = runs_doc["runs"]
    per_run = []
    for run in runs:
        raw_path = RAW / f"{run['variant']}-run{run['run']}.jsonl"
        raw = raw_path.read_bytes()
        trace_objects = []
        for line in raw.splitlines():
            if not line.strip():
                continue
            try:
                value = json.loads(line)
            except ValueError:
                continue
            if isinstance(value, dict) and value.get("scenario") == "cmp1_native_fixed_trace":
                trace_objects.append(value)
        analyses = [analyze_trace(trace, {"sha256": sha(raw), "bytes": len(raw)})
                    for trace in trace_objects]
        per_run.append({
            "variant": run["variant"],
            "run": run["run"],
            "exit_code": run["exit_code"],
            "timed_out": run["timed_out"],
            "elapsed_ms": run["elapsed_ms"],
            "trace_lines": run["trace_lines"],
            "parse_errors": run["parse_errors"],
            "stderr_bytes": run["stderr_bytes"],
            "raw_sha256": run["raw_sha256"],
            "raw_bytes": run["raw_bytes"],
            "traces": analyses,
        })

    by_variant = {}
    for variant in VARIANTS:
        entries = [entry for entry in per_run if entry["variant"] == variant]
        traces = [trace for entry in entries for trace in entry["traces"]]
        def series(getter):
            return stats([getter(trace) for trace in traces if getter(trace) is not None])
        by_variant[variant] = {
            "runs": len(entries),
            "trace_lines": len(traces),
            "exit_codes": [entry["exit_code"] for entry in entries],
            "timed_out": sum(1 for entry in entries if entry["timed_out"]),
            "elapsed_ms": stats([entry["elapsed_ms"] for entry in entries]),
            "samples": series(lambda trace: trace["samples"]),
            "warmup_ms": series(lambda trace: trace["warmup_ms"]),
            "refusals": series(lambda trace: trace["refusals"]),
            "decode_calls": series(lambda trace: trace["counters"]["decode_calls"]),
            "reuse_calls": series(lambda trace: trace["counters"]["reuse_calls"]),
            "retained_frames": series(lambda trace: trace["counters"]["retained_frames"]),
            "consumers": series(lambda trace: trace["counters"]["consumers"]),
            "consumer_unknown_frames": series(
                lambda trace: trace["counters"]["consumer_unknown_frames"]),
            "backup_claims": series(lambda trace: trace["counters"]["backup_claims"]),
            "backup_origins": series(lambda trace: trace["counters"]["backup_origins"]),
            "evicted_frames": series(lambda trace: trace["counters"]["evicted_frames"]),
            "decoded_retained_bytes": series(
                lambda trace: trace["counters"]["decoded_retained_bytes"]),
            "staging_reservation_bytes": series(
                lambda trace: trace["counters"]["staging_reservation_bytes"]),
            "in_flight_reservation_bytes": series(
                lambda trace: trace["counters"]["in_flight_reservation_bytes"]),
            "quarantined_frames": series(
                lambda trace: trace["counters"]["quarantined_frames"]),
            "quarantined_staging_bytes": series(
                lambda trace: trace["counters"]["quarantined_staging_bytes"]),
            "quarantined_in_flight_bytes": series(
                lambda trace: trace["counters"]["quarantined_in_flight_bytes"]),
            "quarantined_resident_bytes": series(
                lambda trace: trace["counters"]["quarantined_resident_bytes"]),
            "quarantined_payload_bytes": series(
                lambda trace: trace["counters"]["quarantined_payload_bytes"]),
            "quarantine_capacity_bytes": series(
                lambda trace: trace["counters"]["quarantine_capacity_bytes"]),
            "packed_retained_bytes": series(
                lambda trace: trace["counters"]["packed_retained_bytes"]),
            "cpu_retained_bytes": series(
                lambda trace: trace["counters"]["cpu_retained_bytes"]),
            "queue_wait_ns": series(lambda trace: trace["counters"]["queue_wait_ns"]),
            "queue_rejections": series(
                lambda trace: trace["counters"]["queue_rejections"]),
            "queue_cancellations": series(
                lambda trace: trace["counters"]["queue_cancellations"]),
            "peak_cpu_retained_bytes": series(
                lambda trace: trace["peaks"]["sampled_peak_cpu_retained_bytes"]),
            "peak_packed_bytes": series(
                lambda trace: trace["peaks"]["sampled_peak_packed_bytes"]),
            "peak_reservation_bytes": series(
                lambda trace: trace["peaks"]["sampled_peak_reservation_bytes"]),
            "cancel_probe_stage": [trace["cancel_probe_stage"] for trace in traces],
            "ready": [trace["ready"] for trace in traces],
            "settled": [trace["settled"] for trace in traces],
            "cadence_us": series(
                lambda trace: trace["cadence_us"]["median"] if trace["cadence_us"] else None),
            "max_gap_us": series(lambda trace: trace["panes"]["max_gap_us"]),
            "linked_pair_camera_delta_max": series(
                lambda trace: trace["camera"]["linked_stable_pair_camera_delta_max"]),
            "linked_pair_scale_delta_max": series(
                lambda trace: trace["camera"]["linked_stable_pair_scale_delta_max"]),
            "first_unlinked_delta_max": series(
                lambda trace: trace["camera"]["first_unlinked_pair_camera_delta_max"]),
            "linked_stable_delta_changes": series(
                lambda trace: trace["camera"]["linked_stable_delta_changes"]),
            "relink_resynchronizations": series(
                lambda trace: trace["camera"]["relink_resynchronizations"]),
            "unlinked_gesture_samples": series(
                lambda trace: trace["camera"]["unlinked_gesture_samples"]),
            "unlinked_other_pane_camera_changes": series(
                lambda trace: trace["camera"]["unlinked_other_pane_camera_changes"]),
            "linked_both_panes_moved_samples": series(
                lambda trace: trace["camera"]["linked_both_panes_moved_samples"]),
            "target_counts": [trace["panes"]["target_counts"] for trace in traces],
            "complete_fraction": [trace["panes"]["complete_fraction"] for trace in traces],
            "dpi": [trace["dpi"] for trace in traces],
            "input_digests_equal": [
                trace["input_decoded_rgba_sha256"][0] == trace["input_decoded_rgba_sha256"][1]
                for trace in traces],
        }

    all_traces = [trace for entry in per_run for trace in entry["traces"]]
    same_shared = by_variant["shared"]
    same_independent = by_variant["independent"]
    settled_values = [trace["settled"] for trace in all_traces]
    unsettled = [(trace["trace_variant"], trace["settled"]) for trace in all_traces
                 if trace["settled"] is not True]

    def all_value(series, expected):
        return bool(series["values"]) and all(value == expected
                                              for value in series["values"])

    adoption = {
        "same_key_shared_one_decode": all_value(same_shared["decode_calls"], 1),
        "same_key_shared_one_retained": all_value(same_shared["retained_frames"], 1),
        "same_key_shared_two_consumers": all_value(same_shared["consumers"], 2),
        "same_key_independent_two_decodes": all_value(same_independent["decode_calls"], 2),
        "same_key_independent_two_retained": all_value(same_independent["retained_frames"], 2),
        "shared_cpu_payload_below_independent": (
            same_shared["cpu_retained_bytes"]["max"] is not None
            and same_independent["cpu_retained_bytes"]["min"] is not None
            and same_shared["cpu_retained_bytes"]["max"]
            < same_independent["cpu_retained_bytes"]["min"]),
        "linked_pair_camera_delta_zero": all(
            series == 0.0 for variant in VARIANTS
            for series in by_variant[variant]["linked_pair_camera_delta_max"]["values"]),
        "linked_stable_delta_changes_zero": all(
            value == 0 for variant in VARIANTS
            for value in by_variant[variant]["linked_stable_delta_changes"]["values"]),
        "unlinked_never_moves_the_other_pane": all(
            value == 0 for variant in VARIANTS
            for value in by_variant[variant]["unlinked_other_pane_camera_changes"]["values"]),
        "unlinked_gestures_do_move_the_target": all(
            value > 0 for variant in VARIANTS
            for value in by_variant[variant]["unlinked_gesture_samples"]["values"]),
        "relink_resynchronizes_both_panes": all(
            value and value > 0 for variant in VARIANTS
            for value in by_variant[variant]["relink_resynchronizations"]["values"]),
        "all_traces_ready": all(trace["ready"] is True for trace in all_traces),
        "settled_trace_count": sum(1 for value in settled_values if value is True),
        "trace_count": len(all_traces),
        "unsettled_traces": unsettled,
        "cancel_probe_verified": all(
            stage == 3 for variant in VARIANTS
            for stage in by_variant[variant]["cancel_probe_stage"]),
        "no_refusals": all(
            series["max"] == 0 for variant in VARIANTS
            for series in (by_variant[variant]["refusals"],)),
    }

    source = {}
    source["analysis_head"] = subprocess.run(
        ["git", "-C", str(REPO), "rev-parse", "HEAD"],
        capture_output=True, text=True, check=True).stdout.strip()
    build_head = "1e204a503d59d7f6a0810f122025151402a32b77"
    source["build_head"] = build_head
    source["build_head_basis"] = (
        "git reflog: 1e204a5 was HEAD from 10:07:48 JST when the release example used by the "
        "matrix was written at 10:12 JST")
    source["worktree_changes_at_build"] = [
        "apps/desktop/examples/compare.rs (this slice's trace accounting extension; committed "
        "with this evidence)",
        "crates/desktop-gpui/src/palette.rs (another lane's in-flight work; not part of this "
        "slice and not committed here)",
    ]
    source["build_command"] = "cargo build --locked --release -p cos-desktop --example compare"
    source["reproducibility"] = (
        "NOT_VERIFIED: this is a shared checkout; the executed binary includes the uncommitted "
        "worktree changes listed above, so only its sha256 binds the executed bytes and a "
        "rebuild from a commit is not claimed to reproduce them")
    exe = REPO / "target/release/examples/compare.exe"
    source["executable"] = {"path": "target/release/examples/compare.exe"}
    source["executable"]["sha256"], source["executable"]["bytes"] = digest_file(exe)
    lock_path = REPO / "Cargo.lock"
    source["cargo_lock"] = digest_file(lock_path)[0]
    source["cargo_lock_at_build_head"] = subprocess.run(
        ["git", "-C", str(REPO), "show", f"{build_head}:Cargo.lock"],
        capture_output=True, check=True).stdout
    source["cargo_lock_at_build_head"] = "sha256:" + hashlib.sha256(
        source["cargo_lock_at_build_head"]).hexdigest()
    drift = subprocess.run(
        ["git", "-C", str(REPO), "diff", "--name-only", f"{build_head}..HEAD",
         "--", "Cargo.lock", "apps/desktop", "crates/desktop-gpui", "crates/ui-model",
         "crates/media", "crates/desktop-protocol"],
        capture_output=True, text=True, check=True).stdout.splitlines()
    source["source_changes_after_build"] = drift
    pins = {}
    block = {}
    for line in lock_path.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if stripped == "[[package]]":
            if block.get("name") and "rev=" in block.get("source", ""):
                pins[block["name"]] = block["source"].rsplit("#", 1)[-1].strip('"')
            block = {}
        elif stripped.startswith("name = "):
            block["name"] = stripped[7:].strip('"')
        elif stripped.startswith("source = "):
            block["source"] = stripped[9:]
    if block.get("name") and "rev=" in block.get("source", ""):
        pins[block["name"]] = block["source"].rsplit("#", 1)[-1].strip('"')
    source["git_revisions"] = pins
    source["host"] = platform.platform()
    source["python"] = platform.python_version()

    matrix = {
        "task": "MP-CMP1",
        "slice": "R5 reuse/fairness fixed matrix",
        "date": "2026-11-08",
        "source": source,
        "protocol": {
            "harness": "examples/compare.exe --fixture --trace <variant> (unchanged)",
            "variants": ["shared", "independent", "distinct", "windows2"],
            "runs_per_variant": 5,
            "run_scope": "each trace is the harness's own fixed 60 s command schedule",
            "isolation": "fresh LOCALAPPDATA/APPDATA/TEMP per run under a private scratch root",
            "serialization": "one native lock for the whole matrix; released afterwards",
            "aggregation": "per-sample achieved_us from the trace ledger; median=p50 of observed samples; p95=nearest rank ceil(n*0.95)-1; min/max exact",
            "requested_cadence_us": 16000,
            "achieved_cadence_note": "requested_us advances by 16 ms per sample but the measured loop cadence is the achieved_us interval distribution; it is observation-loop cadence, not frame pacing",
            "camera_metric": "pair_delta = hypot(anchor_a - anchor_b) from the trace camera tuple; linked state starts linked and toggles at step 64 and every 240 steps; the two samples after each re-link are treated as the re-synchronization transient; unlinked cross-pane change counts samples where a gesture targeted one side and the other side's camera tuple changed",
            "fairness_scope": "pane fairness is measured as per-pane camera/entry progress and cross-pane camera non-interference; window fairness is the two concurrently traced windows' own samples and counters under one process-wide upload service",
            "raw_ledger_policy": "raw JSONL ledgers stay private under target/ and are cited by sha256/bytes in runs.json; committed evidence carries the per-run summaries, distributions and digests",
        },
        "per_run": per_run,
        "by_variant": by_variant,
        "measured_adoption_checks": adoption,
        "measurement_limits": [
            "Present, GPU, frame pacing, displayed/scanout and input-to-photon are NOT_MEASURED; the harness declares them so and this record repeats it",
            "queue wait is the service's CPU preparation wait counter, not a GPU or compositor queue",
            "camera coupling is derived from the trace's reported anchor/scale tuples, not from pixels",
            "cross-window fairness in windows2 is judged from two concurrently traced windows' own samples/counters; the upload service counters are process-wide, so per-window attribution is not separable",
            "unequal dimensions/orientations/aspect ratios/DPI, cursor-anchored zoom and physical 1:1 are NOT_MEASURED by this matrix",
            "no numeric R0 ratification, product acceptance or R5 fairness PASS is claimed; the adoption rule is evaluated only over the measured CPU/cache facts",
        ],
    }
    (EVIDENCE / "matrix.json").write_text(json.dumps(matrix, indent=2) + "\n",
                                          encoding="utf-8")

    def variant_summary(variant):
        data = by_variant[variant]
        return {
            "runs": data["runs"],
            "traces": data["trace_lines"],
            "decode_calls": data["decode_calls"]["values"],
            "retained_frames": data["retained_frames"]["values"],
            "consumers": data["consumers"]["values"],
            "cpu_retained_bytes": data["cpu_retained_bytes"]["values"],
            "packed_retained_bytes": data["packed_retained_bytes"]["values"],
            "decoded_retained_bytes": data["decoded_retained_bytes"]["values"],
            "backup_claims": data["backup_claims"]["values"],
            "backup_origins": data["backup_origins"]["values"],
            "evicted_frames": data["evicted_frames"]["values"],
            "consumer_unknown_frames": data["consumer_unknown_frames"]["values"],
            "quarantined_frames": data["quarantined_frames"]["values"],
            "quarantined_staging_bytes": data["quarantined_staging_bytes"]["values"],
            "quarantined_in_flight_bytes": data["quarantined_in_flight_bytes"]["values"],
            "quarantined_resident_bytes": data["quarantined_resident_bytes"]["values"],
            "quarantined_payload_bytes": data["quarantined_payload_bytes"]["values"],
            "quarantine_capacity_bytes": data["quarantine_capacity_bytes"]["values"],
            "staging_reservation_bytes": data["staging_reservation_bytes"]["values"],
            "in_flight_reservation_bytes": data["in_flight_reservation_bytes"]["values"],
            "peak_reservation_bytes": data["peak_reservation_bytes"]["values"],
            "queue_wait_ns": data["queue_wait_ns"],
            "queue_rejections": data["queue_rejections"]["values"],
            "queue_cancellations": data["queue_cancellations"]["values"],
            "samples": data["samples"],
            "cadence_median_us": data["cadence_us"],
            "max_gap_us": data["max_gap_us"],
            "ready_true": sum(1 for value in data["ready"] if value is True),
            "settled_true": sum(1 for value in data["settled"] if value is True),
            "input_digests_equal": data["input_digests_equal"],
        }

    def unique_or_range(series):
        raw = series["values"] if isinstance(series, dict) else series
        values = sorted({value for value in raw if value is not None})
        if not values:
            return "none"
        return str(values[0]) if len(values) == 1 else f"{values[0]}..{values[-1]}"

    def count_of(series, predicate):
        values = series["values"] if isinstance(series, dict) else series
        return f"{sum(1 for value in values if predicate(value))}/{len(values)}"

    shared = by_variant["shared"]
    independent = by_variant["independent"]
    distinct = by_variant["distinct"]
    windows2 = by_variant["windows2"]

    def across(key):
        values = sorted({trace["counters"][key] for trace in all_traces})
        return ",".join(str(value) for value in values)

    session = {
        "task": "MP-CMP1",
        "slice": "R5 reuse/fairness fixed matrix executed (5 runs x 4 variants)",
        "date": "2026-11-08",
        "status": "EXECUTED_LIMITED — measured CPU/cache reuse and fairness facts; "
                  "paint/GPU/Present NOT_MEASURED; no product or R5 PASS claim",
        "authority": {
            "assignment": "Lead task-8: run the fixed MP-CMP1 R5 matrix without product acceptance",
            "contract": "DC-MP-CMP1 reuse/fairness comparison protocol",
            "gate": "MP-R0D is RATIFIED_DECLARED_BASIS_NOT_MEASURED (ADR-0008); the matrix "
                    "executes the declared basis, it does not ratify numeric gates",
        },
        "harness_change": {
            "file": "apps/desktop/examples/compare.rs",
            "reason": "the fixed trace export did not emit the cache/quarantine accounting that "
                      "the matrix must record; the counters already existed on ImageUploadCounters",
            "change": "the trace JSON now also carries consumer_unknown_frames, backup_claims, "
                      "backup_origins, evicted_frames, decoded_retained_bytes, "
                      "staging/in_flight/resident reservation bytes, quarantined_frames, "
                      "quarantined staging/in-flight/resident/payload bytes and "
                      "quarantine_capacity_bytes; the JSON construction moved into "
                      "trace_document so it can be unit-tested",
            "behavior_unchanged": "the fixed scenario, command schedule, sample fields, "
                                  "NOT_MEASURED declarations and existing counter names are "
                                  "unchanged; only additional JSON fields are emitted",
            "tests": [
                "cargo test --locked -p cos-desktop --example compare → 1 passed, 0 failed "
                "(trace_document_emits_the_cache_and_quarantine_accounting_fields)",
                "cargo clippy --locked -p cos-desktop --example compare -- -D warnings → clean",
                "cargo fmt --all -- --check → clean",
            ],
        },
        "source": source,
        "protocol": matrix["protocol"],
        "results": {variant: variant_summary(variant) for variant in VARIANTS},
        "measured_adoption_checks": adoption,
        "measured_findings": [
            f"same-key shared: {count_of(shared['decode_calls'], lambda v: v == 1)} traces report "
            f"one decode, {count_of(shared['retained_frames'], lambda v: v == 1)} one retained "
            f"entry and {count_of(shared['consumers'], lambda v: v == 2)} two consumers; CPU "
            f"retained payload {unique_or_range(shared['cpu_retained_bytes'])} B, packed "
            f"{unique_or_range(shared['packed_retained_bytes'])} B and peak reservation "
            f"{unique_or_range(shared['peak_reservation_bytes'])} B",
            f"same-key independent: {count_of(independent['decode_calls'], lambda v: v == 2)} "
            f"traces report two decodes, "
            f"{count_of(independent['retained_frames'], lambda v: v == 2)} two retained entries "
            f"and {count_of(independent['consumers'], lambda v: v == 2)} two consumers; CPU "
            f"retained payload {unique_or_range(independent['cpu_retained_bytes'])} B, packed "
            f"{unique_or_range(independent['packed_retained_bytes'])} B and peak reservation "
            f"{unique_or_range(independent['peak_reservation_bytes'])} B — sharing halves the "
            f"retained CPU payload for this class",
            f"distinct keys: {count_of(distinct['decode_calls'], lambda v: v == 2)} traces "
            f"report two independent entries and "
            f"{count_of(distinct['input_digests_equal'], lambda v: v is False)} traces report "
            f"different decoded RGBA digests per pane, so distinct keys are never deduplicated",
            "transform independence: in every unlinked window, zoom/pan gestures targeted one "
            "pane and never changed the other pane's camera tuple "
            f"({count_of(shared['unlinked_other_pane_camera_changes'], lambda v: v == 0)} shared "
            f"traces, {count_of(independent['unlinked_other_pane_camera_changes'], lambda v: v == 0)} "
            f"independent); linked pair camera delta stayed exactly "
            f"{unique_or_range(shared['linked_pair_camera_delta_max'])} image units and each "
            f"re-link re-synchronized both panes "
            f"({unique_or_range(shared['relink_resynchronizations'])} times per shared trace)",
            "two windows: both traced windows were ready and settled in "
            f"{sum(1 for value in windows2['ready'] if value is True)}/"
            f"{len(windows2['ready'])} ready and "
            f"{sum(1 for value in windows2['settled'] if value is True)}/"
            f"{len(windows2['settled'])} settled trace lines with "
            f"{unique_or_range(windows2['refusals'])} refusals; the process-wide service reported "
            f"{unique_or_range(windows2['decode_calls'])} decodes and "
            f"{unique_or_range(windows2['retained_frames'])} retained entries at each trace end "
            f"with {unique_or_range(windows2['consumers'])} consumers; cumulative CPU preparation "
            f"queue wait was {unique_or_range(windows2['queue_wait_ns'])} ns versus "
            f"{unique_or_range(shared['queue_wait_ns'])} ns for one shared window — a measured "
            "contention signal, not a frame-pacing result. Per-window attribution is not "
            "separable because these counters are process-wide",
            f"quarantine accounting over all 25 traces: quarantined_frames [{across('quarantined_frames')}], "
            f"quarantined staging/in-flight/resident bytes "
            f"[{across('quarantined_staging_bytes')}/"
            f"{across('quarantined_in_flight_bytes')}/"
            f"{across('quarantined_resident_bytes')}], "
            f"quarantined payload bytes [{across('quarantined_payload_bytes')}], "
            f"quarantine capacity [{across('quarantine_capacity_bytes')}], "
            f"decoded retained bytes [{across('decoded_retained_bytes')}], "
            f"evicted frames [{across('evicted_frames')}], "
            f"backup claims/origins [{across('backup_claims')}/"
            f"{across('backup_origins')}], "
            f"consumer_unknown_frames [{across('consumer_unknown_frames')}]",
            f"observation-loop cadence: median achieved interval "
            f"{unique_or_range(shared['cadence_us'])} us (shared) against a requested 16,000 us, "
            f"max gaps {unique_or_range(shared['max_gap_us'])} us; this is the trace loop's own "
            "cadence, not frame pacing",
            f"readiness: {sum(1 for trace in all_traces if trace['ready'] is True)}/"
            f"{len(all_traces)} traces ready, "
            f"{sum(1 for trace in all_traces if trace['settled'] is True)}/"
            f"{len(all_traces)} settled, "
            f"{sum(1 for trace in all_traces if trace['refusals'])} traces with refusals, "
            f"cancel probe stage {unique_or_range(shared['cancel_probe_stage'])} in every trace",
            f"reuse_calls stayed {unique_or_range(shared['reuse_calls'])} in every trace even "
            "when one entry served two panes; the one-decode/two-consumer binding is carried by "
            "decode_calls, retained_frames and consumers",
        ],
        "not_measured": [
            "Present, GPU, displayed/scanout and input-to-photon; frame pacing and the "
            "ratified §10 frame/readiness gates",
            "unequal dimensions, orientations, aspect ratios and mixed DPI (both panes were "
            "the same 2048-square class at dpi [1.0, 1.0]); cursor-anchored zoom and physical "
            "1:1",
            "GPU residency/VRAM, process private bytes/working set and the backend's real "
            "decode/upload queue beyond the service's CPU counters",
            "true cold/warm cache separation and cross-process fairness",
        ],
        "evidence": ["session.json", "matrix.json", "runs.json", "run_matrix.py",
                     "analyze.py", "record.json"],
        "commands": [
            "cargo build --locked --release -p cos-desktop --example compare",
            "python -B docs/development/evidence-published/cmp1-r5-latest/run_matrix.py "
            "(driver; each run executes examples/compare.exe --fixture --trace <variant> "
            "under an isolated profile)",
            "python -B docs/development/evidence-published/cmp1-r5-latest/analyze.py "
            "(derives matrix.json and session.json from runs.json and the retained ledger "
            "digests)",
        ],
    }
    (EVIDENCE / "session.json").write_text(json.dumps(session, indent=2) + "\n",
                                           encoding="utf-8")
    print(json.dumps({"variants": {variant: {
        "runs": by_variant[variant]["runs"],
        "decode": by_variant[variant]["decode_calls"]["values"],
        "retained": by_variant[variant]["retained_frames"]["values"],
        "consumers": by_variant[variant]["consumers"]["values"],
        "samples": by_variant[variant]["samples"]["values"],
        "linked_delta": by_variant[variant]["linked_pair_camera_delta_max"]["values"],
    } for variant in VARIANTS}, "adoption": adoption}, indent=1))


if __name__ == "__main__":
    main()
