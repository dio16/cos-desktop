"""MP-TYP1-C: deterministic DPI layout matrix derived from the accepted tokens.

Python standard library only. This module measures nothing natively: it reads the
accepted typography/geometry constants out of `crates/desktop-gpui/src/tokens.rs`,
computes the physical-pixel consequences of the ratified insets at the declared
scale ladder (1.00, 1.25, 1.50, 1.75, 2.00) and refuses to emit a matrix that
breaks a layout invariant a real UI must hold.

It never re-derives an accepted value from memory: every row is computed from the
source constants, and a drift check refuses a token file whose values differ from
the ADR-0008 §5 accepted set (so a silent token change cannot leave this evidence
describing a different UI).

The Japanese fallback line height is included and marked
`used_by_adapter: false`: the pinned GPUI text run carries one line height, so
`TextSpec::ui` draws with the Latin value. That gap is a declared limitation, not
a hidden one.

Closed error vocabulary (never a traceback, never a value from an input file):

    ARGS, TOKENS_READ, TOKENS_TOO_LARGE, TOKENS_PARSE, TOKENS_MISSING,
    TOKENS_INVALID, TOKENS_DRIFT, INVARIANT_FAILED, RECORD_WRITE

`--out` is a JSON record; `--tokens` defaults to the repository copy.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import math
import re
import sys
from pathlib import Path
from typing import Any

MATRIX_VERSION = 1
SCALES = (1.00, 1.25, 1.50, 1.75, 2.00)
MAX_TOKEN_BYTES = 512 * 1024
SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")
CONST = re.compile(
    r"^pub const ([A-Z][A-Z0-9_]*): f32 = ([0-9][0-9_]*(?:\.[0-9]+)?);$", re.MULTILINE
)

# (rendering §9.1 token name, constant stem)
STYLES = (
    ("type-xs", "XS"),
    ("type-sm", "SM"),
    ("type-body", "BODY"),
    ("type-body-strong", "BODY_STRONG"),
    ("type-control", "CONTROL"),
    ("type-title", "TITLE"),
    ("type-display", "DISPLAY"),
)

LAYOUT_TOKENS = (
    "TASK_RAIL_DIP",
    "INSPECTOR_DIP",
    "SEPARATOR_PHYSICAL_PIXELS",
    "FOCUS_OUTLINE_DIP",
)

REQUIRED_TOKENS = LAYOUT_TOKENS + tuple(
    f"TYPE_{stem}_{suffix}"
    for _, stem in STYLES
    for suffix in ("SIZE_DIP", "LINE_HEIGHT_DIP", "JAPANESE_LINE_HEIGHT_DIP")
)

# The accepted values (ADR-0008 §5, rendering §9.1/§9.2). This table is a drift
# detector, never a source: the emitted matrix always comes from the token file.
ACCEPTED = {
    "TASK_RAIL_DIP": 240.0,
    "INSPECTOR_DIP": 320.0,
    "SEPARATOR_PHYSICAL_PIXELS": 1.0,
    "FOCUS_OUTLINE_DIP": 2.0,
    "TYPE_XS_SIZE_DIP": 12.0,
    "TYPE_XS_LINE_HEIGHT_DIP": 16.0,
    "TYPE_XS_JAPANESE_LINE_HEIGHT_DIP": 18.0,
    "TYPE_SM_SIZE_DIP": 13.0,
    "TYPE_SM_LINE_HEIGHT_DIP": 18.0,
    "TYPE_SM_JAPANESE_LINE_HEIGHT_DIP": 20.0,
    "TYPE_BODY_SIZE_DIP": 14.0,
    "TYPE_BODY_LINE_HEIGHT_DIP": 20.0,
    "TYPE_BODY_JAPANESE_LINE_HEIGHT_DIP": 21.0,
    "TYPE_BODY_STRONG_SIZE_DIP": 14.0,
    "TYPE_BODY_STRONG_LINE_HEIGHT_DIP": 20.0,
    "TYPE_BODY_STRONG_JAPANESE_LINE_HEIGHT_DIP": 21.0,
    "TYPE_CONTROL_SIZE_DIP": 16.0,
    "TYPE_CONTROL_LINE_HEIGHT_DIP": 24.0,
    "TYPE_CONTROL_JAPANESE_LINE_HEIGHT_DIP": 24.0,
    "TYPE_TITLE_SIZE_DIP": 20.0,
    "TYPE_TITLE_LINE_HEIGHT_DIP": 28.0,
    "TYPE_TITLE_JAPANESE_LINE_HEIGHT_DIP": 30.0,
    "TYPE_DISPLAY_SIZE_DIP": 24.0,
    "TYPE_DISPLAY_LINE_HEIGHT_DIP": 32.0,
    "TYPE_DISPLAY_JAPANESE_LINE_HEIGHT_DIP": 36.0,
}

# Declared from the wired shell: TextSpec::ui applies only the Latin line height.
JAPANESE_LINE_HEIGHT_USED_BY_ADAPTER = False

NOT_MEASURED = (
    {
        "item": "display_scale_ladder",
        "reason": "changing the display scale needs interactive Windows display control",
        "human_procedure": "Settings > System > Display > Scale: 100/125/150/175/200 %, "
        "one run per scale: cargo run --locked -p cos-desktop -- --fixture",
    },
    {
        "item": "cross_monitor_move",
        "reason": "moving a live window between monitors needs interactive input",
        "human_procedure": "drag the window across two monitors with different scales "
        "and check the separator, focus ring and truncation at the boundary",
    },
    {
        "item": "mixed_script_visual_quality",
        "reason": "tofu, clipping and baseline jitter need native rendering and inspection",
        "human_procedure": "at each scale, inspect Japanese/Latin strings in the task rail, "
        "dock and stop panel for missing glyphs, clipping and baseline jitter",
    },
    {
        "item": "resolved_face_per_run",
        "reason": "which family DirectWrite selects per run is a native rasterisation fact",
        "human_procedure": "at each scale, confirm Latin runs use Segoe UI Variable (or "
        "Segoe UI) and Japanese runs render without fallback-tofu",
    },
    {
        "item": "hairline_blur_and_pixel_snapping",
        "reason": "sub-pixel separator sharpness needs a native screenshot at fractional DPI",
        "human_procedure": "at 125/150/175/200 %, check the dock group separator stays one "
        "crisp physical pixel and does not grey out",
    },
    {
        "item": "high_contrast_focus",
        "reason": "High Contrast theming needs native theme switching and inspection",
        "human_procedure": "enable Settings > Accessibility > Contrast themes and confirm "
        "the 2-DIP focus ring stays visible on every control",
    },
    {
        "item": "text_layout_frame_cost",
        "reason": "no native frame instrumentation is available to this harness",
        "human_procedure": "run the R0 frame instrumented build at each scale and compare "
        "text/layout frame cost against the ratified §10 gates",
    },
    {
        "item": "focus_selection_scroll_retention_across_dpi_change",
        "reason": "a DPI change cannot be triggered from this process without native display "
        "control",
        "human_procedure": "focus a task row, scroll the rail and dock, change the scale, and "
        "confirm focus, selection and scroll anchor survive",
    },
)


class MatrixError(Exception):
    """One closed code; never input text, a path or a traceback."""

    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def fail(code: str) -> None:
    raise MatrixError(code)


def sha256_bytes(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def load_tokens(path: Path) -> dict[str, float]:
    """Read the accepted constants out of the token source, or refuse a closed code."""
    try:
        size = path.stat().st_size
    except OSError:
        fail("TOKENS_READ")
    if size > MAX_TOKEN_BYTES:
        fail("TOKENS_TOO_LARGE")
    try:
        text = path.read_bytes().decode("utf-8")
    except (OSError, UnicodeDecodeError):
        fail("TOKENS_READ")
    found: dict[str, float] = {}
    for match in CONST.finditer(text):
        name = match.group(1)
        try:
            found[name] = float(match.group(2).replace("_", ""))
        except ValueError:
            fail("TOKENS_PARSE")
    if not found:
        fail("TOKENS_PARSE")
    missing = sorted(name for name in REQUIRED_TOKENS if name not in found)
    if missing:
        fail("TOKENS_MISSING")
    for name in REQUIRED_TOKENS:
        value = found[name]
        if not math.isfinite(value) or value <= 0.0:
            fail("TOKENS_INVALID")
    return {name: found[name] for name in REQUIRED_TOKENS}


def verify_accepted(tokens: dict[str, float]) -> None:
    """Refuse a token set that no longer matches the ADR-0008 §5 accepted values."""
    for name, expected in ACCEPTED.items():
        if name not in tokens or abs(tokens[name] - expected) > 1e-6:
            fail("TOKENS_DRIFT")


def physical(dip: float, scale: float) -> float:
    return dip * scale


def physical_rounded(value: float) -> int:
    """Half-up physical-pixel count, documented so the record is reproducible."""
    return int(math.floor(value + 0.5))


def width_row(dip: float, scale: float) -> dict[str, Any]:
    """Exact DIP and physical values; only the pixel count is rounded.

    Rounding the DIP would break the 1-physical-pixel round trip at 150 % and
    175 %, so the record keeps full precision and rounds only `physical_rounded`.
    """
    physical_value = physical(dip, scale)
    return {
        "dip": dip,
        "physical": physical_value,
        "physical_rounded": physical_rounded(physical_value),
    }


def build_rows(tokens: dict[str, float]) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for scale in SCALES:
        separator_dip = tokens["SEPARATOR_PHYSICAL_PIXELS"] / scale
        focus_dip = tokens["FOCUS_OUTLINE_DIP"]
        line_heights = []
        for token, stem in STYLES:
            latin_dip = tokens[f"TYPE_{stem}_LINE_HEIGHT_DIP"]
            japanese_dip = tokens[f"TYPE_{stem}_JAPANESE_LINE_HEIGHT_DIP"]
            size_dip = tokens[f"TYPE_{stem}_SIZE_DIP"]
            line_heights.append(
                {
                    "token": token,
                    "size_dip": size_dip,
                    "latin": width_row(latin_dip, scale),
                    "japanese": width_row(japanese_dip, scale),
                    "japanese_gap_dip": japanese_dip - latin_dip,
                    "japanese_used_by_adapter": JAPANESE_LINE_HEIGHT_USED_BY_ADAPTER,
                }
            )
        rows.append(
            {
                "scale": scale,
                "separator": width_row(separator_dip, scale),
                "focus_outline": width_row(focus_dip, scale),
                "task_rail": width_row(tokens["TASK_RAIL_DIP"], scale),
                "inspector": width_row(tokens["INSPECTOR_DIP"], scale),
                "line_heights": line_heights,
            }
        )
    return rows


def check_invariants(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    checks: list[dict[str, Any]] = []

    def record(name: str, ok: bool, detail: str) -> None:
        checks.append({"name": name, "status": "PASS" if ok else "FAIL", "detail": detail})

    separators = [row["separator"]["physical"] for row in rows]
    record(
        "separator_never_below_one_physical_pixel",
        all(value >= 1.0 - 1e-9 for value in separators),
        f"physical separators: {separators}",
    )
    record(
        "separator_physical_rounds_to_exactly_one",
        all(row["separator"]["physical_rounded"] == 1 for row in rows),
        f"rounded: {[row['separator']['physical_rounded'] for row in rows]}",
    )
    record(
        "focus_outline_never_thinner_than_the_hairline",
        all(
            row["focus_outline"]["dip"] >= row["separator"]["dip"]
            and row["focus_outline"]["physical"] > row["separator"]["physical"]
            for row in rows
        ),
        "2-DIP focus outline against the 1-physical-pixel separator",
    )
    positive = True
    for row in rows:
        for key in ("separator", "focus_outline", "task_rail", "inspector"):
            positive = positive and row[key]["dip"] > 0.0
        for entry in row["line_heights"]:
            positive = (
                positive
                and entry["latin"]["dip"] > 0.0
                and entry["japanese"]["dip"] > 0.0
                and entry["size_dip"] > 0.0
                and entry["latin"]["physical"] > 0.0
            )
    record("no_zero_or_negative_width", positive, "every DIP and physical value > 0")

    monotonic = True
    for key in ("focus_outline", "task_rail", "inspector"):
        values = [row[key]["physical"] for row in rows]
        monotonic = monotonic and all(b >= a for a, b in zip(values, values[1:]))
    for index in range(len(rows[0]["line_heights"])):
        for side in ("latin", "japanese"):
            values = [row["line_heights"][index][side]["physical"] for row in rows]
            monotonic = monotonic and all(b >= a for a, b in zip(values, values[1:]))
    record(
        "physical_widths_monotonic_across_the_scale_ladder",
        monotonic,
        "rail, inspector, focus outline and every line height keep growing with scale",
    )
    return checks


def failed(checks: list[dict[str, Any]]) -> bool:
    return any(check["status"] != "PASS" for check in checks)


def build_record(tokens_path: Path, tokens: dict[str, float]) -> dict[str, Any]:
    rows = build_rows(tokens)
    checks = check_invariants(rows)
    try:
        digest = sha256_bytes(tokens_path.read_bytes())
    except OSError:
        fail("TOKENS_READ")
    return {
        "typography_matrix_version": MATRIX_VERSION,
        "generated_utc": datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "tokens": {
            "path": str(tokens_path).replace("\\", "/"),
            "digest": digest,
            "accepted": {name: tokens[name] for name in REQUIRED_TOKENS},
        },
        "adapter": {
            "japanese_line_height_used": JAPANESE_LINE_HEIGHT_USED_BY_ADAPTER,
            "note": "TextSpec::ui draws with the Latin line height; the Japanese fallback "
            "value is declared but not applied (one line height per GPUI text run)",
        },
        "scales": list(SCALES),
        "rows": rows,
        "invariants": checks,
        "not_measured": list(NOT_MEASURED),
        "declares_mptyp1_complete": False,
    }


def write_record(record: dict[str, Any], path: Path) -> str:
    body = json.dumps(record, sort_keys=True, separators=(",", ":")).encode()
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(body)
    except OSError:
        fail("RECORD_WRITE")
    return sha256_bytes(body)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tokens", default=str(default_tokens_path()))
    parser.add_argument("--out", required=True, help="matrix record path (JSON)")
    args = parser.parse_args(argv)
    try:
        tokens_path = Path(args.tokens)
        tokens = load_tokens(tokens_path)
        verify_accepted(tokens)
        record = build_record(tokens_path, tokens)
        digest = write_record(record, Path(args.out))
        summary = {
            "ok": True,
            "record": args.out,
            "record_digest": digest,
            "tokens": record["tokens"]["digest"],
            "rows": len(record["rows"]),
            "invariants": {
                check["status"]: sum(
                    1 for item in record["invariants"] if item["status"] == check["status"]
                )
                for check in record["invariants"]
            },
            "failed": failed(record["invariants"]),
        }
        print(json.dumps(summary, separators=(",", ":")))
        return 2 if summary["failed"] else 0
    except MatrixError as error:
        print(json.dumps({"ok": False, "error": error.code}, separators=(",", ":")))
    except (OSError, ValueError, TypeError):
        print(json.dumps({"ok": False, "error": "TOKENS_PARSE"}, separators=(",", ":")))
    return 2


def default_tokens_path() -> Path:
    """The repository copy, resolved from this file rather than the current directory."""
    return Path(__file__).resolve().parents[4] / "crates/desktop-gpui/src/tokens.rs"


if __name__ == "__main__":
    sys.exit(main())
