//! Exploratory, decoder-neutral MP-IMG2 comparison harness (record schema v9).
//!
//! This module is shared by the operator-run `decode_compare` example and its
//! contract tests. It is not a product decoder, selects no decoder, links no
//! FFmpeg and makes no numeric claim: the emitted record is explicitly
//! exploratory and every value it contains was actually observed.
//!
//! Candidate process protocol (v9), fixed so that no argv element or decoder path
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
//! `output_missing`. What "verified" means depends on the declared claim and is
//! always narrower than the value name suggests: `structure_verified` asserts
//! container structure, the mandatory chunk/segment fields and the declared
//! dimensions only; `pixels_verified` asserts that the harness's own PNG oracle
//! decoded the bytes and that the resulting RGBA digest equals the operator's
//! expectation. `pixels_verified` compares decoded RGBA sample values byte-for-byte
//! through a SHA-256 digest; neither verification path performs colour management,
//! profile conversion or colourimetric interpretation, and neither establishes
//! general decoder or format compatibility. `structure_verified` alone says nothing
//! about pixel content. Pixel verification covers 8-bit non-interlaced PNG outputs
//! (colour types 0/2/4/6) only; every other pixel form is `not_measured` with a
//! reason token rather than a guess.
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
//! What "verified" does not cover (NIT-1): the harness checks container structure,
//! the declared dimensions and the decoded RGBA bytes — it does **not** recompute PNG
//! chunk CRCs (only the `IEND` value is compared) and does **not** verify the zlib
//! Adler-32 trailer, so an image whose decoded bytes are correct but whose CRC or
//! Adler-32 is corrupt can still verify. Colour management, gamma chunks and EXIF are
//! not interpreted either; pixel values are compared as stored.
//!
//! Observed resources and cancellation (schema 9): sampling is **explicit opt-in**. The
//! operator names the sampler program with `--sample-tool <absolute local disk path>`;
//! without that flag nothing is sampled at all — `working_set_sampling: "not_measured"`,
//! `working_set_reason: "sampling_disabled"`, `working_set_attempts: 0`, `sample_tool:
//! null`, and no process is spawned. There is no default sampler, no `SystemRoot`
//! derivation and no environment lookup anywhere in this path, so neither a redirected
//! system directory, a substituted environment value nor a decoy executable next to the
//! harness can select or influence the sampler: an operator who does not ask for sampling
//! cannot be given a fabricated resource figure, and one who does asks for a specific
//! file by name.
//!
//! The flag is validated but never trusted: it must be an absolute local disk path (no
//! UNC, verbatim or device form, no parent component), and it is used exactly as given.
//! When sampling is enabled the harness reads that file once with a 32 MiB cap and
//! records its SHA-256; the sampler child is spawned from that same path, and the digest
//! is re-verified immediately before every spawn and again after each sample exits. A
//! mismatch discards the value and records `sample_tool_changed` — including a value that
//! already existed. The recorded digest therefore describes the file that was on disk at
//! spawn time, not a copy and not a guess. Residual risk: between the pre-spawn
//! verification and `CreateProcess` another process could still replace the file, and a
//! principal that can write the operator-named file at all is outside what a std-only
//! process can detect; the path, the digest and the per-sample re-verification make such
//! a substitution visible in the evidence.
//!
//! Each sample is a bounded `tasklist /FI "PID eq <id>" /FO CSV /NH` call (argv only, no
//! shell, hard per-sample timeout of 2000 ms — the sampler runs concurrently with the
//! child, so its cost never moves the child's deadline — at most `max_samples` sampling
//! *attempts*, successes or not). Failure accounting: opt-out and an unusable operator
//! tool take zero attempts (`sampling_disabled`, `sample_tool_missing`,
//! `sample_tool_unreadable`, `sample_tool_oversize`, `sample_tool_changed`), while a
//! spawn or per-sample failure counts the attempt it made (`sample_tool_missing` for a
//! spawn failure, `sample_timeout`, `sample_parse_failed`). No value is ever fabricated:
//! `working_set_max_bytes` is `null` unless a validated row was read, and
//! `working_set_sampling` is `not_measured` whenever no sample succeeded.
//!
//! Header compatibility (N4): schema 9 gained `sample_tool`, `sample_tool_sha256` and
//! `sample_tool_reason` additively; a reader that does not know them can ignore them,
//! and no existing field changed shape. A sample is accepted only when the row is the
//! documented five-column Windows layout, its PID column equals the supervised child and
//! its memory column has the  form; anything else is .
//!
//! Capability rows are built only from observations that are `decoded`, complete, and
//! whose validity is `presence_only`, `structure_verified` or `pixels_verified` (A1),
//! after observation truncation (A3), so a mismatch, refusal or dropped row never
//! contributes. Each row carries `pixels_verified`, the number of its observations
//! whose decoded pixels were verified (an observation row's `pixel_verified` is the
//! boolean for that single observation, a capability row's `pixels_verified` is the
//! count — NIT-3); structure and pixel evidence are therefore labelled separately,
//! and a `pixel_mismatch` row is
//! `error` so it never counts at all.
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
pub const SCHEMA_VERSION: u32 = 9;
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
[--cancel-after-ms N] [--max-output-bytes N] [--max-output-file-bytes N] \
[--max-cases N] [--max-observations N] [--max-pixels N] \
[--sample-tool ABSOLUTE_LOCAL_EXE] [--sample-interval-ms N] [--max-samples N]";
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
/// Default interval between working-set samples.
pub const DEFAULT_SAMPLE_INTERVAL_MS: u64 = 50;
/// Hard cap on samples taken from one child.
pub const MAX_SAMPLES_LIMIT: usize = 256;
/// Largest accepted sampling interval.
pub const MAX_SAMPLE_INTERVAL_MS: u64 = 60_000;
/// Bounded read used to fingerprint the sampler program.
const SAMPLE_TOOL_DIGEST_CAP: u64 = 32 * 1024 * 1024;
/// Per-sample hard timeout for the sampling command.
const SAMPLE_TIMEOUT: Duration = Duration::from_millis(2_000);
/// Bounded read of one sample's stdout.
const SAMPLE_OUTPUT_BYTES: u64 = 8 * 1024;
/// Working-set sampling tokens: observed lower bound, or why there is none.
const SAMPLED_LOWER_BOUND: &str = "sampled_lower_bound";
const SAMPLE_TOOL_MISSING: &str = "sample_tool_missing";
const SAMPLE_TIMEOUT_REASON: &str = "sample_timeout";
const SAMPLE_PARSE_FAILED: &str = "sample_parse_failed";
const SAMPLE_NO_SAMPLE: &str = "no_sample";
const SAMPLE_DISABLED: &str = "sampling_disabled";
const SAMPLE_CAPPED: &str = "sample_capped";
const RESOURCE_NO_OBSERVATION: &str = "no_contributing_observation";
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
    "cancelled_at_cancel",
    "cancelled_at_deadline",
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
    /// Interval between working-set samples; `0` disables sampling.
    pub sample_interval_ms: u64,
    /// Maximum number of working-set samples per observation.
    pub max_samples: usize,
    /// Operator-requested cancellation deadline, applied when shorter than
    /// `timeout_ms`.
    pub cancel_after_ms: Option<u64>,
    /// Operator-supplied sampler program (`--sample-tool`), absolute local disk path.
    /// `None` disables sampling entirely: no default, no `SystemRoot` derivation and no
    /// environment lookup exists, so nothing in the environment can select a sampler.
    pub sample_tool: Option<PathBuf>,
    /// Extra argv placed before the fixed sample argv (tests only).
    pub sample_argv_prefix: Vec<OsString>,
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
    /// Peak capture-file size observed while the child was supervised.
    pub scratch_max_bytes: Option<u64>,
    /// Sampling attempts made (bounded by `--max-samples`, successes or not).
    pub working_set_attempts: usize,
    /// Working-set samples that produced a value (0 when none were read).
    pub working_set_samples: usize,
    /// Largest sampled working set; `None` when sampling produced no value.
    pub working_set_max_bytes: Option<u64>,
    /// `sampled_lower_bound` when at least one sample was read, else `not_measured`.
    pub working_set_sampling: &'static str,
    /// Why the working set is `not_measured` (or `sample_capped`).
    pub working_set_reason: Option<&'static str>,
    /// Milliseconds from spawn to the kill request, when one was made.
    pub cancellation_requested_at_ms: Option<u64>,
    /// Milliseconds from the kill request to confirmed process exit.
    pub cancellation_observed_ms: Option<u64>,
    /// The candidate's own `cancelled_at_*` token, when it reported one.
    pub cancelled_at_reason: Option<&'static str>,
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
    /// Sampling interval used for this run (`0` = sampling disabled).
    pub sample_interval_ms: u64,
    /// Sample cap used for this run.
    pub max_samples: usize,
    /// Operator cancellation deadline used for this run, when one was requested.
    pub cancel_after_ms: Option<u64>,
    /// Resolved absolute path of the sampler program.
    pub sample_tool: Option<PathBuf>,
    /// SHA-256 of the sampler program (never fabricated).
    pub sample_tool_sha256: Option<String>,
    /// Why the sampler identity is unavailable, when it is.
    pub sample_tool_reason: Option<&'static str>,
    pub cases_truncated: bool,
    pub observations_truncated: bool,
    pub candidates: Vec<CandidateRecord>,
    pub observations: Vec<Observation>,
    pub capabilities: Vec<CapabilityRow>,
    pub resources: Vec<ResourceRow>,
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
    /// operator digest (NIT-3: a count on this row, where an observation row carries
    /// the boolean `pixel_verified`). Structure and pixel evidence are labelled
    /// separately, and a `pixel_mismatch` observation is `error` so it never reaches
    /// this row at all.
    pub pixels_verified: usize,
    /// Largest sampled working set over the contributing observations, when sampled.
    pub working_set_max_bytes: Option<u64>,
}

