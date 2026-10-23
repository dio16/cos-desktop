//! Exploratory, decoder-neutral MP-IMG2 Phase 3 comparison harness (record schema v8).
//!
//! This module is shared by the operator-run `decode_compare` example and its
//! contract tests. It is not a product decoder, selects no decoder, links no
//! FFmpeg and makes no numeric claim: the emitted record is explicitly
//! exploratory and every value it contains was actually observed.
//!
//! Candidate process protocol (v8), fixed so that no argv element or decoder path
//! can be derived from media metadata. Three transports exist and all of them pass
//! argv as a separate argument vector; no command string is ever built, quoted or
//! interpreted:
//!
//! - executable: `-i <absolute case path> <absolute output path>`, identity probe
//!   `--version` then `-version`. The output keeps the case suffix, because a
//!   format-inferring tool needs it.
//! - pwsh script (`--candidate id=pwsh:<absolute script.ps1>`): the interpreter is
//!   launched by name with
//!   `-NoProfile -File <script> -InputPath <in> -OutputPath <out> [-MaxPixels N]`
//!   and the identity probe is `-Version`. The interpreter path resolved from
//!   `PATH` is recorded as `launcher` (null when it could not be resolved, in
//!   which case the bare name is launched). The script transport always writes
//!   `<stem>.<candidate>.wic.png`, so its encoder format never depends on the input
//!   suffix and no two candidates can ever share an output path.
//! - environment (contract tests only): the case paths and the probe form travel
//!   in [`ENV_INPUT`], [`ENV_OUTPUT`] and [`ENV_PROBE`], because a libtest helper
//!   process's argv is not free.
//!
//! Candidate reasons: the first [`MAX_REASON_CHARS`]-bounded `"reason_code":"..."`
//! token found in the captured stdout is mapped through [`KNOWN_REASONS`]. A token
//! outside that closed vocabulary, a malformed line or no token at all yields
//! `not_measured`; foreign text is never copied into the record.
//!
//! Evidence versus validity (B-02 / A-R1-01): an exit code alone never produces
//! `decoded`. `decoded` requires exit code `0` **and** a candidate output that exists
//! as a regular file with size `> 0`. The record keeps the evidence (`output_present`,
//! `output_bytes`, the `inspected_*` fields) separate from the claim, and `validity`
//! is exactly one of:
//!
//! - `structure_verified` — the output parsed as a **complete** container of the
//!   declared format and its declared dimensions equal the operator expectation. This
//!   means structural presence only: mandatory chunks/segments, their CRCs/lengths and
//!   the declared dimensions were checked, never pixel content.
//! - `pixels_verified` — the decoded RGBA bytes were hashed by the harness's own PNG
//!   oracle and the digest equals the operator's `pixels:<sha256>` expectation.
//! - `pixel_mismatch` — the decoded pixels contradict that expectation; demoted to
//!   `error` with refusal `pixel_mismatch`.
//! - `dimension_mismatch` — it parsed completely but the dimensions contradict the
//!   expectation; the observation is demoted to `error` with refusal
//!   `dimension_mismatch`.
//! - `presence_only` — no expectation was declared, so only the presence of a
//!   non-empty output is claimed; nothing about its content is asserted.
//! - `not_measured` — nothing about the image was established: no output, a
//!   non-success class, an unparsable output (`output_unparsable`) or an incomplete
//!   one (`output_incomplete`). Both demote the observation to `error` with that
//!   refusal while the raw exit code stays recorded as evidence (N5).
//!
//! Exit `0` with a missing or zero-byte output is `error` with refusal
//! `output_missing`. "Verified" means container structure, mandatory chunk/segment
//! presence and the declared dimensions — never pixel content: this harness does not
//! decode pixels, does not validate colour values or colour management, and does not
//! prove that the compressed image data is decodable, so no claim here implies
//! pixel-level validation or format compatibility.
//!
//! Verified evidence and capability facts (MP-IMG2 Phase 2): the harness inspects
//! the bytes the candidate wrote itself with the bounded dependency-free parser in
//! [`inspect`] (PNG IHDR plus a bounded chunk walk with `IEND` and
//! `iCCP`/`sRGB`/`gAMA` presence, BMP `BITMAPINFOHEADER`/V4/V5 width/height/bit
//! count, JPEG `SOF0`/`SOF2` precision, dimensions and component count). Nothing is
//! taken from the candidate's own report and nothing is guessed: an output the parser
//! cannot read is `not_measured`, and when an expectation exists that also demotes
//! the observation class to `error` with refusal `output_unparsable`, so the record
//! never keeps a `decoded` claim it cannot support. A `cases.txt` line may declare
//! `name[|expected:WxH][|declared-class]`; when an expectation exists and the output
//! parses, matching dimensions yield `validity: structure_verified`, and a mismatch
//! demotes the observation to `error` with refusal `dimension_mismatch` — never a
//! success class. Without an expectation the claim stays `presence_only`. The record
//! also carries a `capabilities` table with one row per candidate × observed format.
//!
//! A claim requires a **complete** container (B2 / B-R1-01 / B-R2-01): PNG needs a
//! legal IHDR, at least one non-empty `IDAT` and a real `IEND` of length `0` whose CRC
//! *value* is `AE 42 60 82`; every counted chunk must have its whole payload and its
//! four CRC bytes inside the file, and no CRC is computed for any other chunk. JPEG
//! needs a full `SOF` segment, a well-formed `SOS` after it (`Ls >= 6`, `Ns` in
//! `1..=4`, `Ls == 6 + 2 * Ns`, non-zero component ids with table selectors,
//! `Ss`/`Se`/`AhAl` present, segment end inside the file) and a real `EOI` at the end
//! of the file; BMP needs the whole declared header, a pixel-data offset inside the
//! file and a declared pixel buffer that fits it (`data_offset + row_bytes * height <=
//! file_size`, with the 4-byte-aligned row size of the declared bit count and the
//! height magnitude so top-down bitmaps are covered). Header-only, CRC-less or
//! truncated outputs are therefore `error` + `output_incomplete`, never a success.
//! Independent pixel verification (MP-IMG2 Phase 3): when a `cases.txt` line carries
//! `pixels:<sha256>`, the harness decodes the candidate output with its own bounded
//! PNG oracle ([`pixels`]) — its own DEFLATE and SHA-256, no dependency, no `unsafe`
//! — expands the samples to row-major RGBA and compares the digest. The precedence is
//! explicit: **pixel expectation > dimension expectation > structure**. Structure is
//! checked first (pixels of an incomplete container are not claimed), a declared pixel
//! digest then decides the verdict on its own, and the dimension rules of Phase 2 only
//! apply without one. A mismatch is `error` + `pixel_mismatch`, an undecodable output
//! is `error` + `pixel_unavailable` with the oracle's own reason token, and no digest
//! is ever fabricated: `pixel_digest` is `null` unless the oracle actually decoded the
//! bytes.
//!
//! Capability rows are built only from observations that are `decoded`, complete, and
//! whose validity is `presence_only` or `structure_verified` (A1), after observation
//! truncation (A3), so a mismatch, refusal or dropped row never contributes. Each row
//! carries `pixel_verified`, the number of its observations whose decoded pixels were
//! verified; structure and pixel evidence are therefore labelled separately, and a
//! `pixel_mismatch` row is `error` so it never counts at all.
//! `max_width`/`max_height` are independent observed maxima and need not come from
//! one output (N3). Colour-management markers are reported only when the parser
//! actually saw them, a set the headers never stated is named in `not_measured` with
//! the reason `header_field_not_observed` (N2), and orientation, region/partial-decode
//! and sequential capability are always `not_measured` with an explicit reason token
//! because this harness does not observe them.
//!
//! Outcome mapping: exit code `0` with a present output is `decoded`, `2`
//! unsupported, `3` corrupt, `4` oversize, `5` cancelled; any other code is
//! `error`, a process that cannot be started is `not_run`, a process without an
//! observed exit code is `not_measured`, and a hard-timeout or capped-scratch kill
//! is `cancelled`. Exit codes are the operating-system code as a **signed 32-bit
//! value** (Rust `ExitStatus::code()`), so e.g. FFmpeg's `0xABAAAA08` appears as
//! `-1414549496`.
//!
//! Safety: only the operator-nominated program is launched (never a shell), every
//! child gets the same bounded argv, stdin is closed, stdout/stderr are redirected
//! to per-observation scratch files that the parent reads only after the child is
//! reaped (at most `max_output_bytes` each, never re-emitted as text except the
//! bounded identity line), and each child has a hard deadline. Because capture
//! never depends on a pipe, a descendant that inherits the child's handles cannot
//! keep the harness waiting.
//!
//! Work directory (B-R2-01): the harness never clears, reuses or rewrites an
//! existing directory. `<out parent>/decode_compare_outputs` is created if it is
//! missing and otherwise left exactly as it is; every run creates a fresh child
//! `<run-token>` ([`run_token`]) with `create_dir`, so a token that already exists
//! is refused as [`WORK_DIR_EXISTS`] instead of being reused. Candidate outputs and
//! `captures/` live only inside that run directory, which this process created in
//! this run and is therefore provably ours: it is removed when the run ends
//! (success, refusal or error) and nothing outside it is ever deleted. A crash
//! leaves the run directory behind for the operator to clear; the next run picks a
//! new token and never touches it. Liability for foreign data is therefore zero by
//! construction rather than by name matching. The run token and the work directory
//! are internal plumbing and never appear in the record.
//!
//! Scratch and output bounds (B-03): the combined capture size is checked against
//! `max_output_bytes * 8` (probe: [`PROBE_CAP_BYTES`]` * 8`), and the candidate
//! output file against [`Config::max_output_file_bytes`], both while polling and
//! again after the child has exited, so a flood that finishes between two polls
//! cannot be missed. Exceeding either bound records `cancelled` with refusal
//! `scratch_capped` / `output_capped` and the observed size, never a success
//! class. This is **observation-based detection, not an enforced quota**: the
//! harness only samples sizes, so a child's write burst is not bounded by the
//! harness at all — it can write arbitrarily much between two observations, and the
//! overshoot is merely detected at the next poll or after the child exits. The
//! harness never stops or throttles that write, and a descendant that keeps writing
//! after the child exits is neither observed nor stopped.
//!
//! Termination scope (A-01): kill/reap covers the direct child only. Termination
//! of descendants is neither guaranteed nor claimed anywhere, and no test or
//! record asserts it; the descendant test proves only that the parent is not
//! blocked by an inherited capture handle.

