//! Exploratory, decoder-neutral MP-IMG2 Phase 1 comparison harness (record schema v3).
//!
//! This module is shared by the operator-run `decode_compare` example and its
//! contract tests. It is not a product decoder, selects no decoder, links no
//! FFmpeg and makes no numeric claim: the emitted record is explicitly
//! exploratory and every value it contains was actually observed.
//!
//! Candidate process protocol (v3), fixed so that no argv element or decoder path
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
//!   `<stem>.wic.png`, so its encoder format never depends on the input suffix.
//! - environment (contract tests only): the case paths and the probe form travel
//!   in [`ENV_INPUT`], [`ENV_OUTPUT`] and [`ENV_PROBE`], because a libtest helper
//!   process's argv is not free.
//!
//! Candidate reasons: the first [`MAX_REASON_CHARS`]-bounded `"reason_code":"..."`
//! token found in the captured stdout is mapped through [`KNOWN_REASONS`]. A token
//! outside that closed vocabulary, a malformed line or no token at all yields
//! `not_measured`; foreign text is never copied into the record.
//!
//! Outcome mapping: exit code `0` decoded, `2` unsupported, `3` corrupt, `4`
//! oversize, `5` cancelled; any other code is `error`, a process that cannot be
//! started is `not_run`, a process without an observed exit code is
//! `not_measured`, and a hard-timeout or scratch-cap kill is `cancelled`. Exit
//! codes are the operating-system code as a **signed 32-bit value**
//! (Rust `ExitStatus::code()`), so e.g. FFmpeg's `0xABAAAA08` appears as
//! `-1414549496`.
//!
//! Safety: only the operator-nominated program is launched (never a shell), every
//! child gets the same bounded argv, stdin is closed, stdout/stderr are redirected
//! to per-observation scratch files that the parent reads only after the child is
//! reaped (at most `max_output_bytes` each, never re-emitted as text except the
//! bounded identity line), and each child has a hard deadline. Because capture
//! never depends on a pipe, a descendant that inherits the child's handles cannot
//! keep the harness waiting. Scratch files live under
//! `<out parent>/decode_compare_outputs/captures`: the whole work directory is
//! deleted and recreated empty at the start of every run ([`output_directories`]),
//! each observation's capture files are removed once read, and during the poll
//! loop the combined capture size is checked against `max_output_bytes * 8`
//! (probe: [`PROBE_CAP_BYTES`]` * 8`). Exceeding that bound kills and reaps the
//! child and records `cancelled` with refusal `scratch_capped` plus the observed
//! `scratch_bytes`, so a flooding candidate cannot fill the disk unchecked.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt::Write as _,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Fixed record schema version; changing record semantics requires a new version.
pub const SCHEMA_VERSION: u32 = 3;
/// Exploratory comparisons are not a ratified numeric gate.
pub const NUMERIC_GATE: &str = "NOT_RATIFIED";
/// Optional operator declaration of the expected outcome per case file name.
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

const USAGE: &str = "usage: decode_compare --cases DIR --candidate ID=ABSOLUTE_EXE|\
ID=pwsh:ABSOLUTE_PS1 [--candidate ...]... --out FILE [--timeout-ms N] \
[--max-output-bytes N] [--max-cases N] [--max-observations N] [--max-pixels N]";
const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 8 * 1024;
const DEFAULT_MAX_CASES: usize = 64;
const DEFAULT_MAX_OBSERVATIONS: usize = 256;
const MAX_ID_BYTES: usize = 64;
const MAX_CASE_NAME_BYTES: usize = 256;
const MAX_IDENTITY_CHARS: usize = 256;
const MAX_REASON_CHARS: usize = 32;
const MAX_MANIFEST_LINES: usize = 1024;
const MAX_DIR_ENTRIES: usize = 4096;
const MAX_PATH_ENTRIES: usize = 64;
const OUTPUT_NAME_CHARS: usize = 48;
const PROBE_CAP_BYTES: usize = 4 * 1024;
const PWSH_PROGRAM: &str = "pwsh";
const INPUT_FLAG: &str = "-i";
const NOT_MEASURED: &str = "not_measured";
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

/// One validated comparison run configuration.
#[derive(Debug)]
pub struct Config {
    pub cases: PathBuf,
    pub out: PathBuf,
    pub candidates: Vec<Candidate>,
    pub timeout_ms: u64,
    pub max_output_bytes: usize,
    pub max_cases: usize,
    pub max_observations: usize,
    /// Hard per-record byte budget; [`MAX_RECORD_BYTES`] unless a test lowers it.
    pub max_record_bytes: usize,
    /// Optional operator pixel bound forwarded to the script transport.
    pub max_pixels: Option<u64>,
    pub invocation: Invocation,
    pub declared: BTreeMap<String, OutcomeClass>,
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
    pub output_bytes: Option<u64>,
    /// Stable refusal reason when the case was not measured, else `None`.
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
    pub max_cases: usize,
    pub max_observations: usize,
    pub max_record_bytes: usize,
    pub max_pixels: Option<u64>,
    pub cases_truncated: bool,
    pub observations_truncated: bool,
    pub candidates: Vec<CandidateRecord>,
    pub observations: Vec<Observation>,
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
    scratch_capped: bool,
}