/// Observed resource facts for one candidate, over the same contributing observations
/// as the capability rows (decoded + complete + a verified or presence-only claim).
#[derive(Debug)]
pub struct ResourceRow {
    pub candidate_id: String,
    pub observations: usize,
    pub working_set_max_bytes: Option<u64>,
    pub working_set_sampling: &'static str,
    pub working_set_reason: Option<&'static str>,
    pub scratch_max_bytes: Option<u64>,
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
    /// A deadline-triggered kill happened (timeout or operator cancellation).
    cancelled: bool,
    wall_ms: u64,
    /// Observed peak of the two capture files (not a sampled lower bound).
    scratch_max_bytes: u64,
    capped: Option<CapKind>,
    /// Working-set sampling: a sampled lower bound, never a guess.
    working_set_attempts: usize,
    working_set_samples: usize,
    working_set_max_bytes: Option<u64>,
    working_set_sampling: &'static str,
    working_set_reason: Option<&'static str>,
    /// Cancellation timings, present only when a kill was requested.
    cancellation_requested_at_ms: Option<u64>,
    cancellation_observed_ms: Option<u64>,
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
    let mut sample_tool: Option<PathBuf> = None;
    let mut sample_interval_ms = DEFAULT_SAMPLE_INTERVAL_MS;
    let mut max_samples = MAX_SAMPLES_LIMIT;
    let mut cancel_after_ms = None;
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
            "--sample-tool" => {
                let value = take(&mut arguments, "--sample-tool")?;
                let path = PathBuf::from(value);
                let validated = sample_tool_from_cli(&path)
                    .map_err(|message| UsageError::new(message.as_str()))?;
                sample_tool = Some(validated);
            }
            "--sample-interval-ms" => {
                let value = take(&mut arguments, "--sample-interval-ms")?;
                let parsed = value
                    .to_string_lossy()
                    .parse::<u64>()
                    .map_err(|_| UsageError::new("--sample-interval-ms expects a number of ms"))?;
                if parsed > MAX_SAMPLE_INTERVAL_MS {
                    return Err(UsageError::new(format!(
                        "--sample-interval-ms is capped at {MAX_SAMPLE_INTERVAL_MS} ms"
                    )));
                }
                sample_interval_ms = parsed;
            }
            "--max-samples" => {
                let value = take(&mut arguments, "--max-samples")?;
                let parsed = value
                    .to_string_lossy()
                    .parse::<usize>()
                    .map_err(|_| UsageError::new("--max-samples expects a count"))?;
                if parsed == 0 || parsed > MAX_SAMPLES_LIMIT {
                    return Err(UsageError::new(format!(
                        "--max-samples must be 1..={MAX_SAMPLES_LIMIT}"
                    )));
                }
                max_samples = parsed;
            }
            "--cancel-after-ms" => {
                let value = take(&mut arguments, "--cancel-after-ms")?;
                let parsed = value
                    .to_string_lossy()
                    .parse::<u64>()
                    .map_err(|_| UsageError::new("--cancel-after-ms expects a number of ms"))?;
                cancel_after_ms = Some(parsed);
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
                     --timeout-ms, --cancel-after-ms, --max-output-bytes, \
                     --max-output-file-bytes, --max-cases, --max-observations, --max-pixels, \
                     --sample-tool, --sample-interval-ms, --max-samples"
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
        sample_interval_ms,
        max_samples,
        cancel_after_ms,
        // Opt-in only: the operator names the sampler with `--sample-tool`, and no
        // environment value can supply or influence it.
        sample_tool,
        sample_argv_prefix: Vec::new(),
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
    // Sampling is opt-in: without `--sample-tool` nothing is resolved, nothing is
    // spawned, and every observation reports `sampling_disabled`.
    let sample_tool_source = config.sample_tool.clone();
    let fingerprint = match sample_tool_source.as_ref() {
        Some(path) => read_sample_tool(path).map(|bytes| pixels::hex(&pixels::sha256(&bytes))),
        None => Err(SAMPLE_DISABLED),
    };
    let (sample_tool_sha256, sample_tool_reason) = match fingerprint {
        Ok(digest) => (Some(digest), None),
        Err(reason) => (None, Some(reason)),
    };
    // The digest always describes the file that will be executed; it is re-verified
    // around every spawn and a mismatch discards the sample.
    let resolved_sample_tool = sample_tool_sha256
        .as_ref()
        .and_then(|_| sample_tool_source.clone());
    let sample_tool_failure = sample_tool_reason;
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
            let paths = RunPaths {
                work,
                captures,
                sample_tool: resolved_sample_tool.as_deref(),
                sample_digest: sample_tool_sha256.as_deref(),
                sample_tool_failure,
            };
            observations.push(measure(
                config, candidate, launcher, case, &paths, case_index,
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
        sample_interval_ms: config.sample_interval_ms,
        max_samples: config.max_samples,
        cancel_after_ms: config.cancel_after_ms,
        // The source path is recorded, never the private staged copy.
        sample_tool: sample_tool_source.clone(),
        sample_tool_sha256: sample_tool_sha256.clone(),
        sample_tool_reason,
        cases_truncated,
        observations_truncated,
        candidates: identities,
        observations,
        capabilities: Vec::new(),
        resources: Vec::new(),
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
    report.resources = resource_rows(&report.observations, &report.candidates);
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
                None,
                Some(probe_bounds(&capture)),
                None,
                SAMPLE_DISABLED,
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

/// Run-scoped paths every observation needs.
struct RunPaths<'a> {
    work: &'a Path,
    captures: &'a Path,
    /// Absolute sampler program, or None when it could not be resolved.
    sample_tool: Option<&'a Path>,
    /// Digest of the operator-supplied tool, verified around every spawn.
    sample_digest: Option<&'a str>,
    /// Why the sampler is unusable, when it is (`sampling_disabled` for opt-out).
    sample_tool_failure: Option<&'static str>,
}

fn measure(
    config: &Config,
    candidate: &Candidate,
    launcher: &Launcher,
    case: &Case,
    paths: &RunPaths<'_>,
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
        scratch_max_bytes: None,
        working_set_attempts: 0,
        working_set_samples: 0,
        working_set_max_bytes: None,
        working_set_sampling: NOT_MEASURED,
        working_set_reason: None,
        cancellation_requested_at_ms: None,
        cancellation_observed_ms: None,
        cancelled_at_reason: None,
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
        paths.work,
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
    let Some((capture, stdout_stdio, stderr_stdio)) = CaptureFiles::open(paths.captures, &stem)
    else {
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
            // Sampling needs both an operator-enabled interval and an absolute,
            // resolved sampler program; without the tool nothing is spawned.
            let sampling = (config.sample_interval_ms > 0)
                .then_some(paths.sample_tool)
                .flatten()
                .map(|tool| Sampling {
                    captures: paths.captures,
                    stem: format!("{stem}.ws"),
                    interval: Duration::from_millis(config.sample_interval_ms),
                    max_samples: config.max_samples,
                    tool: tool.to_path_buf(),
                    argv_prefix: config.sample_argv_prefix.clone(),
                    digest: paths.sample_digest.unwrap_or_default().to_owned(),
                });
            // The observation-level reason is the resolved token, not a fixed one:
            // opt-out is `sampling_disabled`, an unreadable or oversized operator tool
            // keeps its own token, and a rejected one is `sample_tool_missing`.
            let sampling_disabled_reason = if config.sample_interval_ms == 0 {
                SAMPLE_DISABLED
            } else if paths.sample_tool.is_none() {
                paths.sample_tool_failure.unwrap_or(SAMPLE_DISABLED)
            } else {
                SAMPLE_DISABLED
            };
            let supervised = supervise(
                child,
                Duration::from_millis(config.timeout_ms),
                config.cancel_after_ms.map(Duration::from_millis),
                Some(bounds),
                sampling.as_ref(),
                sampling_disabled_reason,
            );
            let (stdout, stderr) = capture.read(config.max_output_bytes);
            let (output_bytes, output_present) = match fs::metadata(&output) {
                Ok(metadata) if metadata.is_file() => (Some(metadata.len()), metadata.len() > 0),
                _ => (None, false),
            };
            observation.wall_ms = Some(supervised.wall_ms);
            observation.scratch_max_bytes = Some(supervised.scratch_max_bytes);
            observation.working_set_attempts = supervised.working_set_attempts;
            observation.working_set_samples = supervised.working_set_samples;
            observation.working_set_max_bytes = supervised.working_set_max_bytes;
            observation.working_set_sampling = supervised.working_set_sampling;
            observation.working_set_reason = supervised.working_set_reason;
            observation.cancellation_requested_at_ms = supervised.cancellation_requested_at_ms;
            observation.cancellation_observed_ms = supervised.cancellation_observed_ms;
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
                None if supervised.cancelled => OutcomeClass::Cancelled,
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
                    // Structurally not a usable container: the pixel verdict cannot be
                    // claimed either, and the reason must match the refusal that
                    // `verify` will produce (ANB-2): no dimensions at all means the
                    // output is unparsable, dimensions without a terminator means it is
                    // an incomplete container.
                    observation.pixel_status = PixelStatus::Unavailable;
                    observation.pixel_reason = Some(if inspection.dimensions().is_none() {
                        OUTPUT_UNPARSABLE
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
            // NIT-6: the candidate's own cancellation claim is only recorded when the
            // observed class really is `cancelled`.
            if observation.observed_outcome == OutcomeClass::Cancelled {
                observation.cancelled_at_reason = observation
                    .candidate_reason
                    .starts_with("cancelled_at_")
                    .then_some(observation.candidate_reason);
            }
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
/// A1/B2: only a decoded observation with a complete container and a presence or
/// verified validity contributes capability or resource evidence; refusals,
/// mismatches, cancellations and `not_measured` rows never do.
fn contributes_evidence(observation: &Observation) -> bool {
    observation.observed_outcome == OutcomeClass::Decoded
        && observation.inspected_complete
        && matches!(
            observation.validity,
            PRESENCE_ONLY | STRUCTURE_VERIFIED | PIXELS_VERIFIED
        )
}

fn capability_rows(observations: &[Observation]) -> Vec<CapabilityRow> {
    let mut rows: BTreeMap<(String, Format), CapabilityRow> = BTreeMap::new();
    for observation in observations {
        if !contributes_evidence(observation) {
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
                pixels_verified: 0,
                working_set_max_bytes: None,
            });
        row.observations += 1;
        if observation.validity == PIXELS_VERIFIED {
            row.pixels_verified += 1;
        }
        if let Some(bytes) = observation.working_set_max_bytes {
            row.working_set_max_bytes = Some(
                row.working_set_max_bytes
                    .map_or(bytes, |current| current.max(bytes)),
            );
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

/// Aggregate the same contributing observations into one resource row per candidate.
///
/// The exclusion rule is identical to the capability table: only observations that are
/// `decoded`, complete and carry a `presence_only` / `structure_verified` /
/// `pixels_verified` claim contribute, so a cancelled, refused or mismatching run never
/// inflates a resource figure. A candidate without contributing observations keeps the
/// row with explicit `not_measured` fields.
fn resource_rows(observations: &[Observation], candidates: &[CandidateRecord]) -> Vec<ResourceRow> {
    let mut rows: Vec<ResourceRow> = candidates
        .iter()
        .map(|candidate| ResourceRow {
            candidate_id: candidate.id.clone(),
            observations: 0,
            working_set_max_bytes: None,
            working_set_sampling: NOT_MEASURED,
            working_set_reason: Some(RESOURCE_NO_OBSERVATION),
            scratch_max_bytes: None,
        })
        .collect();
    for observation in observations {
        if !contributes_evidence(observation) {
            continue;
        }
        let Some(row) = rows
            .iter_mut()
            .find(|row| row.candidate_id == observation.candidate_id)
        else {
            continue;
        };
        row.observations += 1;
        row.working_set_reason = None;
        if let Some(bytes) = observation.working_set_max_bytes {
            row.working_set_max_bytes = Some(
                row.working_set_max_bytes
                    .map_or(bytes, |current| current.max(bytes)),
            );
            row.working_set_sampling = SAMPLED_LOWER_BOUND;
        } else if row.working_set_sampling != SAMPLED_LOWER_BOUND {
            row.working_set_sampling = NOT_MEASURED;
            row.working_set_reason = observation.working_set_reason;
        }
        if let Some(bytes) = observation.scratch_max_bytes {
            row.scratch_max_bytes = Some(
                row.scratch_max_bytes
                    .map_or(bytes, |current| current.max(bytes)),
            );
        }
    }
    rows.sort_by(|left, right| left.candidate_id.cmp(&right.candidate_id));
    rows
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
/// Why the sampler program could not be hashed or pinned.
const SAMPLE_TOOL_UNREADABLE: &str = "sample_tool_unreadable";
/// The sampler program changed between pinning and execution (digest mismatch).
const SAMPLE_TOOL_CHANGED: &str = "sample_tool_changed";
/// The sampler program was larger than the bounded read.
const SAMPLE_TOOL_OVERSIZE: &str = "sample_tool_oversize";

/// The operator's `--sample-tool` value, validated as a local disk path.
///
/// Sampling is opt-in: there is no default, no `SystemRoot` derivation and no
/// environment lookup, so no redirect of the environment or of the system directory can
/// select the sampler at all. The value is used exactly as given.
pub fn sample_tool_from_cli(value: &Path) -> Result<PathBuf, String> {
    if !is_local_disk_absolute(value) {
        return Err(format!(
            "--sample-tool must be an absolute local disk path (no UNC, verbatim or device form, \
             no parent component), not {}",
            value.display()
        ));
    }
    Ok(value.to_path_buf())
}

/// The rule the harness applies to candidate paths (see `local_absolute_path`):
/// absolute, a plain local disk prefix, rooted, and no parent or current-directory
/// component. UNC, verbatim and device forms are not a local disk prefix.
fn is_local_disk_absolute(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let mut components = path.components();
        match components.next() {
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)) => {}
            _ => return false,
        }
        if components.next() != Some(Component::RootDir) {
            return false;
        }
        components.all(|component| matches!(component, Component::Normal(_)))
    }
    #[cfg(not(windows))]
    {
        use std::path::Component;
        path.components().all(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        })
    }
}

/// Read the sampler program with a hard size cap; the bytes are hashed and, in the
/// normal path, executed, so a read can never run away.
fn read_sample_tool(path: &Path) -> Result<Vec<u8>, &'static str> {
    let file = fs::File::open(path).map_err(|_| SAMPLE_TOOL_UNREADABLE)?;
    let mut bytes = Vec::new();
    let mut limited = file.take(SAMPLE_TOOL_DIGEST_CAP.saturating_add(1));
    limited
        .read_to_end(&mut bytes)
        .map_err(|_| SAMPLE_TOOL_UNREADABLE)?;
    if bytes.len() as u64 > SAMPLE_TOOL_DIGEST_CAP {
        return Err(SAMPLE_TOOL_OVERSIZE);
    }
    Ok(bytes)
}

/// Test seam: read a path with the sampler's 32 MiB cap.
#[cfg(test)]
pub fn read_sample_tool_for_tests(path: &Path) -> Result<Vec<u8>, &'static str> {
    read_sample_tool(path)
}

/// Whether the program at `path` still matches the recorded digest; a read that fails or
/// a different digest means the file is not the one that was sampled.
fn sample_tool_matches(path: &Path, expected: &str) -> bool {
    read_sample_tool(path)
        .map(|bytes| pixels::hex(&pixels::sha256(&bytes)) == expected)
        .unwrap_or(false)
}

/// Sampling configuration for one observation.
struct Sampling<'a> {
    captures: &'a Path,
    stem: String,
    interval: Duration,
    max_samples: usize,
    /// Sampler program: `tasklist` unless a test injects one (A4).
    tool: PathBuf,
    /// Extra argv placed before the fixed sample argv (test seam, like the candidate
    /// `Invocation::argv_prefix`).
    argv_prefix: Vec<OsString>,
    /// Digest of the bytes pinned for this run; every spawn is verified against it.
    digest: String,
}

/// Working-set sampling bookkeeping, kept separate from the I/O so every transition is
/// unit-testable (A3).
///
/// Invariants: `max_bytes` is `Some` exactly when at least one sample succeeded;
/// `sampling` is `sampled_lower_bound` exactly then, and a failure reason is never kept
/// next to a value (a later success clears it). `sample_capped` is the only reason that
/// may accompany a value, because it describes the cap, not a failure.
pub struct Sampler {
    enabled: bool,
    pub attempts: usize,
    max_samples: usize,
    pub samples: usize,
    pub max_bytes: Option<u64>,
    pub sampling: &'static str,
    pub reason: Option<&'static str>,
    stopped: bool,
}

impl Sampler {
    /// A sampler that never runs, with the documented reason why (`sampling_disabled`
    /// when the operator turned sampling off, `sample_tool_missing` when the sampler
    /// program could not be resolved).
    fn disabled(reason: &'static str) -> Self {
        let mut sampler = Self::new(false, 0);
        sampler.reason = Some(reason);
        sampler
    }

    fn new(enabled: bool, max_samples: usize) -> Self {
        Self {
            enabled,
            attempts: 0,
            max_samples,
            samples: 0,
            max_bytes: None,
            sampling: NOT_MEASURED,
            reason: if enabled { None } else { Some(SAMPLE_DISABLED) },
            stopped: !enabled,
        }
    }

    /// Test seam: an enabled sampler with the given attempt cap.
    #[cfg(test)]
    pub fn for_tests(max_samples: usize) -> Self {
        Self::new(true, max_samples)
    }

    /// Whether another attempt may start now.
    pub fn may_attempt(&self) -> bool {
        self.enabled && !self.stopped && self.attempts < self.max_samples
    }

    /// The cap stopped scheduling further attempts.
    pub fn capped(&mut self) {
        self.stopped = true;
        if self.reason.is_none() {
            self.reason = Some(SAMPLE_CAPPED);
        }
    }

    pub fn attempt(&mut self) {
        self.attempts += 1;
    }

    /// A sample produced a working-set value.
    pub fn succeeded(&mut self, bytes: u64) {
        self.samples += 1;
        self.max_bytes = Some(self.max_bytes.map_or(bytes, |current| current.max(bytes)));
        self.sampling = SAMPLED_LOWER_BOUND;
        // A success clears any earlier failure reason (A3).
        self.reason = None;
    }

    /// The sampler program changed: every value already taken is no longer attributable
    /// to the recorded digest, so it is discarded and the reason is recorded even when a
    /// value existed (NIT N1 — never cleared silently).
    pub fn discarded(&mut self, reason: &'static str) {
        self.stopped = true;
        self.samples = 0;
        self.max_bytes = None;
        self.sampling = NOT_MEASURED;
        self.reason = Some(reason);
    }

    /// A sample could not be read: record why and stop retrying.
    pub fn failed(&mut self, reason: &'static str) {
        self.stopped = true;
        if self.samples == 0 {
            self.sampling = NOT_MEASURED;
            self.reason = Some(reason);
        } else {
            // Someone else already produced a value; keep the value and its label.
            self.reason = None;
        }
    }

    /// Settle the reason for a child that ended without producing anything.
    pub fn finish(&mut self) {
        if self.enabled && self.samples == 0 && self.reason.is_none() {
            self.reason = Some(SAMPLE_NO_SAMPLE);
        }
    }
}

/// Supervise one child: enforce the deadline, watch the caps, sample its working set
/// and record the cancellation timings.
///
/// The deadline is the smaller of `timeout` and `cancel_after` (when the operator asked
/// for an earlier cancellation). The working set is sampled through
/// [`sample_working_set`] at most `max_samples` times, and only while enough time
/// remains before the deadline, so a sample can never push the hard deadline out. Every
/// recorded value is a sampled lower bound: a sample that cannot be read leaves the
/// field absent with a reason token instead of guessing.
fn supervise(
    mut child: Child,
    timeout: Duration,
    cancel_after: Option<Duration>,
    bounds: Option<Bounds<'_>>,
    sampling: Option<&Sampling<'_>>,
    sampling_disabled_reason: &'static str,
) -> Supervised {
    let started = Instant::now();
    let pid = child.id();
    let mut timeout_fired = false;
    let mut cancelled = false;
    let mut killed = false;
    let mut capped = None;
    let mut scratch_max_bytes = 0_u64;
    let mut cancellation_requested_at_ms = None;
    let mut cancellation_observed_ms = None;
    let mut sampler = match sampling {
        Some(config) => Sampler::new(true, config.max_samples),
        None => Sampler::disabled(sampling_disabled_reason),
    };
    let deadline = match cancel_after {
        Some(early) => early.min(timeout),
        None => timeout,
    };
    let mut next_sample = sampling.map(|config| config.interval);

    let mut in_flight: Option<SampleInFlight> = None;
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
        if let Some(bounds) = bounds.as_ref() {
            let scratch = file_len(bounds.first) + file_len(bounds.second);
            scratch_max_bytes = scratch_max_bytes.max(scratch);
        }
        // Harvest a sample that has finished (or timed out) without blocking the loop.
        if let Some(mut flight) = in_flight.take() {
            match flight.child.try_wait() {
                Ok(Some(_)) => {
                    // The bytes must still be the verified ones after the child ran; a
                    // changed program means the value is discarded and the capture is
                    // released like every other completed sample (NIT N2).
                    if !sample_tool_matches(&flight.tool, &flight.digest) {
                        flight.capture.release();
                        sampler.discarded(SAMPLE_TOOL_CHANGED);
                        next_sample = None;
                    } else {
                        match harvest_sample(&mut flight) {
                            Ok(bytes) => sampler.succeeded(bytes),
                            Err(reason) => {
                                sampler.failed(reason);
                                next_sample = None;
                            }
                        }
                    }
                }
                Err(_) => {
                    // A sampler we cannot reap must not be left running (N5).
                    let _ = flight.child.kill();
                    let _ = flight.child.wait();
                    flight.capture.release();
                    sampler.failed(SAMPLE_PARSE_FAILED);
                    next_sample = None;
                }
                Ok(None) => {
                    if flight.started.elapsed() >= SAMPLE_TIMEOUT {
                        // N5: kill, reap and release the timed-out sampler.
                        let _ = flight.child.kill();
                        let _ = flight.child.wait();
                        flight.capture.release();
                        sampler.failed(SAMPLE_TIMEOUT_REASON);
                        next_sample = None;
                    } else {
                        in_flight = Some(flight);
                    }
                }
            }
        }
        let elapsed = started.elapsed();
        if let (Some(config), Some(due)) = (sampling, next_sample)
            && elapsed >= due
            && in_flight.is_none()
        {
            if !sampler.may_attempt() {
                // The attempt cap actually stopped sampling (A2).
                sampler.capped();
                next_sample = None;
            } else {
                // A2: never spawn a program whose bytes differ from the pinned digest.
                if !sample_tool_matches(&config.tool, &config.digest) {
                    sampler.discarded(SAMPLE_TOOL_CHANGED);
                    next_sample = None;
                } else {
                    sampler.attempt();
                    match start_sample(pid, config) {
                        Ok(flight) => {
                            in_flight = Some(flight);
                            next_sample = Some(elapsed.saturating_add(config.interval));
                        }
                        Err(reason) => {
                            sampler.failed(reason);
                            next_sample = None;
                        }
                    }
                }
            }
        }
        if let Some(bounds) = bounds.as_ref()
            && let Some(kind) = exceeded(bounds)
        {
            let (requested, observed) = cancel(&mut child, started);
            cancellation_requested_at_ms = requested;
            cancellation_observed_ms = observed;
            killed = true;
            capped = Some(kind);
            break None;
        }
        let remaining = deadline.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            let (requested, observed) = cancel(&mut child, started);
            cancellation_requested_at_ms = requested;
            cancellation_observed_ms = observed;
            killed = true;
            cancelled = true;
            timeout_fired = cancel_after.is_none_or(|early| early >= timeout);
            break None;
        }
        thread::sleep(remaining.min(POLL_INTERVAL));
    };
    // A sample still in flight when the child ends is killed, reaped and released,
    // and it is never counted (N5).
    if let Some(mut flight) = in_flight {
        let _ = flight.child.kill();
        let _ = flight.child.wait();
        flight.capture.release();
    }
    // Peak after exit: an exited child may have written more than one poll observed.
    if let Some(bounds) = bounds.as_ref() {
        let scratch = file_len(bounds.first) + file_len(bounds.second);
        scratch_max_bytes = scratch_max_bytes.max(scratch);
    }
    let output_bytes = bounds
        .as_ref()
        .and_then(|bounds| bounds.output)
        .map_or(0, file_len);
    // B-03: re-check after exit, because an exited child may have written more
    // than a single poll could observe.
    if let Some(bounds) = bounds.as_ref()
        && capped.is_none()
        && !cancelled
    {
        if scratch_max_bytes > bounds.scratch_limit {
            capped = Some(CapKind::Scratch);
        } else if output_bytes > bounds.output_limit && bounds.output.is_some() {
            capped = Some(CapKind::Output);
        }
    }
    sampler.finish();
    Supervised {
        exit_code: if killed {
            None
        } else {
            status.and_then(|status| status.code())
        },
        timeout_fired,
        cancelled,
        wall_ms: elapsed_millis(started.elapsed()),
        scratch_max_bytes,
        capped,
        working_set_attempts: sampler.attempts,
        working_set_samples: sampler.samples,
        working_set_max_bytes: sampler.max_bytes,
        working_set_sampling: sampler.sampling,
        working_set_reason: sampler.reason,
        cancellation_requested_at_ms,
        cancellation_observed_ms,
    }
}

/// Kill the child and time the round trip: request to confirmed exit.
fn cancel(child: &mut Child, started: Instant) -> (Option<u64>, Option<u64>) {
    let requested = started.elapsed();
    let _ = child.kill();
    let _ = child.wait();
    let observed = started.elapsed().saturating_sub(requested);
    (
        Some(elapsed_millis(requested)),
        Some(elapsed_millis(observed)),
    )
}

/// A sampled `tasklist` call in flight: started by the supervisor and harvested on a
/// later poll, so sampling never blocks the deadline loop.
struct SampleInFlight {
    child: Child,
    capture: CaptureFiles,
    started: Instant,
    /// The supervised child this sample belongs to; the parsed row must match it.
    pid: u32,
    /// Path and pinned digest re-verified after the sampler exits.
    tool: PathBuf,
    digest: String,
}

/// Start one bounded `tasklist` sample with a fixed argv (no shell, no quoting).
///
/// `tasklist` needs a few hundred milliseconds on Windows, so the call runs
/// concurrently with the supervised child: the parent only spawns it here and harvests
/// it from a later poll. That keeps the hard deadline independent of sampling cost.
fn start_sample(pid: u32, config: &Sampling<'_>) -> Result<SampleInFlight, &'static str> {
    let Some((capture, stdout, stderr)) = CaptureFiles::open(config.captures, &config.stem) else {
        return Err(SAMPLE_PARSE_FAILED);
    };
    let filter = format!("PID eq {pid}");
    let mut command = Command::new(&config.tool);
    command.args(&config.argv_prefix);
    command
        .args(["/FI", filter.as_str(), "/FO", "CSV", "/NH"])
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    let Ok(child) = command.spawn() else {
        capture.release();
        return Err(SAMPLE_TOOL_MISSING);
    };
    Ok(SampleInFlight {
        child,
        capture,
        started: Instant::now(),
        pid,
        tool: config.tool.clone(),
        digest: config.digest.clone(),
    })
}

/// Harvest a finished sample: read its bounded stdout, parse the working set and
/// release the capture files. A failure is a token, never a guessed number.
fn harvest_sample(flight: &mut SampleInFlight) -> Result<u64, &'static str> {
    let pid = flight.pid;
    let capture = flight
        .capture
        .read(usize::try_from(SAMPLE_OUTPUT_BYTES).unwrap_or(usize::MAX));
    let bytes = capture.0.bytes;
    flight.capture.release();
    if bytes.is_empty() {
        return Err(SAMPLE_PARSE_FAILED);
    }
    let Ok(text) = String::from_utf8(bytes) else {
        // An OEM-encoded image name is not UTF-8; the row is unusable either way.
        return Err(SAMPLE_PARSE_FAILED);
    };
    let row = parse_working_set(&text, pid).ok_or(SAMPLE_PARSE_FAILED)?;
    if row.pid != pid {
        return Err(SAMPLE_PARSE_FAILED);
    }
    Ok(row.bytes)
}

/// One parsed working-set sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SampleRow {
    pid: u32,
    bytes: u64,
}