use crate::inspect::{self, ColourMarker, ColourType, Format, Inspection};
use crate::pixels;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt::Write as _,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Fixed record schema version; changing record semantics requires a new version.
pub const SCHEMA_VERSION: u32 = 8;
/// Refusal token for a run token whose directory already exists.
pub const WORK_DIR_EXISTS: &str = "work_dir_exists";
/// Fixed prefix of every per-run work-directory token.
pub const RUN_TOKEN_PREFIX: &str = "run-";
/// Exploratory comparisons are not a ratified numeric gate.
pub const NUMERIC_GATE: &str = "NOT_RATIFIED";
/// Optional operator expectations per case: `name[|expected:WxH][|declared-class]`.
pub const MANIFEST_FILE: &str = "cases.txt";
/// Operator work subdirectory created next to the record.
pub const OUTPUT_DIR: &str = "decode_compare_outputs";
/// Scratch subdirectory of [`OUTPUT_DIR`] holding redirected capture files.
pub const CAPTURE_DIR: &str = "captures";
/// CLI prefix selecting the documented PowerShell 7 script transport.
pub const PWSH_PREFIX: &str = "pwsh:";
/// Environment transport: absolute case input path.
pub const ENV_INPUT: &str = "DECODE_COMPARE_INPUT";
/// Environment transport: absolute candidate output path.
pub const ENV_OUTPUT: &str = "DECODE_COMPARE_OUTPUT";
/// Environment transport: identity probe form (`--version` or `-version`).
pub const ENV_PROBE: &str = "DECODE_COMPARE_PROBE";
/// Largest accepted `cases.txt`, in bytes; the reader allocates one byte more.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
/// Hard cap for one emitted record, in bytes; never exceeded when writing.
pub const MAX_RECORD_BYTES: usize = 512 * 1024;
/// Capture scratch a child may write, as a multiple of its read cap.
pub const SCRATCH_FACTOR: u64 = 8;
/// Largest accepted `--max-observations`, so one run stays bounded.
pub const MAX_OBSERVATIONS: usize = 4096;
/// Largest accepted `--max-output-file-bytes`, so the output bound stays bounded.
pub const MAX_OUTPUT_FILE_BYTES: usize = 1024 * 1024 * 1024;

const USAGE: &str = "usage: decode_compare --cases DIR --candidate ID=ABSOLUTE_EXE|\
ID=pwsh:ABSOLUTE_PS1 [--candidate ...]... --out FILE [--timeout-ms N] \
[--max-output-bytes N] [--max-output-file-bytes N] [--max-cases N] \
[--max-observations N] [--max-pixels N]";
const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 8 * 1024;
const DEFAULT_MAX_OUTPUT_FILE_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_MAX_CASES: usize = 64;
const DEFAULT_MAX_OBSERVATIONS: usize = 256;
const MAX_ID_BYTES: usize = 64;
const MAX_CASE_NAME_BYTES: usize = 256;
const MAX_IDENTITY_CHARS: usize = 256;
const MAX_REASON_CHARS: usize = 32;
const MAX_EXPECTED_DIMENSION: u32 = 1 << 20;
const MAX_MANIFEST_LINES: usize = 1024;
const MAX_DIR_ENTRIES: usize = 4096;
const MAX_PATH_ENTRIES: usize = 64;
const OUTPUT_NAME_CHARS: usize = 48;
const PROBE_CAP_BYTES: usize = 4 * 1024;
const PWSH_PROGRAM: &str = "pwsh";
const INPUT_FLAG: &str = "-i";
const NOT_MEASURED: &str = "not_measured";
/// Validity claim when the only evidence is a non-empty output file.
const PRESENCE_ONLY: &str = "presence_only";
/// Validity claim when the inspected output is a complete container of the declared
/// format and its declared dimensions match the operator expectation. The name is
/// deliberately about *structure*: no pixel content is validated.
const STRUCTURE_VERIFIED: &str = "structure_verified";
/// Validity claim (and refusal) when the inspected output contradicts it.
const DIMENSION_MISMATCH: &str = "dimension_mismatch";
/// Validity claim when the decoded RGBA digest matches the operator expectation.
const PIXELS_VERIFIED: &str = "pixels_verified";
/// Validity claim (and refusal) when the decoded pixels contradict the expectation.
const PIXEL_MISMATCH: &str = "pixel_mismatch";
/// Refusal when a declared pixel expectation could not be checked at all.
const PIXEL_UNAVAILABLE: &str = "pixel_unavailable";
/// Reason tokens for a validity that could not be established.
const OUTPUT_UNPARSABLE: &str = "output_unparsable";
const OUTPUT_INCOMPLETE: &str = "output_incomplete";
const NO_EXPECTATION: &str = "no_expectation";
/// Capability facts this harness never observes, with the reason it does not.
const ORIENTATION_REASON: &str = "exif_orientation_not_parsed";
const REGION_REASON: &str = "partial_decode_not_observed";
const SEQUENTIAL_REASON: &str = "sequential_capability_not_observed";
const HEADER_FIELD_NOT_OBSERVED: &str = "header_field_not_observed";
/// Content of the work-directory ownership marker (tool label + schema version).
/// Bytes still to append after the last observation row: `\n  ]`, `\n`, `}`, `\n`.
const OBSERVATIONS_TAIL: &str = "\n  ]\n}\n";
const POLL_INTERVAL: Duration = Duration::from_millis(2);
/// Closed vocabulary a script candidate may report; mirrors `wic_probe.ps1`.
const KNOWN_REASONS: &[&str] = &[
    "argument_missing",
    "imaging_unavailable",
    "input_missing",
    "input_not_local",
    "max_pixels_exceeded",
    "output_not_local",
    "probe_error",
    "unsupported_output_extension",
    "win_bad_header",
    "win_bad_image",
    "win_bad_stream_data",
    "win_codec_error",
    "win_component_not_found",
    "win_unknown_image_format",
];

/// The closed outcome vocabulary; every other observation is `not_measured`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeClass {
    Decoded,
    Unsupported,
    Corrupt,
    Oversize,
    Cancelled,
    Error,
    NotRun,
    NotMeasured,
}

impl OutcomeClass {
    /// Stable wire token, also accepted in the optional case manifest.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decoded => "decoded",
            Self::Unsupported => "unsupported",
            Self::Corrupt => "corrupt",
            Self::Oversize => "oversize",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
            Self::NotRun => "not_run",
            Self::NotMeasured => "not_measured",
        }
    }

    fn from_token(token: &str) -> Option<Self> {
        Some(match token {
            "decoded" => Self::Decoded,
            "unsupported" => Self::Unsupported,
            "corrupt" => Self::Corrupt,
            "oversize" => Self::Oversize,
            "cancelled" => Self::Cancelled,
            "error" => Self::Error,
            "not_run" => Self::NotRun,
            "not_measured" => Self::NotMeasured,
            _ => return None,
        })
    }
}

/// Documented identity probe forms, in the order they are attempted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeForm {
    /// `--version`, the long form of an executable probe.
    Long,
    /// `-version`, the fallback form for tools (FFmpeg) that reject the long form.
    Short,
    /// `-Version`, the only form of the PowerShell script transport.
    Script,
}

impl ProbeForm {
    /// Recorded token; `identity_probe` is `null` when no form succeeded.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Long => "--version",
            Self::Short => "-version",
            Self::Script => "-Version",
        }
    }
}

/// A refused invocation: nothing was measured and no record was written.
#[derive(Debug)]
pub struct UsageError {
    message: String,
}

impl UsageError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for UsageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// The program a candidate names, already validated as an absolute local path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Program {
    /// Operator-nominated executable.
    Executable(PathBuf),
    /// Operator-nominated PowerShell 7 script, run through an interpreter.
    PwshScript(PathBuf),
}

impl Program {
    /// The validated absolute path the candidate named.
    pub fn path(&self) -> &Path {
        match self {
            Self::Executable(path) | Self::PwshScript(path) => path,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Executable(_) => "executable",
            Self::PwshScript(_) => "pwsh-script",
        }
    }

    /// Probe forms to attempt, in order.
    fn probe_forms(&self) -> &'static [ProbeForm] {
        match self {
            Self::Executable(_) => &[ProbeForm::Long, ProbeForm::Short],
            Self::PwshScript(_) => &[ProbeForm::Script],
        }
    }
}

/// The program that is actually launched: the candidate itself, or the interpreter
/// that runs the nominated script. `path` is the resolved absolute launcher when
/// one could be found, so the record says what ran, not just what was requested.
#[derive(Clone, Debug)]
pub struct Launcher {
    pub command: OsString,
    pub path: Option<PathBuf>,
}

/// One operator-nominated candidate.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: String,
    pub program: Program,
}

/// How a candidate process receives the case input and output paths.
#[derive(Clone, Debug)]
pub struct Invocation {
    /// Fixed argv inserted before the transport arguments; empty for operators.
    pub argv_prefix: Vec<String>,
    /// Pass [`ENV_INPUT`]/[`ENV_OUTPUT`]/[`ENV_PROBE`] instead of argv. The
    /// hermetic contract tests need this because their helper process argv must
    /// stay compatible with the libtest runner.
    pub env_transport: bool,
}

/// What the operator declared for one case in `cases.txt`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Expectation {
    /// Declared outcome class, when the line names one.
    pub outcome: Option<OutcomeClass>,
    /// Declared output dimensions, when the line names `expected:WxH`.
    pub dimensions: Option<(u32, u32)>,
    /// Declared SHA-256 of the decoded RGBA bytes, when the line names `pixels:<hex>`.
    pub pixel_digest: Option<[u8; 32]>,
}

/// One validated comparison run configuration.
#[derive(Debug)]
pub struct Config {
    pub cases: PathBuf,
    pub out: PathBuf,
    pub candidates: Vec<Candidate>,
    pub timeout_ms: u64,
    pub max_output_bytes: usize,
    /// Observed bound for the candidate's own output file (B-03).
    pub max_output_file_bytes: usize,
    pub max_cases: usize,
    pub max_observations: usize,
    /// Hard per-record byte budget; [`MAX_RECORD_BYTES`] unless a test lowers it.
    pub max_record_bytes: usize,
    /// Optional operator pixel bound forwarded to the script transport.
    pub max_pixels: Option<u64>,
    pub invocation: Invocation,
    /// Per-case operator expectations, keyed by case file name.
    pub expectations: BTreeMap<String, Expectation>,
}

/// Observed candidate identity. Every field is an observation or `null`.
#[derive(Debug)]
pub struct CandidateRecord {
    pub id: String,
    pub kind: &'static str,
    pub program: PathBuf,
    pub launcher: Option<PathBuf>,
    pub identity: Option<String>,
    pub identity_probe: Option<&'static str>,
    /// Both attempted probe exit codes, in attempt order (signed DWORDs).
    pub identity_exit_codes: [Option<i32>; 2],
    /// Why the identity could not be observed at all, when that applies.
    pub refusal: Option<&'static str>,
}