#[derive(Default)]
struct ProbedIdentity {
    identity: Option<String>,
    probe: Option<&'static str>,
    exit_codes: [Option<i32>; 2],
    refusal: Option<&'static str>,
}

/// Combined scratch size a child may reach before it is killed.
struct ScratchBound<'a> {
    first: &'a Path,
    second: &'a Path,
    limit: u64,
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
                     --timeout-ms, --max-output-bytes, --max-cases, --max-observations, \
                     --max-pixels"
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
    let declared = load_manifest(&cases)?;
    Ok(Config {
        cases,
        out,
        candidates,
        timeout_ms,
        max_output_bytes,
        max_cases,
        max_observations,
        max_record_bytes: MAX_RECORD_BYTES,
        max_pixels,
        invocation: Invocation {
            argv_prefix: Vec::new(),
            env_transport: false,
        },
        declared,
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
/// manifest is a refusal. The file is opened and read with `take(MAX + 1)` so the
/// declared cap bounds allocation and one extra byte proves the cap was exceeded.
fn load_manifest(cases: &Path) -> Result<BTreeMap<String, OutcomeClass>, UsageError> {
    let mut declared = BTreeMap::new();
    let manifest = cases.join(MANIFEST_FILE);
    let file = match fs::File::open(&manifest) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(declared),
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
        let (name, token) = line.split_once('\t').ok_or_else(|| {
            UsageError::new(format!(
                "{MANIFEST_FILE} line {} must be '<case file name><TAB><declared outcome>'",
                index + 1
            ))
        })?;
        let name = name.trim();
        if name.is_empty() || name.len() > MAX_CASE_NAME_BYTES {
            return Err(UsageError::new(format!(
                "{MANIFEST_FILE} line {} names a case of 1-{MAX_CASE_NAME_BYTES} bytes",
                index + 1
            )));
        }
        let class = OutcomeClass::from_token(token.trim()).ok_or_else(|| {
            UsageError::new(format!(
                "{MANIFEST_FILE} line {} declares an unknown outcome class {:?}",
                index + 1,
                token.trim()
            ))
        })?;
        declared.insert(name.to_owned(), class);
    }
    Ok(declared)
}

/// Run every admitted case against every candidate and return ordered observations.
///
/// `max_observations` bounds how many observations are collected, and the record is
/// trimmed in a single pass against `max_record_bytes`; both truncations set
/// `observations_truncated` instead of silently shortening the record.
pub fn run(config: &Config) -> Result<Report, UsageError> {
    let (work, captures) = output_directories(config)?;
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
            let probed = probe_identity(config, candidate, launcher, &captures);
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
                config, candidate, launcher, case, &work, &captures, case_index,
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
        max_cases: config.max_cases,
        max_observations: config.max_observations,
        max_record_bytes: config.max_record_bytes,
        max_pixels: config.max_pixels,
        cases_truncated,
        observations_truncated,
        candidates: identities,
        observations,
    };
    // One rendering pass decides how many observations fit the byte budget.
    let (_, kept) = render(&report);
    if kept < report.observations.len() {
        report.observations.truncate(kept);
        report.observations_truncated = true;
    }
    if render(&report).0.len() > config.max_record_bytes {
        return Err(UsageError::new(format!(
            "the record header alone exceeds the {} byte cap",
            config.max_record_bytes
        )));
    }
    Ok(report)
}

/// Delete and recreate the operator work directory so every run starts empty.
fn output_directories(config: &Config) -> Result<(PathBuf, PathBuf), UsageError> {
    let parent = config
        .out
        .parent()
        .ok_or_else(|| UsageError::new("--out must name a file inside a directory"))?;
    let work = parent.join(OUTPUT_DIR);
    if work.exists() {
        fs::remove_dir_all(&work).map_err(|error| {
            UsageError::new(format!("cannot clear {}: {error}", work.display()))
        })?;
    }
    let captures = work.join(CAPTURE_DIR);
    fs::create_dir_all(&captures).map_err(|error| {
        UsageError::new(format!("cannot create {}: {error}", captures.display()))
    })?;
    Ok((work, captures))
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
/// format-inferring tool needs it; the script transport always writes
/// `<stem>.wic.png`, so its encoder format never depends on the input suffix and a
/// valid `.jpg`/`.tif`/`.gif` input is decoded instead of reported as unsupported.
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
        // Two script candidates on one case share this name by design; each
        // observation measures its own output immediately after its own child.
        Program::PwshScript(_) => work.join(format!("{stem}.wic.png")),
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
                Some(probe_scratch(&capture)),
            )
        });
        let (stdout, stderr) = capture.read(PROBE_CAP_BYTES);
        capture.release();
        let Some(supervised) = supervised else {
            continue;
        };
        probed.exit_codes[index] = supervised.exit_code;
        if supervised.timeout_fired || supervised.scratch_capped || supervised.exit_code != Some(0)
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