/// Parse a `tasklist /FO CSV /NH` output into the row of `pid`.
///
/// Only the documented Windows layout is accepted — exactly five quoted fields
/// (`Image Name`, `PID`, `Session Name`, `Session#`, `Mem Usage`), a PID column equal to
/// the supervised child and a memory column of the form `12,000 K` — so a fabricated,
/// localized, relocated or truncated row can never be read as a byte count (A1). A row
/// that does not validate is skipped; no row at all means `None`.
fn parse_working_set(text: &str, pid: u32) -> Option<SampleRow> {
    text.lines()
        .filter_map(|line| parse_sample_row(line, pid))
        .next()
}

fn parse_sample_row(line: &str, pid: u32) -> Option<SampleRow> {
    let fields = csv_fields(line);
    // Exactly the five columns `tasklist /FO CSV /NH` emits; anything else (a header
    // row, a fabricated extra column, a different locale) is not our row.
    if fields.len() != 5 {
        return None;
    }
    if fields[0].is_empty() || fields[2].is_empty() {
        return None;
    }
    let row_pid = fields[1].trim().parse::<u32>().ok()?;
    if row_pid != pid {
        return None;
    }
    if fields[3].trim().parse::<u32>().is_err() {
        return None;
    }
    let bytes = parse_memory_field(fields[4])?;
    Some(SampleRow {
        pid: row_pid,
        bytes,
    })
}