/// One (case, candidate) observation. Absent values are `None`, never guessed.
#[derive(Debug)]
pub struct Observation {
    pub candidate_id: String,
    pub case_id: String,
    pub input_path: PathBuf,
    pub suffix: Option<String>,
    pub input_bytes: Option<u64>,
    pub declared_outcome: OutcomeClass,
    pub observed_outcome: OutcomeClass,
    /// Whitelisted reason token from the candidate, or `not_measured`.
    pub candidate_reason: &'static str,
    pub exit_code: Option<i32>,
    pub timeout_fired: bool,
    pub captured_stdout_bytes: usize,
    pub captured_stderr_bytes: usize,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// Observed child wall time; `None` when no child was started.
    pub wall_ms: Option<u64>,
    /// Combined capture-file bytes observed when the child was reaped.
    pub scratch_bytes: Option<u64>,
    /// Whether a non-empty regular output file was observed for this case.
    pub output_present: bool,
    /// Observed output size when a regular file exists (may be `0`), else `None`.
    pub output_bytes: Option<u64>,
    /// Container the harness itself recognised in the output.
    pub inspected_format: Format,
    /// Whether the container terminator was observed.
    pub inspected_complete: bool,
    pub inspected_width: Option<u32>,
    pub inspected_height: Option<u32>,
    pub inspected_bit_depth: Option<u32>,
    pub inspected_channels: Option<u32>,
    pub inspected_colour_type: ColourType,
    /// Colour-management markers the inspector saw in this output.
    pub inspected_colour_markers: Vec<ColourMarker>,
    /// Operator expectation for this case, when the manifest declares one.
    pub expected_dimensions: Option<(u32, u32)>,
    /// Declared RGBA digest, when the manifest declares `pixels:<hex>`.
    pub pixel_expectation: Option<[u8; 32]>,
    /// Gate of what the oracle produced: `verified`, `mismatch` or `not_measured`.
    pub pixel_status: PixelStatus,
    /// Digest of the decoded RGBA bytes; `None` when nothing was decoded.
    pub pixel_digest: Option<[u8; 32]>,
    /// Whether the decoded pixels matched the operator expectation.
    pub pixel_verified: bool,
    /// Stable token explaining the pixel verdict (`None` when there is none).
    pub pixel_reason: Option<&'static str>,
    /// Claim standing on that evidence: `structure_verified`, `dimension_mismatch`,
    /// `presence_only` or `not_measured`.
    pub validity: &'static str,
    /// Why the claim is not `structure_verified`, as a stable token.
    pub validity_reason: Option<&'static str>,
    /// Stable refusal reason when the case was not fully measured, else `None`.
    pub refusal: Option<&'static str>,
}

/// The complete bounded record; it has no pass, winner, score or decision field.
#[derive(Debug)]
pub struct Report {
    pub schema_version: u32,
    pub exploratory: bool,
    pub numeric_gate: &'static str,
    pub cases_dir: PathBuf,
    pub timeout_ms: u64,
    pub max_output_bytes: usize,
    pub max_output_file_bytes: usize,
    pub max_cases: usize,
    pub max_observations: usize,
    pub max_record_bytes: usize,
    pub max_pixels: Option<u64>,
    pub cases_truncated: bool,
    pub observations_truncated: bool,
    pub candidates: Vec<CandidateRecord>,
    pub observations: Vec<Observation>,
    pub capabilities: Vec<CapabilityRow>,
}

/// What the independent pixel oracle concluded for one observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelStatus {
    /// No pixel expectation was declared, so nothing was decoded.
    NotRequested,
    /// Pixels decoded and their digest matched the expectation.
    Verified,
    /// Pixels decoded and their digest contradicted the expectation.
    Mismatch,
    /// An expectation exists but the pixels could not be decoded.
    Unavailable,
}

/// One candidate × observed format capability row, derived only from outputs the
/// harness parsed itself.
///
/// `max_width`/`max_height` are independent observed maxima: they need not come from
/// the same output, so they are a lower bound on each axis, not a decoded size.
#[derive(Debug)]
pub struct CapabilityRow {
    pub candidate_id: String,
    pub format: Format,
    pub observations: usize,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub bit_depths: Vec<u32>,
    pub channels: Vec<u32>,
    pub colour_types: Vec<ColourType>,
    pub colour_markers: Vec<ColourMarker>,
    /// How many of `observations` had their decoded pixels verified against an
    /// operator digest. Structure and pixel evidence are labelled separately, and a
    /// `pixel_mismatch` observation is `error` so it never reaches this row at all.
    pub pixel_verified: usize,
}

struct Case {
    id: String,
    path: PathBuf,
    suffix: Option<String>,
    input_bytes: Option<u64>,
}

#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    truncated: bool,
}

struct Supervised {
    exit_code: Option<i32>,
    timeout_fired: bool,
    wall_ms: u64,
    scratch_bytes: u64,
    capped: Option<CapKind>,
}

/// Which observed bound a run exceeded; the refusal token follows from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CapKind {
    Scratch,
    Output,
}

impl CapKind {
    fn refusal(self) -> &'static str {
        match self {
            Self::Scratch => "scratch_capped",
            Self::Output => "output_capped",
        }
    }
}

#[derive(Default)]
struct ProbedIdentity {
    identity: Option<String>,
    probe: Option<&'static str>,
    exit_codes: [Option<i32>; 2],
    refusal: Option<&'static str>,
}

/// Observed byte bounds for one child. This is detection, not an enforced quota:
/// the sizes are sampled while polling and re-read after the child is reaped.
struct Bounds<'a> {
    first: &'a Path,
    second: &'a Path,
    scratch_limit: u64,
    output: Option<&'a Path>,
    output_limit: u64,
}

/// Parse and validate the operator CLI; any failure is a refusal, not a guess.
pub fn parse<I>(args: I) -> Result<Config, UsageError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut cases = None;
    let mut out = None;
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut timeout_ms = DEFAULT_TIMEOUT_MS;
    let mut max_output_bytes = DEFAULT_MAX_OUTPUT_BYTES;
    let mut max_output_file_bytes = DEFAULT_MAX_OUTPUT_FILE_BYTES;
    let mut max_cases = DEFAULT_MAX_CASES;
    let mut max_observations = DEFAULT_MAX_OBSERVATIONS;
    let mut max_pixels = None;
    let mut arguments = args.into_iter();
    while let Some(flag) = arguments.next() {
        let flag = flag
            .into_string()
            .map_err(|_| UsageError::new("argument name is not valid UTF-8"))?;
        match flag.as_str() {
            "--cases" => cases = Some(PathBuf::from(take(&mut arguments, "--cases")?)),
            "--out" => out = Some(PathBuf::from(take(&mut arguments, "--out")?)),
            "--candidate" => {
                candidates.push(parse_candidate(&take(&mut arguments, flag.as_str())?)?)
            }
            "--timeout-ms" => timeout_ms = take_number(&mut arguments, "--timeout-ms")?,
            "--max-output-bytes" => {
                max_output_bytes = take_size(&mut arguments, "--max-output-bytes")?;
            }
            "--max-output-file-bytes" => {
                max_output_file_bytes = take_size(&mut arguments, "--max-output-file-bytes")?;
            }
            "--max-cases" => max_cases = take_size(&mut arguments, "--max-cases")?,
            "--max-observations" => {
                max_observations = take_size(&mut arguments, "--max-observations")?;
            }
            "--max-pixels" => {
                let pixels = take_number(&mut arguments, "--max-pixels")?;
                if pixels == 0 {
                    return Err(UsageError::new("--max-pixels must be greater than zero"));
                }
                max_pixels = Some(pixels);
            }
            other => {
                return Err(UsageError::new(format!(
                    "unexpected argument {other:?}; expected one of --cases, --candidate, --out, \
                     --timeout-ms, --max-output-bytes, --max-output-file-bytes, --max-cases, \
                     --max-observations, --max-pixels"
                )));
            }
        }
    }
    let cases = cases.ok_or_else(|| UsageError::new("--cases DIR is required"))?;
    let out = out.ok_or_else(|| UsageError::new("--out FILE is required"))?;
    if candidates.is_empty() {
        return Err(UsageError::new(
            "at least one --candidate ID=ABSOLUTE_EXE or ID=pwsh:ABSOLUTE_PS1 is required",
        ));
    }
    if timeout_ms == 0 {
        return Err(UsageError::new("--timeout-ms must be greater than zero"));
    }
    if max_output_bytes == 0 {
        return Err(UsageError::new(
            "--max-output-bytes must be greater than zero",
        ));
    }
    if max_output_file_bytes == 0 || max_output_file_bytes > MAX_OUTPUT_FILE_BYTES {
        return Err(UsageError::new(format!(
            "--max-output-file-bytes must be 1-{MAX_OUTPUT_FILE_BYTES}"
        )));
    }
    if max_cases == 0 {
        return Err(UsageError::new("--max-cases must be greater than zero"));
    }
    if max_observations == 0 || max_observations > MAX_OBSERVATIONS {
        return Err(UsageError::new(format!(
            "--max-observations must be 1-{MAX_OBSERVATIONS}"
        )));
    }
    let cases = local_absolute_path(&cases, "--cases")?;
    if !cases.is_dir() {
        return Err(UsageError::new(format!(
            "--cases is not an existing directory: {}",
            cases.display()
        )));
    }
    let out = local_absolute_path(&out, "--out")?;
    if out.file_name().is_none() || !out.parent().is_some_and(|parent| parent.is_dir()) {
        return Err(UsageError::new(format!(
            "--out must name a file inside an existing directory: {}",
            out.display()
        )));
    }
    let mut ids: Vec<&str> = Vec::with_capacity(candidates.len());
    for candidate in &candidates {
        if ids.contains(&candidate.id.as_str()) {
            return Err(UsageError::new(format!(
                "duplicate --candidate id {:?}",
                candidate.id
            )));
        }
        ids.push(candidate.id.as_str());
    }
    let expectations = load_manifest(&cases)?;
    Ok(Config {
        cases,
        out,
        candidates,
        timeout_ms,
        max_output_bytes,
        max_output_file_bytes,
        max_cases,
        max_observations,
        max_record_bytes: MAX_RECORD_BYTES,
        max_pixels,
        invocation: Invocation {
            argv_prefix: Vec::new(),
            env_transport: false,
        },
        expectations,
    })
}

fn take<I>(arguments: &mut I, flag: &str) -> Result<OsString, UsageError>
where
    I: Iterator<Item = OsString>,
{
    arguments
        .next()
        .ok_or_else(|| UsageError::new(format!("{flag} requires a value")))
}

fn take_number<I>(arguments: &mut I, flag: &str) -> Result<u64, UsageError>
where
    I: Iterator<Item = OsString>,
{
    let value = take(arguments, flag)?
        .into_string()
        .map_err(|_| UsageError::new(format!("{flag} value is not valid UTF-8")))?;
    value
        .parse::<u64>()
        .map_err(|_| UsageError::new(format!("{flag} requires a non-negative integer")))
}