fn probe_scratch(capture: &CaptureFiles) -> ScratchBound<'_> {
    ScratchBound {
        first: &capture.stdout,
        second: &capture.stderr,
        limit: u64::try_from(PROBE_CAP_BYTES)
            .unwrap_or(u64::MAX)
            .saturating_mul(SCRATCH_FACTOR),
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
            .declared
            .get(&case.id)
            .copied()
            .unwrap_or(OutcomeClass::NotMeasured),
        observed_outcome: OutcomeClass::NotRun,
        candidate_reason: NOT_MEASURED,
        exit_code: None,
        timeout_fired: false,
        captured_stdout_bytes: 0,
        captured_stderr_bytes: 0,
        stdout_truncated: false,
        stderr_truncated: false,
        wall_ms: None,
        scratch_bytes: None,
        output_bytes: None,
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
    if let Err(reason) = clear_output(&output) {
        observation.observed_outcome = OutcomeClass::NotRun;
        observation.refusal = Some(reason);
        return observation;
    }
    let stem = format!(
        "{:03}_{}.{}",
        case_index,
        sanitize_name(&case.id),
        candidate.id
    );
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
            let scratch = ScratchBound {
                first: &capture.stdout,
                second: &capture.stderr,
                limit: u64::try_from(config.max_output_bytes)
                    .unwrap_or(u64::MAX)
                    .saturating_mul(SCRATCH_FACTOR),
            };
            let supervised = supervise(
                child,
                Duration::from_millis(config.timeout_ms),
                Some(scratch),
            );
            let (stdout, stderr) = capture.read(config.max_output_bytes);
            observation.wall_ms = Some(supervised.wall_ms);
            observation.scratch_bytes = Some(supervised.scratch_bytes);
            observation.timeout_fired = supervised.timeout_fired;
            observation.exit_code = supervised.exit_code;
            observation.candidate_reason = candidate_reason(&stdout);
            observation.captured_stdout_bytes = stdout.bytes.len();
            observation.captured_stderr_bytes = stderr.bytes.len();
            observation.stdout_truncated = stdout.truncated;
            observation.stderr_truncated = stderr.truncated;
            observation.observed_outcome = if supervised.scratch_capped || supervised.timeout_fired
            {
                OutcomeClass::Cancelled
            } else {
                classify(supervised.exit_code)
            };
            if supervised.scratch_capped {
                observation.refusal = Some("scratch_capped");
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
    observation.output_bytes = fs::metadata(&output)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .map(|metadata| metadata.len());
    observation
}

/// Remove any earlier output so a size can only come from this run, then prove the
/// path is really gone; a stale or unreusable output path fails closed. The run
/// already clears the whole work directory, so this is the per-observation guard
/// that keeps `output_bytes` attributable.
pub fn clear_output(path: &Path) -> Result<(), &'static str> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("output_stale_unremovable"),
    }
    if path.exists() {
        return Err("output_stale_present");
    }
    Ok(())
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

fn classify(exit_code: Option<i32>) -> OutcomeClass {
    match exit_code {
        Some(0) => OutcomeClass::Decoded,
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

/// Supervise one child under a hard deadline and a combined scratch-byte bound.
/// Capture lives in files, so nothing here can block on a descendant that
/// inherited the child's handles.
fn supervise(mut child: Child, timeout: Duration, scratch: Option<ScratchBound<'_>>) -> Supervised {
    let started = Instant::now();
    let mut timeout_fired = false;
    let mut scratch_capped = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            // A supervision error still reaps the child before returning.
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
        if let Some(bound) = scratch.as_ref() {
            let written = file_len(bound.first).saturating_add(file_len(bound.second));
            if written > bound.limit {
                let _ = child.kill();
                let _ = child.wait();
                scratch_capped = true;
                break None;
            }
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            let _ = child.kill();
            let _ = child.wait();
            timeout_fired = true;
            break None;
        }
        thread::sleep(remaining.min(POLL_INTERVAL));
    };
    let scratch_bytes = scratch.map_or(0, |bound| {
        file_len(bound.first).saturating_add(file_len(bound.second))
    });
    let reaped_without_status = timeout_fired || scratch_capped;
    Supervised {
        exit_code: if reaped_without_status {
            None
        } else {
            status.and_then(|status| status.code())
        },
        timeout_fired,
        wall_ms: elapsed_millis(started.elapsed()),
        scratch_bytes,
        scratch_capped,
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
         \"stderr_truncated\": {}, \"wall_ms\": {}, \"scratch_bytes\": {}, \"output_bytes\": {}, \
         \"refusal\": {}}}",
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
        number_field(observation.output_bytes),
        text_field(observation.refusal),
    )
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