/// Test seam for the row parser: the byte count of the first valid row for `pid`.
#[cfg(test)]
pub fn parse_working_set_for_tests(text: &str, pid: u32) -> Option<u64> {
    parse_working_set(text, pid).map(|row| row.bytes)
}

/// Parse the `Mem Usage` column: `12,000 K`, `6,408 K` or a plain `12000 K`, in
/// kilobytes. Anything else (a session number, a localized unit, an empty field) is
/// refused.
fn parse_memory_field(field: &str) -> Option<u64> {
    let number = field.trim().strip_suffix(" K")?;
    if !number
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b',')
    {
        return None;
    }
    let kilobytes = if number.contains(',') {
        // Grouped form: `12,000`, i.e. 1-3 digits then groups of exactly three.
        let mut groups = number.split(',');
        let first = groups.next()?;
        if first.is_empty() || first.len() > 3 || !first.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let mut value = first.parse::<u64>().ok()?;
        for group in groups {
            if group.len() != 3 || !group.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            value = value
                .checked_mul(1000)?
                .checked_add(group.parse::<u64>().ok()?)?;
        }
        value
    } else {
        // Plain form, also documented: `12000 K`.
        if number.is_empty() || number.len() > 9 {
            return None;
        }
        number.parse::<u64>().ok()?
    };
    Some(kilobytes.saturating_mul(1024))
}