fn take_size<I>(arguments: &mut I, flag: &str) -> Result<usize, UsageError>
where
    I: Iterator<Item = OsString>,
{
    usize::try_from(take_number(arguments, flag)?)
        .map_err(|_| UsageError::new(format!("{flag} is out of range on this platform")))
}

fn parse_candidate(value: &OsString) -> Result<Candidate, UsageError> {
    let text = value
        .to_str()
        .ok_or_else(|| UsageError::new("--candidate value is not valid UTF-8"))?;
    let (id, rest) = text.split_once('=').ok_or_else(|| {
        UsageError::new(format!(
            "--candidate requires ID=ABSOLUTE_EXE or ID=pwsh:ABSOLUTE_PS1, not {text:?}"
        ))
    })?;
    let id_ok = !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && id != "."
        && id != ".."
        && id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        });
    if !id_ok {
        return Err(UsageError::new(format!(
            "--candidate id {id:?} must be 1-{MAX_ID_BYTES} characters of [A-Za-z0-9._-] and not \
             '.' or '..'"
        )));
    }
    let program = if let Some(script) = rest.strip_prefix(PWSH_PREFIX) {
        Program::PwshScript(local_absolute_path(
            Path::new(script),
            &format!("--candidate {id:?} script"),
        )?)
    } else {
        Program::Executable(local_absolute_path(
            Path::new(rest),
            &format!("--candidate {id:?}"),
        )?)
    };
    Ok(Candidate {
        id: id.to_owned(),
        program,
    })
}

/// Accept only an absolute local disk path: no relative, derived, UNC, device,
/// parent-traversal or current-directory form. The path is never canonicalized,
/// so an absent candidate stays an honest `not_run` instead of becoming a usage
/// refusal.
fn local_absolute_path(path: &Path, what: &str) -> Result<PathBuf, UsageError> {
    if !path.is_absolute() {
        return Err(UsageError::new(format!(
            "{what} must be an absolute path, not {}",
            path.display()
        )));
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let mut components = path.components();
        match components.next() {
            Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) => {}
            _ => {
                return Err(UsageError::new(format!(
                    "{what} must name a local disk path, not {}",
                    path.display()
                )));
            }
        }
        if components.next() != Some(Component::RootDir) {
            return Err(UsageError::new(format!(
                "{what} must be rooted, not {}",
                path.display()
            )));
        }
        for component in components {
            match component {
                Component::Normal(_) => {}
                _ => {
                    return Err(UsageError::new(format!(
                        "{what} contains a parent or current directory component: {}",
                        path.display()
                    )));
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        use std::path::Component;
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {}
                _ => {
                    return Err(UsageError::new(format!(
                        "{what} contains a parent or current directory component: {}",
                        path.display()
                    )));
                }
            }
        }
    }
    Ok(path.to_path_buf())
}

/// Read the optional operator declaration through a hard byte bound; a malformed
/// manifest is a refusal. Lines are `name[|expected:WxH][|declared-class]`, with the
/// fields after the name optional and any other field refused. The file is opened
/// and read with `take(MAX + 1)` so the declared cap bounds allocation and one extra
/// byte proves the cap was exceeded.
fn load_manifest(cases: &Path) -> Result<BTreeMap<String, Expectation>, UsageError> {
    let mut expectations: BTreeMap<String, Expectation> = BTreeMap::new();
    let manifest = cases.join(MANIFEST_FILE);
    let file = match fs::File::open(&manifest) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(expectations),
        Err(error) => {
            return Err(UsageError::new(format!(
                "cannot read {}: {error}",
                manifest.display()
            )));
        }
    };
    let limit = u64::try_from(MAX_MANIFEST_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::new();
    file.take(limit)
        .read_to_end(&mut bytes)
        .map_err(|error| UsageError::new(format!("cannot read {}: {error}", manifest.display())))?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(UsageError::new(format!(
            "{MANIFEST_FILE} exceeds {MAX_MANIFEST_BYTES} bytes"
        )));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| UsageError::new(format!("{MANIFEST_FILE} is not UTF-8")))?;
    if text.lines().count() > MAX_MANIFEST_LINES {
        return Err(UsageError::new(format!(
            "{MANIFEST_FILE} exceeds {MAX_MANIFEST_LINES} lines"
        )));
    }
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // A4: the legacy TAB form is refused by name instead of silently parsing
        // into an expectation that matches no field.
        if line.contains('\t') {
            return Err(UsageError::new(format!(
                "{MANIFEST_FILE} line {} uses the legacy TAB separator; use \
                 '<name>|<expected:WxH>|<declared-class>'",
                index + 1
            )));
        }
        let mut fields = line.split('|').map(str::trim);
        let name = fields.next().unwrap_or_default();
        if name.is_empty() || name.len() > MAX_CASE_NAME_BYTES {
            return Err(UsageError::new(format!(
                "{MANIFEST_FILE} line {} names a case of 1-{MAX_CASE_NAME_BYTES} bytes",
                index + 1
            )));
        }
        let mut expectation = Expectation::default();
        for field in fields {
            if field.is_empty() {
                return Err(UsageError::new(format!(
                    "{MANIFEST_FILE} line {} has an empty field",
                    index + 1
                )));
            }
            if let Some(digest) = field.strip_prefix("pixels:") {
                if expectation.pixel_digest.is_some() {
                    return Err(UsageError::new(format!(
                        "{MANIFEST_FILE} line {} declares more than one pixel digest",
                        index + 1
                    )));
                }
                expectation.pixel_digest = Some(pixels::parse_digest(digest).ok_or_else(|| {
                    UsageError::new(format!(
                        "{MANIFEST_FILE} line {} declares a pixel digest that is not 64 hex \
                         digits",
                        index + 1
                    ))
                })?);
            } else if let Some(dimensions) = field.strip_prefix("expected:") {
                if expectation.dimensions.is_some() {
                    return Err(UsageError::new(format!(
                        "{MANIFEST_FILE} line {} declares more than one expectation",
                        index + 1
                    )));
                }
                let parsed = parse_dimensions(dimensions).ok_or_else(|| {
                    UsageError::new(format!(
                        "{MANIFEST_FILE} line {} expects {dimensions:?}, not WxH",
                        index + 1
                    ))
                })?;
                expectation.dimensions = Some(parsed);
            } else {
                if expectation.outcome.is_some() {
                    return Err(UsageError::new(format!(
                        "{MANIFEST_FILE} line {} declares more than one outcome class",
                        index + 1
                    )));
                }
                let class = OutcomeClass::from_token(field).ok_or_else(|| {
                    UsageError::new(format!(
                        "{MANIFEST_FILE} line {} declares an unknown outcome class {field:?}",
                        index + 1
                    ))
                })?;
                expectation.outcome = Some(class);
            }
        }
        // A4: an expectation for a case that is not in the directory is a typo,
        // not a silent no-op.
        if !cases.join(name).is_file() {
            return Err(UsageError::new(format!(
                "{MANIFEST_FILE} line {} names unknown case {name:?}",
                index + 1
            )));
        }
        if expectations.insert(name.to_owned(), expectation).is_some() {
            return Err(UsageError::new(format!(
                "{MANIFEST_FILE} line {} repeats case {name:?}",
                index + 1
            )));
        }
    }
    Ok(expectations)
}

/// Parse `WxH`; both dimensions must be positive and bounded.
fn parse_dimensions(text: &str) -> Option<(u32, u32)> {
    let (width, height) = text.split_once('x')?;
    let width = width.parse::<u32>().ok()?;
    let height = height.parse::<u32>().ok()?;
    if width == 0
        || height == 0
        || width > MAX_EXPECTED_DIMENSION
        || height > MAX_EXPECTED_DIMENSION
    {
        return None;
    }
    Some((width, height))
}

/// Run every admitted case against every candidate and return ordered observations.
///
/// `max_observations` bounds how many observations are collected, and the record is
/// trimmed in a single pass against `max_record_bytes`; both truncations set
/// `observations_truncated` instead of silently shortening the record.
pub fn run(config: &Config) -> Result<Report, UsageError> {
    let (work, captures) = output_directories(config)?;
    let collected = collect(config, &work, &captures);
    // Only this run's own directory is removed, whatever the outcome; anything that
    // already lived in the work root is never touched.
    remove_run_directory(&work);
    collected
}

fn collect(config: &Config, work: &Path, captures: &Path) -> Result<Report, UsageError> {
    let mut candidates = config.candidates.clone();
    candidates.sort_by(|left, right| left.id.cmp(&right.id));
    let launchers: Vec<Launcher> = candidates
        .iter()
        .map(|candidate| resolve_launcher(&candidate.program))
        .collect();
    let identities: Vec<CandidateRecord> = candidates
        .iter()
        .zip(&launchers)
        .map(|(candidate, launcher)| {
            let probed = probe_identity(config, candidate, launcher, captures);
            CandidateRecord {
                id: candidate.id.clone(),
                kind: candidate.program.kind(),
                program: candidate.program.path().to_path_buf(),
                launcher: launcher.path.clone(),
                identity: probed.identity,
                identity_probe: probed.probe,
                identity_exit_codes: probed.exit_codes,
                refusal: probed.refusal,
            }
        })
        .collect();
    let (cases, cases_truncated) = scan_cases(config)?;
    let mut observations: Vec<Observation> = Vec::new();
    let mut observations_truncated = false;
    'cases: for (case_index, case) in cases.iter().enumerate() {
        for (candidate, launcher) in candidates.iter().zip(&launchers) {
            if observations.len() >= config.max_observations {
                observations_truncated = true;
                break 'cases;
            }
            observations.push(measure(
                config, candidate, launcher, case, work, captures, case_index,
            ));
        }
    }
    let mut report = Report {
        schema_version: SCHEMA_VERSION,
        exploratory: true,
        numeric_gate: NUMERIC_GATE,
        cases_dir: config.cases.clone(),
        timeout_ms: config.timeout_ms,
        max_output_bytes: config.max_output_bytes,
        max_output_file_bytes: config.max_output_file_bytes,
        max_cases: config.max_cases,
        max_observations: config.max_observations,
        max_record_bytes: config.max_record_bytes,
        max_pixels: config.max_pixels,
        cases_truncated,
        observations_truncated,
        candidates: identities,
        observations,
        capabilities: Vec::new(),
    };
    // One rendering pass decides how many observations fit the byte budget.
    let (_, kept) = render(&report);
    if kept < report.observations.len() {
        report.observations.truncate(kept);
        report.observations_truncated = true;
    }
    // A3: the capability table is derived from the observations that survived the
    // byte budget, so a dropped row can never contribute a capability claim.
    report.capabilities = capability_rows(&report.observations);
    if render(&report).0.len() > config.max_record_bytes {
        return Err(UsageError::new(format!(
            "the record header alone exceeds the {} byte cap",
            config.max_record_bytes
        )));
    }
    Ok(report)
}

/// Create the work root if needed and a fresh, unique run directory inside it.
///
/// Nothing that already exists in the root is ever cleared, renamed or deleted:
/// the run directory is created with `create_dir`, which fails if the token is
/// already taken, and only that new directory is used for outputs and captures.
fn output_directories(config: &Config) -> Result<(PathBuf, PathBuf), UsageError> {
    let parent = config
        .out
        .parent()
        .ok_or_else(|| UsageError::new("--out must name a file inside a directory"))?;
    let root = parent.join(OUTPUT_DIR);
    fs::create_dir_all(&root)
        .map_err(|error| UsageError::new(format!("cannot create {}: {error}", root.display())))?;
    let work = create_run_directory(&root, &run_token())?;
    let captures = work.join(CAPTURE_DIR);
    if let Err(error) = fs::create_dir_all(&captures) {
        remove_run_directory(&work);
        return Err(UsageError::new(format!(
            "cannot create {}: {error}",
            captures.display()
        )));
    }
    Ok((work, captures))
}

/// A fresh run token: process id, wall clock and a process-local sequence number.
pub fn run_token() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    format!(
        "{RUN_TOKEN_PREFIX}{}-{nanos}-{sequence}",
        std::process::id()
    )
}

/// Create this run's own directory; a token that already exists is refused rather
/// than reused, so two runs can never share a directory.
pub fn create_run_directory(root: &Path, token: &str) -> Result<PathBuf, UsageError> {
    let token_ok = !token.is_empty()
        && token.len() <= MAX_CASE_NAME_BYTES
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if !token_ok {
        return Err(UsageError::new(format!(
            "run token {token:?} is not usable as a directory name"
        )));
    }
    let directory = root.join(token);
    match fs::create_dir(&directory) {
        Ok(()) => Ok(directory),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(UsageError::new(
            format!("{WORK_DIR_EXISTS}: {} already exists", directory.display()),
        )),
        Err(error) => Err(UsageError::new(format!(
            "cannot create {}: {error}",
            directory.display()
        ))),
    }
}

/// Remove this run's own directory. Failure is reported but never fatal: the run
/// directory is ours alone and nothing else is touched either way.
fn remove_run_directory(directory: &Path) {
    if let Err(error) = fs::remove_dir_all(directory) {
        eprintln!(
            "event=decode-compare cleanup-deferred dir={} error={error}",
            directory.display()
        );
    }
}

/// Admit only direct regular files with a UTF-8 name, sorted, bounded by max_cases.
fn scan_cases(config: &Config) -> Result<(Vec<Case>, bool), UsageError> {
    let entries = fs::read_dir(&config.cases).map_err(|error| {
        UsageError::new(format!("cannot list {}: {error}", config.cases.display()))
    })?;
    let mut names: Vec<(String, PathBuf)> = Vec::new();
    let mut examined = 0_usize;
    for entry in entries {
        examined += 1;
        if examined > MAX_DIR_ENTRIES {
            return Err(UsageError::new(format!(
                "{} has more than {MAX_DIR_ENTRIES} entries",
                config.cases.display()
            )));
        }
        let entry = entry.map_err(|error| {
            UsageError::new(format!(
                "cannot read a {} entry: {error}",
                config.cases.display()
            ))
        })?;
        let file_type = entry.file_type().map_err(|error| {
            UsageError::new(format!(
                "cannot inspect a {} entry: {error}",
                config.cases.display()
            ))
        })?;
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name().into_string().map_err(|_| {
            UsageError::new(format!(
                "{} contains a case name that is not valid UTF-8",
                config.cases.display()
            ))
        })?;
        if name == MANIFEST_FILE {
            continue;
        }
        if name.len() > MAX_CASE_NAME_BYTES {
            return Err(UsageError::new(format!(
                "case name {name:?} exceeds {MAX_CASE_NAME_BYTES} bytes"
            )));
        }
        names.push((name, entry.path()));
    }
    names.sort_by(|left, right| left.0.cmp(&right.0));
    let truncated = names.len() > config.max_cases;
    names.truncate(config.max_cases);
    let mut cases = Vec::with_capacity(names.len());
    for (id, path) in names {
        let input_bytes = fs::metadata(&path)
            .ok()
            .filter(std::fs::Metadata::is_file)
            .map(|metadata| metadata.len());
        let suffix = Path::new(&id)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_owned);
        cases.push(Case {
            id,
            path,
            suffix,
            input_bytes,
        });
    }
    Ok((cases, truncated))
}

/// Resolve the program to launch: the candidate itself, or the script interpreter.
fn resolve_launcher(program: &Program) -> Launcher {
    match program {
        Program::Executable(path) => Launcher {
            command: path.clone().into_os_string(),
            path: Some(path.clone()),
        },
        Program::PwshScript(_) => match resolve_on_path(PWSH_PROGRAM) {
            Some(path) => Launcher {
                command: path.clone().into_os_string(),
                path: Some(path),
            },
            // The bare name is still launched by the operating system; the record
            // then says the absolute interpreter was not observed.
            None => Launcher {
                command: OsString::from(PWSH_PROGRAM),
                path: None,
            },
        },
    }
}

/// Bounded `PATH` search for the script interpreter, without a shell.
fn resolve_on_path(program: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&paths).take(MAX_PATH_ENTRIES) {
        if !directory.is_absolute() {
            continue;
        }
        let candidate = if cfg!(windows) {
            directory.join(format!("{program}.exe"))
        } else {
            directory.join(program)
        };
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// The case output path. An executable keeps the case suffix because a
/// format-inferring tool needs it; the script transport always writes a PNG, so its
/// encoder format never depends on the input suffix and a valid `.jpg`/`.tif`/`.gif`
/// input is decoded instead of reported as unsupported.
///
/// The candidate id is part of every name (B1): two candidates on the same case can
/// therefore never share a path, so one candidate's output can never be attributed
/// to another that wrote nothing.
pub fn output_path(
    program: &Program,
    work: &Path,
    case_index: usize,
    case_id: &str,
    candidate_id: &str,
    suffix: &str,
) -> PathBuf {
    let stem = format!("{:03}_{}", case_index, sanitize_name(case_id));
    match program {
        Program::Executable(_) => work.join(format!("{stem}.{candidate_id}.{suffix}")),
        Program::PwshScript(_) => work.join(format!("{stem}.{candidate_id}.wic.png")),
    }
}

/// Prove the output path this observation will measure does not exist yet (B1).
///
/// The run directory is fresh, so this can only trip on a path the harness itself
/// reuses; refusing keeps a foreign or shared file from being read as this child's
/// evidence. Any error other than `NotFound` also refuses, because an unverifiable
/// path must not be trusted.
pub fn require_absent_output(path: &Path) -> Result<(), &'static str> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err("output_stale_present"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("output_unverifiable"),
    }
}

/// The exact argv handed to a measurement process, argv prefix included. Kept
/// separate from process creation so the shipped transport is directly testable.
pub fn measurement_argv(
    invocation: &Invocation,
    program: &Program,
    input: &Path,
    output: &Path,
    max_pixels: Option<u64>,
) -> Vec<OsString> {
    let mut argv: Vec<OsString> = invocation.argv_prefix.iter().map(OsString::from).collect();
    if invocation.env_transport {
        return argv;
    }
    match program {
        Program::Executable(_) => {
            argv.push(OsString::from(INPUT_FLAG));
            argv.push(input.as_os_str().to_os_string());
            argv.push(output.as_os_str().to_os_string());
        }
        Program::PwshScript(script) => {
            argv.push(OsString::from("-NoProfile"));
            argv.push(OsString::from("-File"));
            argv.push(script.as_os_str().to_os_string());
            argv.push(OsString::from("-InputPath"));
            argv.push(input.as_os_str().to_os_string());
            argv.push(OsString::from("-OutputPath"));
            argv.push(output.as_os_str().to_os_string());
            if let Some(pixels) = max_pixels {
                argv.push(OsString::from("-MaxPixels"));
                argv.push(OsString::from(pixels.to_string()));
            }
        }
    }
    argv
}

/// The exact argv handed to an identity probe, argv prefix included.
pub fn probe_argv(invocation: &Invocation, program: &Program, form: ProbeForm) -> Vec<OsString> {
    let mut argv: Vec<OsString> = invocation.argv_prefix.iter().map(OsString::from).collect();
    if invocation.env_transport {
        return argv;
    }
    match program {
        Program::Executable(_) => argv.push(OsString::from(form.as_str())),
        Program::PwshScript(script) => {
            argv.push(OsString::from("-NoProfile"));
            argv.push(OsString::from("-File"));
            argv.push(script.as_os_str().to_os_string());
            argv.push(OsString::from(ProbeForm::Script.as_str()));
        }
    }
    argv
}

fn probe_identity(
    config: &Config,
    candidate: &Candidate,
    launcher: &Launcher,
    captures: &Path,
) -> ProbedIdentity {
    let mut probed = ProbedIdentity::default();
    for (index, form) in candidate.program.probe_forms().iter().enumerate() {
        let stem = format!("probe.{index}.{}", candidate.id);
        let Some((capture, stdout_stdio, stderr_stdio)) = CaptureFiles::open(captures, &stem)
        else {
            // An identity that could not even be attempted is refused explicitly.
            probed.refusal = Some("capture_uncreatable");
            break;
        };
        let mut command = Command::new(&launcher.command);
        command
            .args(probe_argv(&config.invocation, &candidate.program, *form))
            .stdin(Stdio::null())
            .stdout(stdout_stdio)
            .stderr(stderr_stdio);
        if config.invocation.env_transport {
            command.env(ENV_PROBE, form.as_str());
        }
        let supervised = command.spawn().ok().map(|child| {
            supervise(
                child,
                Duration::from_millis(config.timeout_ms),
                Some(probe_bounds(&capture)),
            )
        });
        let (stdout, stderr) = capture.read(PROBE_CAP_BYTES);
        capture.release();
        let Some(supervised) = supervised else {
            continue;
        };
        probed.exit_codes[index] = supervised.exit_code;
        if supervised.timeout_fired
            || supervised.capped.is_some()
            || supervised.exit_code != Some(0)
        {
            continue;
        }
        let answer = if config.invocation.env_transport {
            &stderr
        } else {
            &stdout
        };
        if let Some(identity) = identity_line(answer) {
            probed.identity = Some(identity);
            probed.probe = Some(form.as_str());
            break;
        }
    }
    probed
}

fn probe_bounds(capture: &CaptureFiles) -> Bounds<'_> {
    Bounds {
        first: &capture.stdout,
        second: &capture.stderr,
        scratch_limit: u64::try_from(PROBE_CAP_BYTES)
            .unwrap_or(u64::MAX)
            .saturating_mul(SCRATCH_FACTOR),
        output: None,
        output_limit: 0,
    }
}