/// Split one CSV line into its quoted fields (`tasklist` always quotes).
fn csv_fields(line: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let mut start = None;
    for (index, character) in line.char_indices() {
        if character == '"' {
            match start.take() {
                Some(opened) => fields.push(&line[opened..index]),
                None => start = Some(index + 1),
            }
        }
    }
    fields
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
    let _ = writeln!(
        json,
        "  \"sample_interval_ms\": {},",
        report.sample_interval_ms
    );
    let _ = writeln!(json, "  \"max_samples\": {},", report.max_samples);
    let _ = writeln!(
        json,
        "  \"cancel_after_ms\": {},",
        number_field(report.cancel_after_ms)
    );
    let _ = writeln!(
        json,
        "  \"sample_tool\": {},",
        lossy_path_field(report.sample_tool.as_deref())
    );
    let _ = writeln!(
        json,
        "  \"sample_tool_sha256\": {},",
        text_field(report.sample_tool_sha256.as_deref())
    );
    let _ = writeln!(
        json,
        "  \"sample_tool_reason\": {},",
        text_field(report.sample_tool_reason)
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
    let resources: Vec<String> = report.resources.iter().map(resource_row).collect();
    let _ = writeln!(json, "  \"resources\": {},", json_array(&resources));
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
         \"stderr_truncated\": {}, \"wall_ms\": {}, \"scratch_max_bytes\": {}, \
         \"working_set_attempts\": {}, \"working_set_samples\": {}, \"working_set_max_bytes\": {}, \
         \"working_set_sampling\": {}, \"working_set_reason\": {}, \
         \"cancellation_requested_at_ms\": {}, \"cancellation_observed_ms\": {}, \
         \"cancelled_at_reason\": {}, \
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
        number_field(observation.scratch_max_bytes),
        observation.working_set_attempts,
        observation.working_set_samples,
        number_field(observation.working_set_max_bytes),
        quoted(observation.working_set_sampling),
        text_field(observation.working_set_reason),
        number_field(observation.cancellation_requested_at_ms),
        number_field(observation.cancellation_observed_ms),
        text_field(observation.cancelled_at_reason),
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

fn resource_row(row: &ResourceRow) -> String {
    format!(
        "    {{\"candidate_id\": {}, \"observations\": {}, \"working_set_max_bytes\": {}, \
         \"working_set_sampling\": {}, \"working_set_reason\": {}, \"scratch_max_bytes\": {}}}",
        quoted(&row.candidate_id),
        row.observations,
        number_field(row.working_set_max_bytes),
        quoted(row.working_set_sampling),
        text_field(row.working_set_reason),
        number_field(row.scratch_max_bytes),
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
         \"colour_markers\": {}, \"pixels_verified\": {}, \"working_set_max_bytes\": {}, \
         \"not_measured\": {}, \"not_measured_reason\": {}, \
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
        row.pixels_verified,
        number_field(row.working_set_max_bytes),
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

/// Render a path even when it is not valid UTF-8: the digest beside it stays exact, so
/// a lossy display is still useful evidence and better than a bare `null` (N5).
#[cfg(test)]
pub fn lossy_path_field_for_tests(path: Option<&Path>) -> String {
    lossy_path_field(path)
}

fn lossy_path_field(path: Option<&Path>) -> String {
    match path {
        Some(path) => quoted(&path.to_string_lossy()),
        None => "null".to_owned(),
    }
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