/// A complete first line only; a truncated line is not an observed identity.
fn identity_line(capture: &Capture) -> Option<String> {
    let end = match capture.bytes.iter().position(|byte| *byte == b'\n') {
        Some(position) => position,
        None if capture.truncated => return None,
        None => capture.bytes.len(),
    };
    let line = std::str::from_utf8(&capture.bytes[..end]).ok()?;
    let identity: String = line
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_IDENTITY_CHARS)
        .collect();
    let identity = identity.trim();
    if identity.is_empty() {
        return None;
    }
    Some(identity.to_owned())
}

/// Whitelisted candidate reason: a `"reason_code":"..."` token from the captured
/// stdout, mapped through [`KNOWN_REASONS`]. Anything else is `not_measured`, so
/// foreign text is never copied into the record.
fn candidate_reason(capture: &Capture) -> &'static str {
    const FIELD: &str = "\"reason_code\":\"";
    let Ok(text) = std::str::from_utf8(&capture.bytes) else {
        return NOT_MEASURED;
    };
    let Some(start) = text.find(FIELD) else {
        return NOT_MEASURED;
    };
    let rest = &text[start + FIELD.len()..];
    let end = rest.find('"').unwrap_or(0);
    let token = &rest[..end];
    if token.is_empty()
        || token.len() > MAX_REASON_CHARS
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return NOT_MEASURED;
    }
    KNOWN_REASONS
        .iter()
        .find(|known| **known == token)
        .copied()
        .unwrap_or(NOT_MEASURED)
}

/// One redirected capture stream: the parent owns the files, the child owns the
/// handles. Because nothing is piped, a descendant that inherits those handles
/// cannot keep this process waiting.
struct CaptureFiles {
    stdout: PathBuf,
    stderr: PathBuf,
}

impl CaptureFiles {
    /// Create fresh capture files (they are truncated, so no stale bytes survive)
    /// and hand the child the matching stdio. Failure is a refusal, never a
    /// silent fallback.
    fn open(directory: &Path, stem: &str) -> Option<(Self, Stdio, Stdio)> {
        let files = Self {
            stdout: directory.join(format!("{stem}.stdout")),
            stderr: directory.join(format!("{stem}.stderr")),
        };
        let stdout = fs::File::create(&files.stdout).ok()?;
        let stderr = fs::File::create(&files.stderr).ok()?;
        Some((files, Stdio::from(stdout), Stdio::from(stderr)))
    }

    fn read(&self, cap: usize) -> (Capture, Capture) {
        (
            read_capture(&self.stdout, cap),
            read_capture(&self.stderr, cap),
        )
    }

    fn release(&self) {
        let _ = fs::remove_file(&self.stdout);
        let _ = fs::remove_file(&self.stderr);
    }
}

/// Read at most `cap` bytes and report whether the file held more.
fn read_capture(path: &Path, cap: usize) -> Capture {
    let Ok(file) = fs::File::open(path) else {
        return Capture::default();
    };
    let limit = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
    let mut bytes = Vec::new();
    if file.take(limit).read_to_end(&mut bytes).is_err() {
        return Capture::default();
    }
    let truncated = bytes.len() > cap;
    bytes.truncate(cap);
    Capture { bytes, truncated }
}

fn measure(
    config: &Config,
    candidate: &Candidate,
    launcher: &Launcher,
    case: &Case,
    work: &Path,
    captures: &Path,
    case_index: usize,
) -> Observation {
    let mut observation = Observation {
        candidate_id: candidate.id.clone(),
        case_id: case.id.clone(),
        input_path: case.path.clone(),
        suffix: case.suffix.clone(),
        input_bytes: case.input_bytes,
        declared_outcome: config
            .expectations
            .get(&case.id)
            .and_then(|expectation| expectation.outcome)
            .unwrap_or(OutcomeClass::NotMeasured),
        observed_outcome: OutcomeClass::NotRun,
        candidate_reason: NOT_MEASURED,
        inspected_format: Format::NotMeasured,
        inspected_complete: false,
        inspected_width: None,
        inspected_height: None,
        inspected_bit_depth: None,
        inspected_channels: None,
        inspected_colour_type: ColourType::NotMeasured,
        inspected_colour_markers: Vec::new(),
        expected_dimensions: config
            .expectations
            .get(&case.id)
            .and_then(|expectation| expectation.dimensions),
        pixel_expectation: config
            .expectations
            .get(&case.id)
            .and_then(|expectation| expectation.pixel_digest),
        pixel_status: PixelStatus::NotRequested,
        pixel_digest: None,
        pixel_verified: false,
        pixel_reason: None,
        validity_reason: None,
        exit_code: None,
        timeout_fired: false,
        captured_stdout_bytes: 0,
        captured_stderr_bytes: 0,
        stdout_truncated: false,
        stderr_truncated: false,
        wall_ms: None,
        scratch_bytes: None,
        output_present: false,
        output_bytes: None,
        validity: NOT_MEASURED,
        refusal: None,
    };
    // Without a case suffix the case identity cannot be expressed in the record,
    // so no child is started and nothing is claimed.
    let Some(suffix) = case.suffix.as_deref() else {
        observation.observed_outcome = OutcomeClass::NotMeasured;
        observation.refusal = Some("extension_needed_for_output");
        return observation;
    };
    let output = output_path(
        &candidate.program,
        work,
        case_index,
        &case.id,
        &candidate.id,
        suffix,
    );
    let stem = format!(
        "{:03}_{}.{}",
        case_index,
        sanitize_name(&case.id),
        candidate.id
    );
    if let Err(reason) = require_absent_output(&output) {
        observation.observed_outcome = OutcomeClass::NotRun;
        observation.refusal = Some(reason);
        return observation;
    }
    let Some((capture, stdout_stdio, stderr_stdio)) = CaptureFiles::open(captures, &stem) else {
        observation.observed_outcome = OutcomeClass::NotRun;
        observation.refusal = Some("capture_uncreatable");
        return observation;
    };
    let mut command = Command::new(&launcher.command);
    command
        .args(measurement_argv(
            &config.invocation,
            &candidate.program,
            &case.path,
            &output,
            config.max_pixels,
        ))
        .stdin(Stdio::null())
        .stdout(stdout_stdio)
        .stderr(stderr_stdio);
    if config.invocation.env_transport {
        command.env(ENV_INPUT, &case.path).env(ENV_OUTPUT, &output);
    }
    match command.spawn() {
        Ok(child) => {
            let bounds = Bounds {
                first: &capture.stdout,
                second: &capture.stderr,
                scratch_limit: u64::try_from(config.max_output_bytes)
                    .unwrap_or(u64::MAX)
                    .saturating_mul(SCRATCH_FACTOR),
                output: Some(&output),
                output_limit: u64::try_from(config.max_output_file_bytes).unwrap_or(u64::MAX),
            };
            let supervised = supervise(
                child,
                Duration::from_millis(config.timeout_ms),
                Some(bounds),
            );
            let (stdout, stderr) = capture.read(config.max_output_bytes);
            let (output_bytes, output_present) = match fs::metadata(&output) {
                Ok(metadata) if metadata.is_file() => (Some(metadata.len()), metadata.len() > 0),
                _ => (None, false),
            };
            observation.wall_ms = Some(supervised.wall_ms);
            observation.scratch_bytes = Some(supervised.scratch_bytes);
            observation.output_bytes = output_bytes;
            observation.output_present = output_present;
            observation.timeout_fired = supervised.timeout_fired;
            observation.exit_code = supervised.exit_code;
            observation.candidate_reason = candidate_reason(&stdout);
            observation.captured_stdout_bytes = stdout.bytes.len();
            observation.captured_stderr_bytes = stderr.bytes.len();
            observation.stdout_truncated = stdout.truncated;
            observation.stderr_truncated = stderr.truncated;
            // B-03: a capped run is never a success class, even when the child
            // exited normally before the post-exit re-check saw the size.
            // B-02: exit 0 alone is not evidence; a non-empty present output is.
            observation.observed_outcome = match supervised.capped {
                Some(kind) => {
                    observation.refusal = Some(kind.refusal());
                    OutcomeClass::Cancelled
                }
                None if supervised.timeout_fired => OutcomeClass::Cancelled,
                None if supervised.exit_code == Some(0) && output_present => OutcomeClass::Decoded,
                None if supervised.exit_code == Some(0) => {
                    observation.refusal = Some("output_missing");
                    OutcomeClass::Error
                }
                None => classify(supervised.exit_code),
            };
            // Phase 2: verify the bytes the candidate wrote, never its exit code.
            let inspection = if output_present {
                match fs::File::open(&output) {
                    Ok(mut file) => inspect::inspect(&mut file),
                    Err(_) => Inspection::default(),
                }
            } else {
                Inspection::default()
            };
            observation.inspected_format = inspection.format.unwrap_or(Format::NotMeasured);
            observation.inspected_complete = inspection.complete;
            observation.inspected_width = inspection.width;
            observation.inspected_height = inspection.height;
            observation.inspected_bit_depth = inspection.bit_depth;
            observation.inspected_channels = inspection.channels;
            observation.inspected_colour_type =
                inspection.colour_type.unwrap_or(ColourType::NotMeasured);
            observation.inspected_colour_markers = inspection.colour_markers.clone();
            // Phase 3: when an operator declares pixel expectations, decode the
            // output with the independent oracle and compare its RGBA digest.
            if let Some(expected) = observation.pixel_expectation {
                if output_present && inspection.complete {
                    match fs::File::open(&output) {
                        Ok(mut file) => match pixels::read_png(&mut file) {
                            Ok(image) => {
                                let digest = image.digest();
                                observation.pixel_status = if digest == expected {
                                    PixelStatus::Verified
                                } else {
                                    PixelStatus::Mismatch
                                };
                                observation.pixel_digest = Some(digest);
                            }
                            Err(error) => {
                                observation.pixel_status = PixelStatus::Unavailable;
                                observation.pixel_reason = Some(error.as_str());
                            }
                        },
                        Err(_) => {
                            observation.pixel_status = PixelStatus::Unavailable;
                            observation.pixel_reason =
                                Some(pixels::PixelError::Unreadable.as_str());
                        }
                    }
                } else if observation.observed_outcome == OutcomeClass::Decoded {
                    // Structurally not a complete container: the pixel verdict cannot
                    // be claimed either, and the structure verdict already says why.
                    observation.pixel_status = PixelStatus::Unavailable;
                    observation.pixel_reason = Some(if inspection.complete {
                        PIXEL_UNAVAILABLE
                    } else {
                        OUTPUT_INCOMPLETE
                    });
                }
            }
            let (validity, reason) = verify(
                observation.observed_outcome,
                observation.expected_dimensions,
                inspection.dimensions(),
                inspection.complete,
                observation.pixel_expectation.is_some(),
                observation.pixel_status,
            );
            observation.validity = validity;
            observation.validity_reason = reason;
            observation.pixel_verified = validity == PIXELS_VERIFIED;
            // A verdict the evidence contradicts can never keep a success class.
            // An expectation-bearing output that is contradicted, unparsable or
            // incomplete is a contract failure, so it is `error` with the named
            // refusal (N5) while the raw exit code stays recorded as evidence.
            match (validity, reason) {
                (PIXEL_MISMATCH, _) => {
                    observation.observed_outcome = OutcomeClass::Error;
                    observation.refusal = Some(PIXEL_MISMATCH);
                }
                (DIMENSION_MISMATCH, _) => {
                    observation.observed_outcome = OutcomeClass::Error;
                    observation.refusal = Some(DIMENSION_MISMATCH);
                }
                (_, Some(PIXEL_UNAVAILABLE)) => {
                    observation.observed_outcome = OutcomeClass::Error;
                    observation.refusal = Some(PIXEL_UNAVAILABLE);
                }
                (_, Some(OUTPUT_UNPARSABLE)) => {
                    observation.observed_outcome = OutcomeClass::Error;
                    observation.refusal = Some(OUTPUT_UNPARSABLE);
                }
                (_, Some(OUTPUT_INCOMPLETE)) => {
                    observation.observed_outcome = OutcomeClass::Error;
                    observation.refusal = Some(OUTPUT_INCOMPLETE);
                }
                _ => {}
            }
        }
        Err(error) => {
            observation.refusal = Some(if error.kind() == std::io::ErrorKind::NotFound {
                "candidate_not_found"
            } else {
                "candidate_spawn_failed"
            });
        }
    }
    capture.release();
    observation
}

/// Aggregate the parsed outputs into one row per candidate × observed format.
///
/// Only outputs the inspector itself recognised *and* measured are counted, and a
/// capped (cancelled) run is not capability evidence. Values are sorted and
/// de-duplicated so the table is deterministic; anything the inspector could not
/// read stays empty here and is rendered as `not_measured` in the record.
fn capability_rows(observations: &[Observation]) -> Vec<CapabilityRow> {
    let mut rows: BTreeMap<(String, Format), CapabilityRow> = BTreeMap::new();
    for observation in observations {
        // A1/B2: only a decoded observation with a complete container and a
        // presence/content validity is capability evidence; refusals, mismatches
        // and not_measured rows never are.
        if observation.observed_outcome != OutcomeClass::Decoded {
            continue;
        }
        if !observation.inspected_complete {
            continue;
        }
        if !matches!(
            observation.validity,
            PRESENCE_ONLY | STRUCTURE_VERIFIED | PIXELS_VERIFIED
        ) {
            continue;
        }
        let format = observation.inspected_format;
        if format == Format::NotMeasured {
            continue;
        }
        let (Some(width), Some(height)) =
            (observation.inspected_width, observation.inspected_height)
        else {
            continue;
        };
        let row = rows
            .entry((observation.candidate_id.clone(), format))
            .or_insert_with(|| CapabilityRow {
                candidate_id: observation.candidate_id.clone(),
                format,
                observations: 0,
                max_width: None,
                max_height: None,
                bit_depths: Vec::new(),
                channels: Vec::new(),
                colour_types: Vec::new(),
                colour_markers: Vec::new(),
                pixel_verified: 0,
            });
        row.observations += 1;
        if observation.validity == PIXELS_VERIFIED {
            row.pixel_verified += 1;
        }
        row.max_width = Some(row.max_width.map_or(width, |current| current.max(width)));
        row.max_height = Some(row.max_height.map_or(height, |current| current.max(height)));
        push_unique(&mut row.bit_depths, observation.inspected_bit_depth);
        push_unique(&mut row.channels, observation.inspected_channels);
        push_unique_colour(&mut row.colour_types, observation.inspected_colour_type);
        row.colour_markers
            .extend(observation.inspected_colour_markers.iter().copied());
        row.colour_markers.sort();
        row.colour_markers.dedup();
    }
    rows.into_values().collect()
}

fn push_unique(values: &mut Vec<u32>, value: Option<u32>) {
    if let Some(value) = value
        && !values.contains(&value)
    {
        values.push(value);
        values.sort_unstable();
    }
}

fn push_unique_colour(values: &mut Vec<ColourType>, value: ColourType) {
    if value != ColourType::NotMeasured && !values.contains(&value) {
        values.push(value);
        values.sort();
    }
}

/// Decide the validity claim from the class, the operator expectation and what the
/// inspector actually read. A parsed output that contradicts the expectation is a
/// mismatch, never a success class; an unparsable output with an expectation is
/// `not_measured`, and the caller demotes the class so no unsupported success claim
/// survives in the record.
/// Decide the validity claim. Precedence is explicit: **pixel expectation >
/// dimension expectation > structure**. Structure is checked first because nothing
/// can be claimed about pixels or dimensions of a container that is not complete; a
/// declared pixel digest then decides the verdict on its own (a matching digest is
/// `pixels_verified`, a differing one `pixel_mismatch`, an undecodable one
/// `not_measured` + `pixel_unavailable`); only without a pixel expectation do the
/// dimension rules of Phase 2 apply.
fn verify(
    outcome: OutcomeClass,
    expected: Option<(u32, u32)>,
    observed: Option<(u32, u32)>,
    complete: bool,
    pixel_expected: bool,
    pixel: PixelStatus,
) -> (&'static str, Option<&'static str>) {
    if outcome != OutcomeClass::Decoded {
        return (NOT_MEASURED, None);
    }
    // A declared expectation needs a readable, complete container before anything can
    // be claimed about its dimensions or pixels. Without an expectation the Phase 1
    // presence contract applies unchanged: only presence is claimed, whatever the
    // bytes turn out to be.
    if (expected.is_some() || pixel_expected) && (!complete || observed.is_none()) {
        return (
            NOT_MEASURED,
            Some(if observed.is_none() {
                OUTPUT_UNPARSABLE
            } else {
                OUTPUT_INCOMPLETE
            }),
        );
    }
    if pixel_expected {
        return match pixel {
            PixelStatus::Verified => (PIXELS_VERIFIED, None),
            PixelStatus::Mismatch => (PIXEL_MISMATCH, Some(PIXEL_MISMATCH)),
            PixelStatus::Unavailable | PixelStatus::NotRequested => {
                (NOT_MEASURED, Some(PIXEL_UNAVAILABLE))
            }
        };
    }
    match (expected, observed) {
        (Some(expected), Some(observed)) if expected == observed => (STRUCTURE_VERIFIED, None),
        (Some(_), Some(_)) => (DIMENSION_MISMATCH, Some(DIMENSION_MISMATCH)),
        (Some(_), None) => (NOT_MEASURED, Some(OUTPUT_UNPARSABLE)),
        (None, _) => (PRESENCE_ONLY, Some(NO_EXPECTATION)),
    }
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .take(OUTPUT_NAME_CHARS)
        .collect()
}

/// Map a non-zero exit code to its class. Exit code `0` never reaches this
/// function: the evidence rule in [`measure`] decides it, so a bare success code
/// can never become `decoded` here.
fn classify(exit_code: Option<i32>) -> OutcomeClass {
    match exit_code {
        Some(2) => OutcomeClass::Unsupported,
        Some(3) => OutcomeClass::Corrupt,
        Some(4) => OutcomeClass::Oversize,
        Some(5) => OutcomeClass::Cancelled,
        Some(_) => OutcomeClass::Error,
        None => OutcomeClass::NotMeasured,
    }
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

/// Which documented bound the observed sizes exceed, if any.
fn exceeded(bounds: &Bounds<'_>) -> Option<CapKind> {
    let scratch = file_len(bounds.first).saturating_add(file_len(bounds.second));
    if scratch > bounds.scratch_limit {
        return Some(CapKind::Scratch);
    }
    if let Some(output) = bounds.output
        && file_len(output) > bounds.output_limit
    {
        return Some(CapKind::Output);
    }
    None
}

/// Supervise one child under a hard deadline and the observed byte bounds.
///
/// Capture lives in files, so nothing here can block on a descendant that
/// inherited the child's handles. The size limits are **observed**, not enforced
/// by the operating system: they are sampled while polling and checked again after
/// the child is reaped, so a burst that finishes between two polls is still
/// classified as capped instead of a success. Kill/reap applies to the direct child
/// only (A-01); termination of descendants is neither guaranteed nor claimed.
fn supervise(mut child: Child, timeout: Duration, bounds: Option<Bounds<'_>>) -> Supervised {
    let started = Instant::now();
    let mut timeout_fired = false;
    let mut killed = false;
    let mut capped = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            // A supervision error still reaps the child before returning.
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                killed = true;
                break None;
            }
        }
        if let Some(bounds) = bounds.as_ref()
            && let Some(kind) = exceeded(bounds)
        {
            let _ = child.kill();
            let _ = child.wait();
            killed = true;
            capped = Some(kind);
            break None;
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            let _ = child.kill();
            let _ = child.wait();
            killed = true;
            timeout_fired = true;
            break None;
        }
        thread::sleep(remaining.min(POLL_INTERVAL));
    };
    let scratch_bytes = bounds
        .as_ref()
        .map_or(0, |bounds| file_len(bounds.first) + file_len(bounds.second));
    let output_bytes = bounds
        .as_ref()
        .and_then(|bounds| bounds.output)
        .map_or(0, file_len);
    // B-03: re-check after exit, because an exited child may have written more
    // than a single poll could observe.
    if let Some(bounds) = bounds.as_ref()
        && capped.is_none()
        && !timeout_fired
    {
        if scratch_bytes > bounds.scratch_limit {
            capped = Some(CapKind::Scratch);
        } else if output_bytes > bounds.output_limit && bounds.output.is_some() {
            capped = Some(CapKind::Output);
        }
    }
    Supervised {
        exit_code: if killed {
            None
        } else {
            status.and_then(|status| status.code())
        },
        timeout_fired,
        wall_ms: elapsed_millis(started.elapsed()),
        scratch_bytes,
        capped,
    }
}

fn elapsed_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Render the record within its byte budget in one pass: observation rows are
/// appended while they fit, and the number kept is returned so the caller can
/// record the truncation. Unobserved values are `null`.
pub fn render(report: &Report) -> (String, usize) {
    let candidates: Vec<String> = report
        .candidates
        .iter()
        .map(|candidate| {
            format!(
                "    {{\"id\": {}, \"kind\": {}, \"program\": {}, \"launcher\": {}, \
                 \"identity\": {}, \"identity_probe\": {}, \"identity_exit_codes\": [{}, {}], \
                 \"refusal\": {}}}",
                quoted(&candidate.id),
                quoted(candidate.kind),
                path_field(&candidate.program),
                path_field_opt(candidate.launcher.as_deref()),
                text_field(candidate.identity.as_deref()),
                text_field(candidate.identity_probe),
                int_field(candidate.identity_exit_codes[0]),
                int_field(candidate.identity_exit_codes[1]),
                text_field(candidate.refusal)
            )
        })
        .collect();
    let mut json = String::new();
    json.push_str("{\n");
    let _ = writeln!(json, "  \"schema_version\": {},", report.schema_version);
    let _ = writeln!(json, "  \"exploratory\": {},", report.exploratory);
    let _ = writeln!(json, "  \"numeric_gate\": {},", quoted(report.numeric_gate));
    let _ = writeln!(json, "  \"cases_dir\": {},", path_field(&report.cases_dir));
    let _ = writeln!(json, "  \"timeout_ms\": {},", report.timeout_ms);
    let _ = writeln!(json, "  \"max_output_bytes\": {},", report.max_output_bytes);
    let _ = writeln!(
        json,
        "  \"max_output_file_bytes\": {},",
        report.max_output_file_bytes
    );
    let _ = writeln!(json, "  \"max_cases\": {},", report.max_cases);
    let _ = writeln!(json, "  \"max_observations\": {},", report.max_observations);
    let _ = writeln!(json, "  \"max_record_bytes\": {},", report.max_record_bytes);
    let _ = writeln!(
        json,
        "  \"max_pixels\": {},",
        number_field(report.max_pixels)
    );
    let _ = writeln!(json, "  \"cases_truncated\": {},", report.cases_truncated);
    let _ = writeln!(
        json,
        "  \"observations_truncated\": {},",
        report.observations_truncated
    );
    let _ = writeln!(json, "  \"candidates\": {},", json_array(&candidates));
    let capabilities: Vec<String> = report.capabilities.iter().map(capability_row).collect();
    let _ = writeln!(json, "  \"capabilities\": {},", json_array(&capabilities));
    json.push_str("  \"observations\": ");
    let budget = report.max_record_bytes;
    let mut rows: Vec<String> = Vec::new();
    let mut used = 0_usize;
    for observation in &report.observations {
        let row = observation_row(observation);
        // Each row costs its separator ("[\n" for the first, ",\n" afterwards).
        let cost = 2 + row.len();
        if json.len() + used + cost + OBSERVATIONS_TAIL.len() > budget {
            break;
        }
        used += cost;
        rows.push(row);
    }
    if rows.is_empty() {
        json.push_str("[]");
    } else {
        json.push_str("[\n");
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                json.push_str(",\n");
            }
            json.push_str(row);
        }
        json.push_str("\n  ]");
    }
    json.push_str("\n}\n");
    (json, rows.len())
}

fn observation_row(observation: &Observation) -> String {
    format!(
        "    {{\"candidate_id\": {}, \"case_id\": {}, \"input_path\": {}, \"suffix\": {}, \
         \"input_bytes\": {}, \"declared_outcome\": {}, \"observed_outcome\": {}, \
         \"candidate_reason\": {}, \"exit_code\": {}, \"timeout_fired\": {}, \
         \"captured_stdout_bytes\": {}, \"captured_stderr_bytes\": {}, \"stdout_truncated\": {}, \
         \"stderr_truncated\": {}, \"wall_ms\": {}, \"scratch_bytes\": {}, \
         \"output_present\": {}, \"output_bytes\": {}, \"inspected_format\": {}, \
         \"inspected_complete\": {}, \"inspected_width\": {}, \"inspected_height\": {}, \
         \"inspected_bit_depth\": {}, \"inspected_channels\": {}, \"inspected_colour_type\": {}, \
         \"inspected_colour_markers\": {}, \"expected_dimensions\": {}, \"pixel_expectation\": {}, \
         \"pixel_digest\": {}, \"pixel_verified\": {}, \"pixel_reason\": {}, \"validity\": {}, \
         \"validity_reason\": {}, \"refusal\": {}}}",
        quoted(&observation.candidate_id),
        quoted(&observation.case_id),
        path_field(&observation.input_path),
        text_field(observation.suffix.as_deref()),
        number_field(observation.input_bytes),
        quoted(observation.declared_outcome.as_str()),
        quoted(observation.observed_outcome.as_str()),
        quoted(observation.candidate_reason),
        int_field(observation.exit_code),
        observation.timeout_fired,
        observation.captured_stdout_bytes,
        observation.captured_stderr_bytes,
        observation.stdout_truncated,
        observation.stderr_truncated,
        number_field(observation.wall_ms),
        number_field(observation.scratch_bytes),
        observation.output_present,
        number_field(observation.output_bytes),
        quoted(observation.inspected_format.as_str()),
        observation.inspected_complete,
        number_field(observation.inspected_width.map(u64::from)),
        number_field(observation.inspected_height.map(u64::from)),
        number_field(observation.inspected_bit_depth.map(u64::from)),
        number_field(observation.inspected_channels.map(u64::from)),
        quoted(observation.inspected_colour_type.as_str()),
        marker_array(&observation.inspected_colour_markers),
        dimensions_field(observation.expected_dimensions),
        digest_field(observation.pixel_expectation),
        digest_field(observation.pixel_digest),
        observation.pixel_verified,
        text_field(observation.pixel_reason),
        quoted(observation.validity),
        text_field(observation.validity_reason),
        text_field(observation.refusal),
    )
}

fn capability_row(row: &CapabilityRow) -> String {
    // N2: a set the headers never stated is named explicitly, with one reason.
    let mut unmeasured: Vec<&str> = Vec::new();
    if row.bit_depths.is_empty() {
        unmeasured.push("bit_depths");
    }
    if row.channels.is_empty() {
        unmeasured.push("channels");
    }
    if row.colour_types.is_empty() {
        unmeasured.push("colour_types");
    }
    let reason = if unmeasured.is_empty() {
        "null".to_owned()
    } else {
        quoted(HEADER_FIELD_NOT_OBSERVED)
    };
    format!(
        "    {{\"candidate_id\": {}, \"format\": {}, \"observations\": {}, \"max_width\": {}, \
         \"max_height\": {}, \"bit_depths\": {}, \"channels\": {}, \"colour_types\": {}, \
         \"colour_markers\": {}, \"pixel_verified\": {}, \"not_measured\": {}, \"not_measured_reason\": {}, \
         \"orientation\": {}, \"orientation_reason\": {}, \
         \"region\": {}, \"region_reason\": {}, \"sequential\": {}, \
         \"sequential_reason\": {}}}",
        quoted(&row.candidate_id),
        quoted(row.format.as_str()),
        row.observations,
        number_field(row.max_width.map(u64::from)),
        number_field(row.max_height.map(u64::from)),
        number_array(&row.bit_depths),
        number_array(&row.channels),
        colour_type_array(&row.colour_types),
        marker_array(&row.colour_markers),
        row.pixel_verified,
        string_list(&unmeasured),
        reason,
        quoted(NOT_MEASURED),
        quoted(ORIENTATION_REASON),
        quoted(NOT_MEASURED),
        quoted(REGION_REASON),
        quoted(NOT_MEASURED),
        quoted(SEQUENTIAL_REASON),
    )
}

fn string_list(values: &[&str]) -> String {
    let rendered: Vec<String> = values.iter().map(|value| quoted(value)).collect();
    format!("[{}]", rendered.join(", "))
}

fn digest_field(digest: Option<[u8; 32]>) -> String {
    match digest {
        Some(digest) => quoted(&pixels::hex(&digest)),
        None => "null".to_owned(),
    }
}

fn dimensions_field(dimensions: Option<(u32, u32)>) -> String {
    match dimensions {
        Some((width, height)) => quoted(&format!("{width}x{height}")),
        None => "null".to_owned(),
    }
}

fn number_array(values: &[u32]) -> String {
    let rendered: Vec<String> = values.iter().map(|value| value.to_string()).collect();
    format!("[{}]", rendered.join(", "))
}

fn colour_type_array(values: &[ColourType]) -> String {
    let rendered: Vec<String> = values.iter().map(|value| quoted(value.as_str())).collect();
    format!("[{}]", rendered.join(", "))
}

fn marker_array(values: &[ColourMarker]) -> String {
    let rendered: Vec<String> = values.iter().map(|value| quoted(value.as_str())).collect();
    format!("[{}]", rendered.join(", "))
}

fn json_array(rows: &[String]) -> String {
    if rows.is_empty() {
        return "[]".to_owned();
    }
    let mut array = String::from("[\n");
    for (index, row) in rows.iter().enumerate() {
        array.push_str(row);
        if index + 1 < rows.len() {
            array.push(',');
        }
        array.push('\n');
    }
    array.push_str("  ]");
    array
}

fn quoted(text: &str) -> String {
    let mut rendered = String::with_capacity(text.len() + 2);
    rendered.push('"');
    for character in text.chars() {
        match character {
            '"' => rendered.push_str("\\\""),
            '\\' => rendered.push_str("\\\\"),
            '\n' => rendered.push_str("\\n"),
            '\r' => rendered.push_str("\\r"),
            '\t' => rendered.push_str("\\t"),
            control if control < ' ' => {
                let _ = write!(rendered, "\\u{:04x}", u32::from(control));
            }
            other => rendered.push(other),
        }
    }
    rendered.push('"');
    rendered
}

fn path_field(path: &Path) -> String {
    match path.to_str() {
        Some(text) => quoted(text),
        None => "null".to_owned(),
    }
}

fn path_field_opt(path: Option<&Path>) -> String {
    match path {
        Some(path) => path_field(path),
        None => "null".to_owned(),
    }
}

fn text_field(text: Option<&str>) -> String {
    match text {
        Some(text) => quoted(text),
        None => "null".to_owned(),
    }
}

fn number_field(value: Option<u64>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "null".to_owned(),
    }
}

fn int_field(value: Option<i32>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "null".to_owned(),
    }
}

/// Write the record; the caller owns the exit status. The configured record budget
/// is enforced here as well, and a report whose observations do not all fit is
/// refused rather than written short.
pub fn write_report(report: &Report, out: &Path) -> std::io::Result<()> {
    let (json, kept) = render(report);
    if kept < report.observations.len() || json.len() > report.max_record_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "record exceeds the configured {} byte cap",
                report.max_record_bytes
            ),
        ));
    }
    fs::write(out, json)
}

/// Full operator CLI: `2` means refused, `1` means the record could not be written.
pub fn run_from_args<I>(args: I) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let config = match parse(args) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("decode_compare refused: {error}");
            eprintln!("{USAGE}");
            return 2;
        }
    };
    let report = match run(&config) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("decode_compare refused: {error}");
            return 2;
        }
    };
    if let Err(error) = write_report(&report, &config.out) {
        eprintln!(
            "decode_compare cannot write {}: {error}",
            config.out.display()
        );
        return 1;
    }
    eprintln!(
        "event=decode-compare candidates={} observations={} cases_truncated={} \
         observations_truncated={} record={}",
        report.candidates.len(),
        report.observations.len(),
        report.cases_truncated,
        report.observations_truncated,
        config.out.display()
    );
    0
}
