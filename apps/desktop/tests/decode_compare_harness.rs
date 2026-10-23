//! Contract tests for the exploratory MP-IMG2 Phase 1 decode-comparison harness.
//!
//! The candidate under test is this test binary itself, re-invoked as a helper
//! process with `DECODE_COMPARE_*` environment variables (the standard
//! self-reexec helper pattern). No external decoder, shell, network or Runtime is
//! used, and the only sleeps live inside helper child processes; no test sleeps to
//! drive an expectation.
//!
//! Each test names the review finding it closes (A1–A6, NB-1, NB-2, N6, NIT-4).

#[path = "../examples/decode_compare/harness.rs"]
mod harness;
#[path = "../examples/decode_compare/inspect.rs"]
mod inspect;

use harness::{Candidate, Invocation, OutcomeClass, Program, Report};
use std::{
    ffi::OsString,
    io::{Read, Write},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

const HELPER: &str = "fake_candidate_helper";
const HELPER_LONG_PROBE: &str = "fake_candidate_long_probe_helper";
const HELPER_NO_PROBE: &str = "fake_candidate_no_probe_helper";
const SLEEPER: &str = "fake_sleeper_helper";
const SLEEP_ENV: &str = "DECODE_COMPARE_SLEEP";
const IDENTITY: &str = "fake-decode-candidate 1.0";
const FAKE_OUTPUT: &[u8] = b"fake-decoded-output";
/// A flooding helper stops here if the harness never caps it (2 MiB is 4000x the
/// 512-byte bound used by the scratch-cap test, so a working cap kills it first).
const FLOOD_LIMIT_CHUNKS: usize = 2_000;
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// One manifest row: case name, optional `expected:WxH`, declared outcome class.
type Declaration<'a> = (&'a str, Option<(u32, u32)>, OutcomeClass);

struct Fixture {
    base: PathBuf,
    cases: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "img2-decode-compare-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let cases = base.join("cases");
        std::fs::create_dir_all(&cases).unwrap();
        Self { base, cases }
    }

    fn case(&self, name: &str, marker: &str) {
        std::fs::write(self.cases.join(name), marker).unwrap();
    }

    fn declare(&self, declarations: &[(&str, OutcomeClass)]) {
        let rows: Vec<Declaration<'_>> = declarations
            .iter()
            .map(|(name, class)| (*name, None, *class))
            .collect();
        self.declare_expected(&rows);
    }

    /// Write a manifest with optional `expected:WxH` dimensions per case.
    fn declare_expected(&self, declarations: &[Declaration<'_>]) {
        let mut text = String::from("# case file name[|expected:WxH][|declared-class]\n");
        for (name, expected, class) in declarations {
            text.push_str(name);
            if let Some((width, height)) = expected {
                text.push_str(&format!("|expected:{width}x{height}"));
            }
            text.push_str(&format!("|{}\n", class.as_str()));
        }
        std::fs::write(self.cases.join(harness::MANIFEST_FILE), text).unwrap();
    }

    fn record(&self, name: &str) -> PathBuf {
        self.base.join(format!("{name}.json"))
    }

    /// The shared work root; the harness creates it but never clears it.
    fn work(&self) -> PathBuf {
        self.base.join(harness::OUTPUT_DIR)
    }

    /// The test binary itself: absolute, local-disk, and never derived from media.
    fn fake(&self) -> Candidate {
        Candidate {
            id: "fake".to_owned(),
            program: Program::Executable(std::env::current_exe().unwrap()),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// Re-invoke the test binary for exactly one helper test, transporting the case
/// through the environment because libtest owns our argv.
fn helper_invocation(name: &str) -> Invocation {
    Invocation {
        argv_prefix: vec![
            "--exact".to_owned(),
            name.to_owned(),
            "--nocapture".to_owned(),
            "--quiet".to_owned(),
        ],
        env_transport: true,
    }
}

fn arguments(fixture: &Fixture, candidates: &[Candidate]) -> Vec<OsString> {
    let mut arguments = vec![
        OsString::from("--cases"),
        fixture.cases.clone().into_os_string(),
        OsString::from("--out"),
        fixture.record("record").into_os_string(),
    ];
    for candidate in candidates {
        arguments.push(OsString::from("--candidate"));
        arguments.push(OsString::from(format!(
            "{}={}",
            candidate.id,
            candidate.program.path().display()
        )));
    }
    arguments
}

fn measured(fixture: &Fixture, candidates: &[Candidate], helper: &str) -> Report {
    let mut config = harness::parse(arguments(fixture, candidates)).unwrap();
    config.invocation = helper_invocation(helper);
    harness::run(&config).unwrap()
}

fn as_strings(argv: &[OsString]) -> Vec<String> {
    argv.iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn normal_admit_records_decoded_identity_and_output_size() {
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");
    fixture.declare(&[("a.png", OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    assert_eq!(report.schema_version, harness::SCHEMA_VERSION);
    assert!(report.exploratory);
    assert_eq!(report.numeric_gate, harness::NUMERIC_GATE);
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].id, "fake");
    assert_eq!(report.candidates[0].kind, "executable");
    // NIT-2: the launcher is recorded, and for an executable it is the program.
    assert_eq!(
        report.candidates[0].launcher.as_deref(),
        Some(std::env::current_exe().unwrap().as_path())
    );
    // A3: this helper answers only the short form, so the shipped fallback runs.
    assert_eq!(report.candidates[0].identity.as_deref(), Some(IDENTITY));
    assert_eq!(report.candidates[0].identity_probe, Some("-version"));
    assert_eq!(report.candidates[0].identity_exit_codes, [Some(1), Some(0)]);
    assert_eq!(report.candidates[0].refusal, None);

    assert_eq!(report.observations.len(), 1);
    let observation = &report.observations[0];
    assert_eq!(observation.case_id, "a.png");
    assert_eq!(observation.suffix.as_deref(), Some("png"));
    assert_eq!(observation.input_bytes, Some(2));
    assert_eq!(observation.declared_outcome, OutcomeClass::Decoded);
    assert_eq!(observation.observed_outcome, OutcomeClass::Decoded);
    // NB-1: no whitelisted reason token on stdout stays not_measured.
    assert_eq!(observation.candidate_reason, "not_measured");
    assert_eq!(observation.exit_code, Some(0));
    assert!(!observation.timeout_fired);
    // The runner owns this helper's stdout, so exact byte accounting is proven on
    // the clean stderr channel and stdout capture only has to be non-empty.
    assert_eq!(observation.captured_stderr_bytes, "fake-marked\n".len());
    assert!(observation.captured_stdout_bytes > 0);
    assert!(!observation.stdout_truncated);
    assert!(observation.scratch_bytes.unwrap() > 0);
    // B-02: the evidence is recorded separately from the claim.
    assert!(observation.output_present);
    assert_eq!(observation.output_bytes, Some(FAKE_OUTPUT.len() as u64));
    assert_eq!(observation.validity, "presence_only");
    assert_eq!(observation.refusal, None);
    assert!(observation.wall_ms.is_some());
}

#[test]
fn declared_oversize_is_observed_from_the_protocol_exit_code() {
    let fixture = Fixture::new();
    fixture.case("big.png", "oversize");
    fixture.declare(&[("big.png", OutcomeClass::Oversize)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.declared_outcome, OutcomeClass::Oversize);
    assert_eq!(observation.observed_outcome, OutcomeClass::Oversize);
    assert_eq!(observation.exit_code, Some(4));
    assert_eq!(observation.output_bytes, None);
}

#[test]
fn corrupt_and_unsupported_stay_distinct_and_undeclared_cases_are_not_measured() {
    let fixture = Fixture::new();
    fixture.case("bad.png", "corrupt");
    fixture.case("odd.xyz", "unsupported");
    fixture.case("plain.png", "ok");
    fixture.declare(&[
        ("bad.png", OutcomeClass::Corrupt),
        ("odd.xyz", OutcomeClass::Unsupported),
    ]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let cases: Vec<(&str, OutcomeClass, OutcomeClass, Option<i32>)> = report
        .observations
        .iter()
        .map(|observation| {
            (
                observation.case_id.as_str(),
                observation.declared_outcome,
                observation.observed_outcome,
                observation.exit_code,
            )
        })
        .collect();
    assert_eq!(
        cases,
        vec![
            (
                "bad.png",
                OutcomeClass::Corrupt,
                OutcomeClass::Corrupt,
                Some(3)
            ),
            (
                "odd.xyz",
                OutcomeClass::Unsupported,
                OutcomeClass::Unsupported,
                Some(2)
            ),
            (
                "plain.png",
                OutcomeClass::NotMeasured,
                OutcomeClass::Decoded,
                Some(0)
            ),
        ]
    );
    assert_eq!(report.observations[2].suffix.as_deref(), Some("png"));
}

#[test]
fn hard_deadline_cancels_without_a_reported_exit_code() {
    let fixture = Fixture::new();
    fixture.case("slow.png", "slow");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.timeout_ms = 150;
    config.invocation = helper_invocation(HELPER);
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert!(observation.timeout_fired);
    assert_eq!(observation.observed_outcome, OutcomeClass::Cancelled);
    assert_eq!(observation.exit_code, None);
    // A timeout is not a scratch cap.
    assert_eq!(observation.refusal, None);
}

/// A2: capture must not depend on a pipe. A descendant of the candidate inherits
/// the redirected handles and outlives the direct child by seconds; with pipe
/// capture the parent would block until that descendant closed them, so the
/// assertion is on return latency (2.5 s descendant against a 1.5 s bound).
#[test]
fn deadline_holds_when_a_descendant_inherits_the_capture_handles() {
    let fixture = Fixture::new();
    fixture.case("orphan.png", "orphan");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.timeout_ms = 5_000;
    config.invocation = helper_invocation(HELPER);
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert_eq!(observation.observed_outcome, OutcomeClass::Decoded);
    assert!(!observation.timeout_fired);
    let wall_ms = observation.wall_ms.unwrap();
    assert!(
        wall_ms < 1_500,
        "the parent must not wait for an inherited capture handle ({wall_ms} ms)"
    );
}

#[test]
fn absent_candidate_is_not_run_instead_of_a_guessed_error() {
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");
    let absent = Candidate {
        id: "absent".to_owned(),
        program: Program::Executable(fixture.base.join("absent-candidate.exe")),
    };
    let report = measured(&fixture, &[absent], HELPER);

    assert_eq!(report.candidates[0].identity, None);
    assert_eq!(report.candidates[0].identity_probe, None);
    assert_eq!(report.candidates[0].identity_exit_codes, [None, None]);
    assert_eq!(report.candidates[0].refusal, None);
    let observation = &report.observations[0];
    assert_eq!(observation.observed_outcome, OutcomeClass::NotRun);
    assert_eq!(observation.refusal, Some("candidate_not_found"));
    assert_eq!(observation.exit_code, None);
    assert_eq!(observation.wall_ms, None);
    assert_eq!(observation.scratch_bytes, None);
    assert!(!observation.timeout_fired);
    assert!(!observation.output_present);
    assert_eq!(observation.validity, "not_measured");
}

/// A1: the manifest reader allocates at most cap + 1 bytes, so the documented cap
/// is a real bound and one extra byte fails closed.
#[test]
fn manifest_read_is_bounded_at_the_documented_limit() {
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");
    let manifest = fixture.cases.join(harness::MANIFEST_FILE);

    let mut text = String::from("#");
    text.push_str(&"x".repeat(harness::MAX_MANIFEST_BYTES - 1));
    assert_eq!(text.len(), harness::MAX_MANIFEST_BYTES);
    std::fs::write(&manifest, &text).unwrap();
    assert!(harness::parse(arguments(&fixture, &[fixture.fake()])).is_ok());

    text.push('x');
    std::fs::write(&manifest, &text).unwrap();
    let error = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap_err();
    let message = format!("{error}");
    assert!(message.contains("exceeds"), "{message}");
}

/// A3/A4/NIT-4: the shipped argv vectors are the documented ones for both
/// transports and every probe form, including the forwarded pixel bound, and the
/// CLI recognizes the `pwsh:` script transport. No shell string is ever assembled.
#[test]
fn argv_assembly_matches_the_documented_transports() {
    let input = PathBuf::from(r"C:\cases\a.png");
    let output = PathBuf::from(r"C:\work\000_a.png.wic.png");
    let script = PathBuf::from(r"C:\tools\wic_probe.ps1");
    let executable = Program::Executable(PathBuf::from(r"C:\tools\ffmpeg.exe"));
    let scripted = Program::PwshScript(script.clone());
    let plain = Invocation {
        argv_prefix: Vec::new(),
        env_transport: false,
    };
    let helper = Invocation {
        argv_prefix: vec!["--exact".to_owned(), HELPER.to_owned()],
        env_transport: true,
    };

    assert_eq!(
        as_strings(&harness::measurement_argv(
            &plain,
            &executable,
            &input,
            &output,
            None
        )),
        vec![r"-i", r"C:\cases\a.png", r"C:\work\000_a.png.wic.png"]
    );
    assert_eq!(
        as_strings(&harness::probe_argv(
            &plain,
            &executable,
            harness::ProbeForm::Long
        )),
        vec!["--version"]
    );
    assert_eq!(
        as_strings(&harness::probe_argv(
            &plain,
            &executable,
            harness::ProbeForm::Short
        )),
        vec!["-version"]
    );
    // NIT-4: the operator pixel bound reaches the script as its own argv elements.
    assert_eq!(
        as_strings(&harness::measurement_argv(
            &plain,
            &scripted,
            &input,
            &output,
            Some(1_048_576)
        )),
        vec![
            "-NoProfile",
            "-File",
            r"C:\tools\wic_probe.ps1",
            "-InputPath",
            r"C:\cases\a.png",
            "-OutputPath",
            r"C:\work\000_a.png.wic.png",
            "-MaxPixels",
            "1048576",
        ]
    );
    assert_eq!(
        as_strings(&harness::measurement_argv(
            &plain, &scripted, &input, &output, None
        )),
        vec![
            "-NoProfile",
            "-File",
            r"C:\tools\wic_probe.ps1",
            "-InputPath",
            r"C:\cases\a.png",
            "-OutputPath",
            r"C:\work\000_a.png.wic.png",
        ]
    );
    assert_eq!(
        as_strings(&harness::probe_argv(
            &plain,
            &scripted,
            harness::ProbeForm::Script
        )),
        vec!["-NoProfile", "-File", r"C:\tools\wic_probe.ps1", "-Version"]
    );
    // The environment transport carries the paths out of band, so only the prefix
    // reaches argv.
    assert_eq!(
        as_strings(&harness::measurement_argv(
            &helper,
            &executable,
            &input,
            &output,
            None
        )),
        vec!["--exact", HELPER]
    );

    let fixture = Fixture::new();
    let config = harness::parse(vec![
        OsString::from("--cases"),
        fixture.cases.clone().into_os_string(),
        OsString::from("--out"),
        fixture.record("record").into_os_string(),
        OsString::from("--candidate"),
        OsString::from(r"wic=pwsh:C:\tools\wic_probe.ps1"),
        OsString::from("--max-pixels"),
        OsString::from("1048576"),
    ])
    .unwrap();
    assert_eq!(config.candidates[0].program, scripted);
    assert_eq!(config.max_pixels, Some(1_048_576));
}

/// NB-1: the script transport always asks for a decodable `.wic.png`, so a valid
/// `.jpg`/`.tif`/`.gif` input can no longer be reported as `unsupported` merely
/// because the encoder has no matching format; an executable still keeps the case
/// suffix because a format-inferring tool needs it.
#[test]
fn script_transport_writes_a_fixed_png_output_for_any_input_suffix() {
    let work = PathBuf::from(r"C:\work");
    let scripted = Program::PwshScript(PathBuf::from(r"C:\tools\wic_probe.ps1"));
    let executable = Program::Executable(PathBuf::from(r"C:\tools\ffmpeg.exe"));

    for suffix in ["jpg", "jpeg", "tif", "tiff", "gif", "png", "bmp"] {
        let case_id = format!("photo.{suffix}");
        let path = harness::output_path(&scripted, &work, 3, &case_id, "wic", suffix);
        assert_eq!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("png"),
            "{case_id} must be decoded into a PNG the probe can always encode"
        );
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(format!("003_photo.{suffix}.wic.wic.png").as_str()),
            "B1: the candidate id is part of the name"
        );
    }

    assert_eq!(
        harness::output_path(&executable, &work, 3, "photo.jpg", "ffmpeg8", "jpg"),
        work.join("003_photo.jpg.ffmpeg8.jpg")
    );
}

/// NB-1: the probe's own reason is surfaced only through a closed vocabulary; an
/// unknown token is never copied into the record.
#[test]
fn candidate_reason_is_whitelisted_and_never_copied() {
    let fixture = Fixture::new();
    fixture.case("a-known.png", "reason-known");
    fixture.case("b-unknown.png", "reason-unknown");
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    assert_eq!(report.observations[0].candidate_reason, "win_bad_header");
    assert_eq!(report.observations[1].candidate_reason, "not_measured");
    assert_eq!(
        report.observations[1].observed_outcome,
        OutcomeClass::Corrupt
    );

    let json = harness::render(&report).0;
    assert!(
        json.contains("\"candidate_reason\": \"win_bad_header\""),
        "the known token must be recorded"
    );
    assert!(
        !json.contains("totally_made_up"),
        "an unknown token must never be copied"
    );
}

/// A3: the long probe form is preferred when it works, and an absent identity is
/// recorded as absent (never guessed) while measurement still proceeds.
#[test]
fn identity_probe_prefers_the_long_form_and_reports_absence() {
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");

    let long = measured(&fixture, &[fixture.fake()], HELPER_LONG_PROBE);
    assert_eq!(long.candidates[0].identity.as_deref(), Some(IDENTITY));
    assert_eq!(long.candidates[0].identity_probe, Some("--version"));
    assert_eq!(long.candidates[0].identity_exit_codes, [Some(0), None]);
    assert_eq!(long.observations[0].observed_outcome, OutcomeClass::Decoded);

    let absent = measured(&fixture, &[fixture.fake()], HELPER_NO_PROBE);
    assert_eq!(absent.candidates[0].identity, None);
    assert_eq!(absent.candidates[0].identity_probe, None);
    assert_eq!(absent.candidates[0].identity_exit_codes, [Some(1), Some(1)]);
    // A missing identity is a probe result, not a measurement failure.
    assert_eq!(
        absent.observations[0].observed_outcome,
        OutcomeClass::Decoded
    );
}

#[test]
fn captured_output_is_capped_and_marked_truncated() {
    let fixture = Fixture::new();
    fixture.case("flood.png", "flood");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    // The read cap bounds what is recorded; this helper stays below the separate
    // scratch bound (cap * 8) so the case still completes normally.
    config.max_output_bytes = 256;
    config.invocation = helper_invocation(HELPER);
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert_eq!(observation.captured_stdout_bytes, 256);
    assert!(observation.stdout_truncated);
    assert!(!observation.stderr_truncated);
    assert_eq!(observation.observed_outcome, OutcomeClass::Decoded);
    assert_eq!(observation.refusal, None);
}

/// NB-2: a candidate that keeps writing is killed once the combined capture files
/// exceed `max_output_bytes * 8`, and the observed class and size are recorded.
#[test]
fn scratch_cap_kills_a_flooding_candidate() {
    let fixture = Fixture::new();
    fixture.case("flood.png", "floodcap");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.max_output_bytes = 64;
    config.invocation = helper_invocation(HELPER);
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert_eq!(observation.observed_outcome, OutcomeClass::Cancelled);
    assert_eq!(observation.refusal, Some("scratch_capped"));
    assert!(!observation.timeout_fired);
    assert_eq!(observation.exit_code, None);
    let scratch = observation.scratch_bytes.unwrap();
    assert!(
        scratch > 64 * harness::SCRATCH_FACTOR,
        "the observed scratch size must exceed the bound ({scratch})"
    );
    assert!(
        scratch < (FLOOD_LIMIT_CHUNKS as u64 * 1024),
        "the child must be killed well before it finishes flooding ({scratch})"
    );
}

/// B-02: a successful exit without an output file is not `decoded`.
#[test]
fn successful_exit_without_output_is_not_decoded() {
    let fixture = Fixture::new();
    fixture.case("a.png", "nooutput");
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.exit_code, Some(0));
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("output_missing"));
    assert!(!observation.output_present);
    assert_eq!(observation.output_bytes, None);
    assert_eq!(observation.validity, "not_measured");
}

/// B-02: a zero-byte output file is not evidence of a decode either.
#[test]
fn zero_byte_output_is_not_decoded() {
    let fixture = Fixture::new();
    fixture.case("a.png", "emptyoutput");
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.exit_code, Some(0));
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("output_missing"));
    assert!(!observation.output_present);
    assert_eq!(observation.output_bytes, Some(0));
    assert_eq!(observation.validity, "not_measured");
}

/// B-03: a flood that finishes between two polls is still capped after exit.
#[test]
fn fast_flood_finishing_immediately_is_still_capped() {
    let fixture = Fixture::new();
    fixture.case("a.png", "burst");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    // The scratch bound is max_output_bytes * 8 = 512 bytes.
    config.max_output_bytes = 64;
    config.invocation = helper_invocation(HELPER);
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert_eq!(observation.observed_outcome, OutcomeClass::Cancelled);
    assert_eq!(observation.refusal, Some("scratch_capped"));
    let scratch = observation.scratch_bytes.unwrap();
    assert!(
        scratch > 64 * harness::SCRATCH_FACTOR,
        "the post-exit re-check must see the burst ({scratch})"
    );
    assert_ne!(observation.validity, "presence_only");
}

/// B-03: an oversized candidate output is capped even though the exit code is 0.
#[test]
fn oversized_output_file_is_capped() {
    let fixture = Fixture::new();
    fixture.case("a.png", "bigoutput");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.max_output_file_bytes = 1024;
    config.invocation = helper_invocation(HELPER);
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert_eq!(observation.observed_outcome, OutcomeClass::Cancelled);
    assert_eq!(observation.refusal, Some("output_capped"));
    assert!(
        observation.output_bytes.unwrap() > 1024,
        "the observed output size must exceed the bound"
    );
    assert_ne!(observation.validity, "presence_only");
}

/// B-R2-01: nothing that already lives in the shared work root is deleted or
/// rewritten, even when its name looks exactly like one of the harness's own.
#[test]
fn preexisting_work_root_is_never_modified() {
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");
    let base = fixture.work();
    std::fs::create_dir_all(base.join("foreign")).unwrap();
    let planted = [
        (
            base.join("123_operator_notes.txt"),
            b"operator notes".to_vec(),
        ),
        (base.join("notes.stdout"), b"looks like a capture".to_vec()),
        (base.join("foreign").join("keep.bin"), vec![7_u8; 1024]),
        (
            base.join(".decode_compare_owned"),
            b"cos-desktop decode_compare work dir schema=5".to_vec(),
        ),
    ];
    for (path, bytes) in &planted {
        std::fs::write(path, bytes).unwrap();
    }

    let report = measured(&fixture, &[fixture.fake()], HELPER);
    assert_eq!(report.observations.len(), 1);
    assert_eq!(
        report.observations[0].observed_outcome,
        OutcomeClass::Decoded
    );

    for (path, bytes) in &planted {
        assert_eq!(
            &std::fs::read(path).unwrap(),
            bytes,
            "{path:?} must be byte-identical after the run"
        );
    }
    assert!(base.join("foreign").is_dir());
    // The run removed only the directory it created itself.
    let mut remaining: Vec<String> = std::fs::read_dir(&base)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    remaining.sort();
    assert_eq!(
        remaining,
        vec![
            ".decode_compare_owned",
            "123_operator_notes.txt",
            "foreign",
            "notes.stdout"
        ]
    );
}

/// B-R2-01: the run's own directory is removed when the run ends, so no `captures`
/// directory and no output survives a completed run.
#[test]
fn run_directory_and_captures_are_removed_when_the_run_finishes() {
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    assert_eq!(report.observations.len(), 1);
    assert_eq!(
        report.observations[0].output_bytes,
        Some(FAKE_OUTPUT.len() as u64),
        "the output is measured inside the run directory before cleanup"
    );
    let base = fixture.work();
    assert!(base.is_dir(), "the shared work root stays in place");
    assert_eq!(
        std::fs::read_dir(&base).unwrap().count(),
        0,
        "only the run's own directory existed, and it is gone"
    );
}

/// B-R2-01: two runs never share a directory. A token that is already taken is
/// refused instead of reused, and two full runs both succeed on their own token.
#[test]
fn two_runs_never_share_a_directory() {
    let first = harness::run_token();
    let second = harness::run_token();
    assert_ne!(first, second, "run tokens must be unique per call");
    assert!(first.starts_with(harness::RUN_TOKEN_PREFIX));

    let root = std::env::temp_dir().join(format!(
        "img2-run-token-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let token = harness::run_token();
    let directory = harness::create_run_directory(&root, &token).unwrap();
    assert!(directory.is_dir());
    let error = harness::create_run_directory(&root, &token).unwrap_err();
    let message = format!("{error}");
    assert!(
        message.contains(harness::WORK_DIR_EXISTS),
        "unexpected refusal: {message}"
    );
    assert!(
        directory.is_dir(),
        "a refused token leaves the existing directory alone"
    );
    let other = harness::create_run_directory(&root, &harness::run_token()).unwrap();
    assert_ne!(directory, other);
    let _ = std::fs::remove_dir_all(&root);

    let fixture = Fixture::new();
    fixture.case("a.png", "ok");
    let first_run = measured(&fixture, &[fixture.fake()], HELPER);
    let second_run = measured(&fixture, &[fixture.fake()], HELPER);
    assert_eq!(
        first_run.observations[0].output_bytes,
        Some(FAKE_OUTPUT.len() as u64)
    );
    assert_eq!(
        second_run.observations[0].output_bytes,
        Some(FAKE_OUTPUT.len() as u64)
    );
    assert_eq!(std::fs::read_dir(fixture.work()).unwrap().count(), 0);
}

/// B-R2-01: the record stays deterministic — the run token and the work directory
/// are internal plumbing and never leak into the emitted JSON.
#[test]
fn run_token_never_appears_in_the_record() {
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let json = harness::render(&report).0;
    assert!(
        !json.contains(harness::RUN_TOKEN_PREFIX),
        "the run token must not appear in the record"
    );
    assert!(
        !json.contains(harness::OUTPUT_DIR),
        "the work directory must not appear in the record"
    );
    assert!(
        !json.contains(&std::env::temp_dir().display().to_string()),
        "temporary paths must not appear in the record"
    );
}

/// Phase 2: the inspector reads PNG, BMP and JPEG headers itself, and reports
/// nothing it could not parse.
#[test]
fn output_inspector_reads_png_bmp_and_jpeg_headers() {
    let mut png = std::io::Cursor::new(png_bytes(16, 9, 6, true));
    let inspected = inspect::inspect(&mut png);
    assert_eq!(inspected.format, Some(inspect::Format::Png));
    assert!(inspected.complete, "IEND must be observed");
    assert_eq!(inspected.dimensions(), Some((16, 9)));
    assert_eq!(inspected.bit_depth, Some(8));
    assert_eq!(inspected.channels, Some(4));
    assert_eq!(inspected.colour_type, Some(inspect::ColourType::Rgba));
    assert_eq!(inspected.colour_markers, vec![inspect::ColourMarker::Srgb]);

    let mut bmp = std::io::Cursor::new(bmp_bytes(8, 4, 24));
    let inspected = inspect::inspect(&mut bmp);
    assert_eq!(inspected.format, Some(inspect::Format::Bmp));
    assert_eq!(inspected.dimensions(), Some((8, 4)));
    assert_eq!(inspected.bit_depth, Some(24));
    assert_eq!(inspected.channels, Some(3));
    assert_eq!(inspected.colour_type, Some(inspect::ColourType::Rgb));

    let mut jpeg = std::io::Cursor::new(jpeg_bytes(24, 12, 3));
    let inspected = inspect::inspect(&mut jpeg);
    assert_eq!(inspected.format, Some(inspect::Format::Jpeg));
    assert_eq!(inspected.dimensions(), Some((24, 12)));
    assert_eq!(inspected.bit_depth, Some(8));
    assert_eq!(inspected.channels, Some(3));
    assert_eq!(inspected.colour_type, Some(inspect::ColourType::Rgb));

    // A truncated PNG keeps only the signature observation and no dimensions.
    let mut truncated = std::io::Cursor::new(png_bytes(16, 16, 2, false)[..12].to_vec());
    let inspected = inspect::inspect(&mut truncated);
    assert_eq!(inspected.format, Some(inspect::Format::Png));
    assert_eq!(inspected.dimensions(), None);
    assert!(!inspected.complete);

    // Garbage is not guessed into a format.
    let mut garbage = std::io::Cursor::new(vec![0xAB_u8; 64]);
    let inspected = inspect::inspect(&mut garbage);
    assert_eq!(inspected.format, None);
    assert_eq!(inspected.dimensions(), None);
    assert!(inspected.colour_markers.is_empty());
}

/// Phase 2: a parsed output that matches the operator expectation is verified.
#[test]
fn content_verified_when_the_output_matches_the_expectation() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png16");
    fixture.declare_expected(&[("a.png", Some((16, 16)), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.observed_outcome, OutcomeClass::Decoded);
    assert_eq!(observation.validity, "content_verified");
    assert_eq!(observation.validity_reason, None);
    assert_eq!(observation.inspected_format, inspect::Format::Png);
    assert_eq!(observation.inspected_width, Some(16));
    assert_eq!(observation.inspected_height, Some(16));
    assert_eq!(observation.inspected_bit_depth, Some(8));
    assert_eq!(observation.inspected_channels, Some(3));
    assert_eq!(observation.expected_dimensions, Some((16, 16)));
    assert_eq!(observation.refusal, None);
}

/// Phase 2: a parsed output that contradicts the expectation is an error, never a
/// success class, and the mismatch is visible in both the class and the validity.
#[test]
fn dimension_mismatch_demotes_decoded_to_error() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png16");
    fixture.declare_expected(&[("a.png", Some((32, 32)), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(
        observation.exit_code,
        Some(0),
        "the child still exited successfully"
    );
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("dimension_mismatch"));
    assert_eq!(observation.validity, "dimension_mismatch");
    assert_eq!(observation.validity_reason, Some("dimension_mismatch"));
    assert_eq!(observation.inspected_width, Some(16));
    assert_eq!(observation.inspected_height, Some(16));
    assert_eq!(observation.expected_dimensions, Some((32, 32)));
}

/// Phase 2/N5: an expected output the inspector cannot parse is `error` with
/// refusal `output_unparsable` — the record must not keep a `decoded` success claim
/// it cannot support, and the raw exit code stays as evidence.
#[test]
fn unparsable_output_with_an_expectation_is_not_measured() {
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");
    fixture.declare_expected(&[("a.png", Some((16, 16)), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(
        observation.exit_code,
        Some(0),
        "the child still exited successfully"
    );
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("output_unparsable"));
    assert_eq!(observation.validity, "not_measured");
    assert_eq!(observation.validity_reason, Some("output_unparsable"));
    assert_eq!(observation.inspected_format, inspect::Format::NotMeasured);
    assert_eq!(observation.inspected_width, None);
}

/// Phase 2: the capability table holds one row per candidate × observed format,
/// derived only from parsed outputs, with sorted de-duplicated values.
#[test]
fn capability_rows_are_derived_from_parsed_outputs() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png16");
    fixture.case("b.png", "png32");
    fixture.case("c.png", "png-srgb");
    fixture.case("d.bmp", "bmp");
    fixture.case("e.jpg", "jpeg");
    fixture.case("f.bin", "ok");
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let formats: Vec<&str> = report
        .capabilities
        .iter()
        .map(|row| row.format.as_str())
        .collect();
    assert_eq!(formats, vec!["png", "bmp", "jpeg"], "sorted by format");

    let png = &report.capabilities[0];
    assert_eq!(png.observations, 3);
    assert_eq!((png.max_width, png.max_height), (Some(32), Some(32)));
    assert_eq!(png.bit_depths, vec![8]);
    assert_eq!(png.channels, vec![3, 4]);
    assert_eq!(
        png.colour_types,
        vec![inspect::ColourType::Rgb, inspect::ColourType::Rgba]
    );
    assert_eq!(png.colour_markers, vec![inspect::ColourMarker::Srgb]);

    let bmp = &report.capabilities[1];
    assert_eq!(bmp.observations, 1);
    assert_eq!((bmp.max_width, bmp.max_height), (Some(8), Some(4)));
    assert_eq!(bmp.bit_depths, vec![24]);

    let jpeg = &report.capabilities[2];
    assert_eq!(jpeg.observations, 1);
    assert_eq!((jpeg.max_width, jpeg.max_height), (Some(24), Some(12)));

    // The garbage output produced no capability row at all.
    assert!(
        report
            .observations
            .iter()
            .any(|observation| observation.case_id == "f.bin"
                && observation.inspected_format == inspect::Format::NotMeasured)
    );

    // The table is rendered with explicit not_measured facts and reasons.
    let json = harness::render(&report).0;
    assert!(json.contains("\"capabilities\": ["));
    assert!(json.contains("\"orientation\": \"not_measured\""));
    assert!(json.contains("\"orientation_reason\": \"exif_orientation_not_parsed\""));
    assert!(json.contains("\"region_reason\": \"partial_decode_not_observed\""));
    assert!(json.contains("\"sequential_reason\": \"sequential_capability_not_observed\""));
}

/// Phase 2: capability rows are deterministic across runs.
#[test]
fn capability_table_is_deterministic() {
    let fixture = Fixture::new();
    fixture.case("b.png", "png32");
    fixture.case("a.png", "png16");
    let first = measured(&fixture, &[fixture.fake()], HELPER);
    let second = measured(&fixture, &[fixture.fake()], HELPER);
    assert_eq!(first.capabilities.len(), 1);
    assert_eq!(
        harness::render(&first).0.contains("\"max_width\": 32"),
        harness::render(&second).0.contains("\"max_width\": 32")
    );
    assert_eq!(
        first.capabilities[0].observations,
        second.capabilities[0].observations
    );
}

/// B1: every candidate gets its own output name, and a path that already exists is
/// refused before any child is spawned.
#[test]
fn output_names_are_per_candidate_and_stale_paths_are_refused() {
    let work = PathBuf::from(r"C:\work");
    let scripted = Program::PwshScript(PathBuf::from(r"C:\tools\wic_probe.ps1"));
    let first = harness::output_path(&scripted, &work, 0, "a.png", "wic", "png");
    let second = harness::output_path(&scripted, &work, 0, "a.png", "wic-alt", "png");
    assert_ne!(
        first, second,
        "two script candidates on one case must never share a path"
    );
    assert_eq!(
        first.file_name().and_then(|name| name.to_str()),
        Some("000_a.png.wic.wic.png")
    );
    assert_eq!(
        second.file_name().and_then(|name| name.to_str()),
        Some("000_a.png.wic-alt.wic.png")
    );

    let root = std::env::temp_dir().join(format!(
        "img2-stale-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let taken = root.join("taken.png");
    std::fs::write(&taken, b"x").unwrap();
    assert_eq!(
        harness::require_absent_output(&taken),
        Err("output_stale_present")
    );
    assert_eq!(
        harness::require_absent_output(&root.join("free.png")),
        Ok(())
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// B1: a silent candidate never inherits another candidate's output.
#[test]
fn silent_candidate_never_inherits_another_candidates_output() {
    let fixture = Fixture::new();
    fixture.case("a.png", "unless-silent");
    fixture.declare_expected(&[("a.png", Some((16, 16)), OutcomeClass::Decoded)]);
    let mut writer = fixture.fake();
    writer.id = "writer".to_owned();
    let mut silent = fixture.fake();
    silent.id = "silent".to_owned();
    let report = measured(&fixture, &[writer, silent], HELPER);

    let writer_row = report
        .observations
        .iter()
        .find(|observation| observation.candidate_id == "writer")
        .unwrap();
    assert_eq!(writer_row.observed_outcome, OutcomeClass::Decoded);
    assert_eq!(writer_row.validity, "content_verified");

    let silent_row = report
        .observations
        .iter()
        .find(|observation| observation.candidate_id == "silent")
        .unwrap();
    assert_eq!(silent_row.exit_code, Some(0));
    assert_eq!(silent_row.observed_outcome, OutcomeClass::Error);
    assert_eq!(silent_row.refusal, Some("output_missing"));
    assert!(!silent_row.output_present);
    assert_eq!(silent_row.validity, "not_measured");
    assert_ne!(silent_row.validity, "content_verified");

    let candidates: Vec<&str> = report
        .capabilities
        .iter()
        .map(|row| row.candidate_id.as_str())
        .collect();
    assert_eq!(candidates, vec!["writer"], "the silent leg gets no row");
}

/// B2: a container that stops before its terminator never verifies and never
/// produces a capability row.
#[test]
fn incomplete_outputs_are_never_verified() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png-header-only");
    fixture.case("b.jpg", "jpeg-sof-only");
    fixture.case("c.bmp", "bmp-header-only");
    fixture.declare_expected(&[
        ("a.png", Some((16, 16)), OutcomeClass::Decoded),
        ("b.jpg", Some((24, 12)), OutcomeClass::Decoded),
        ("c.bmp", Some((8, 4)), OutcomeClass::Decoded),
    ]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    for observation in &report.observations {
        assert_eq!(
            observation.exit_code,
            Some(0),
            "{} still exited successfully",
            observation.case_id
        );
        assert_ne!(
            observation.observed_outcome,
            OutcomeClass::Decoded,
            "{} must not claim a decode the bytes do not support",
            observation.case_id
        );
        assert_eq!(observation.validity, "not_measured");
        assert!(
            matches!(
                observation.refusal,
                Some("output_incomplete") | Some("output_unparsable")
            ),
            "{}: unexpected refusal {:?}",
            observation.case_id,
            observation.refusal
        );
    }
    assert!(
        report.capabilities.is_empty(),
        "no capability row may come from an incomplete container"
    );
}

/// A1: capability rows come only from decoded observations with a presence or
/// content validity, so a mismatch and a truncated output in the same run never
/// contribute.
#[test]
fn capability_rows_need_a_verified_decode() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png16");
    fixture.case("b.png", "png16");
    fixture.case("c.png", "png-header-only");
    fixture.declare_expected(&[
        ("a.png", Some((16, 16)), OutcomeClass::Decoded),
        ("b.png", Some((32, 32)), OutcomeClass::Decoded),
        ("c.png", Some((16, 16)), OutcomeClass::Decoded),
    ]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let by_case = |case: &str| {
        report
            .observations
            .iter()
            .find(|observation| observation.case_id == case)
            .unwrap()
    };
    assert_eq!(by_case("a.png").validity, "content_verified");
    assert_eq!(by_case("b.png").validity, "dimension_mismatch");
    assert_eq!(by_case("b.png").observed_outcome, OutcomeClass::Error);
    assert_eq!(by_case("c.png").validity, "not_measured");

    assert_eq!(report.capabilities.len(), 1);
    assert_eq!(report.capabilities[0].observations, 1);
    assert_eq!(report.capabilities[0].max_width, Some(16));
}

/// A2: an illegal header field combination is not measured and not a capability.
#[test]
fn illegal_header_combinations_are_not_measured() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png-illegal");
    fixture.declare_expected(&[("a.png", Some((16, 16)), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.exit_code, Some(0));
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("output_unparsable"));
    assert_eq!(observation.validity, "not_measured");
    assert_eq!(observation.inspected_format, inspect::Format::NotMeasured);
    assert_eq!(observation.inspected_width, None);
    assert!(report.capabilities.is_empty());
}

/// A3: the capability table is built after observation truncation.
#[test]
fn capabilities_ignore_truncated_observations() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png16");
    fixture.case("b.png", "png16");
    fixture.case("c.png", "png16");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.max_record_bytes = 2600;
    config.invocation = helper_invocation(HELPER);
    let report = harness::run(&config).unwrap();

    assert!(
        report.observations.len() < 3,
        "the record must be truncated"
    );
    assert_eq!(
        report.capabilities[0].observations,
        report.observations.len(),
        "the table may only count observations that survived the budget"
    );
}

/// A4: the manifest fails closed on an unknown case name and on the legacy TAB
/// separator.
#[test]
fn manifest_rejects_unknown_names_and_the_tab_separator() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png16");

    std::fs::write(
        fixture.cases.join(harness::MANIFEST_FILE),
        "ghost.png|expected:16x16|decoded\n",
    )
    .unwrap();
    let error = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap_err();
    assert!(
        format!("{error}").contains("unknown case"),
        "unexpected refusal: {error}"
    );

    std::fs::write(
        fixture.cases.join(harness::MANIFEST_FILE),
        "a.png\tdecoded\n",
    )
    .unwrap();
    let error = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap_err();
    assert!(
        format!("{error}").contains("legacy TAB separator"),
        "unexpected refusal: {error}"
    );
}

/// N1: a PNG terminator must be an empty IEND, and IHDR may appear only once and
/// first.
#[test]
fn png_terminator_and_ihdr_order_are_enforced() {
    let mut bad_iend = std::io::Cursor::new(png_with_iend_length(16, 16, 5));
    let inspected = inspect::inspect(&mut bad_iend);
    assert_eq!(inspected.dimensions(), Some((16, 16)));
    assert!(
        !inspected.complete,
        "IEND with a payload is not a terminator"
    );

    let mut duplicate = std::io::Cursor::new(png_duplicate_ihdr(16, 16));
    let inspected = inspect::inspect(&mut duplicate);
    assert!(!inspected.complete);

    let mut header_only = std::io::Cursor::new(png_header_only(16, 16));
    let inspected = inspect::inspect(&mut header_only);
    assert_eq!(inspected.format, Some(inspect::Format::Png));
    assert_eq!(inspected.dimensions(), Some((16, 16)));
    assert!(!inspected.complete, "33 bytes carry no IEND");
}

/// N2: a set the headers never stated is named explicitly with a reason token.
#[test]
fn empty_capability_sets_are_explicit() {
    let fixture = Fixture::new();
    fixture.case("a.bmp", "bmp-paletted");
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let row = &report.capabilities[0];
    assert_eq!(row.bit_depths, vec![8]);
    assert!(
        row.channels.is_empty(),
        "a paletted bitmap states no channels"
    );
    assert!(row.colour_types.is_empty());

    let json = harness::render(&report).0;
    assert!(json.contains("\"not_measured\": [\"channels\", \"colour_types\"]"));
    assert!(json.contains("\"not_measured_reason\": \"header_field_not_observed\""));
}

/// A6: both global bounds truncate explicitly instead of growing the record.
#[test]
fn observation_and_record_caps_truncate_explicitly() {
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");
    fixture.case("b.png", "ok");
    fixture.case("c.png", "ok");

    let mut bounded = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    bounded.invocation = helper_invocation(HELPER);
    bounded.max_observations = 2;
    let report = harness::run(&bounded).unwrap();
    assert!(report.observations_truncated);
    assert_eq!(report.observations.len(), 2);
    assert!(
        harness::render(&report)
            .0
            .contains("\"observations_truncated\": true"),
        "the truncation must be recorded"
    );

    let mut wide = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    wide.invocation = helper_invocation(HELPER);
    let full = harness::run(&wide).unwrap();
    let full_len = harness::render(&full).0.len();
    assert_eq!(full.observations.len(), 3);

    // NIT-1: the byte budget is enforced in one rendering pass and reported.
    let mut tight = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    tight.invocation = helper_invocation(HELPER);
    tight.max_record_bytes = full_len - 300;
    let tight_report = harness::run(&tight).unwrap();
    let json = harness::render(&tight_report).0;
    assert!(json.len() <= tight.max_record_bytes);
    assert!(tight_report.observations_truncated);
    assert!(tight_report.observations.len() < 3);

    // NIT-1: the writer refuses a report above its configured budget.
    let mut oversized = harness::run(&wide).unwrap();
    oversized.max_record_bytes = 600;
    let refused = fixture.record("oversized");
    assert!(harness::write_report(&oversized, &refused).is_err());
    assert!(!refused.exists());
}

/// N6: without a case suffix the candidate output format cannot be expressed, so
/// nothing is measured and no unusable `.output` name is invented.
#[test]
fn extensionless_case_is_not_measured_with_a_reason() {
    let fixture = Fixture::new();
    fixture.case("noext", "ok");
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.suffix, None);
    assert_eq!(observation.observed_outcome, OutcomeClass::NotMeasured);
    assert_eq!(observation.refusal, Some("extension_needed_for_output"));
    assert_eq!(observation.exit_code, None);
    assert_eq!(observation.wall_ms, None);
    assert_eq!(observation.output_bytes, None);
}

#[test]
fn max_cases_bounds_the_record_and_flags_truncation() {
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");
    fixture.case("b.png", "ok");
    fixture.case("c.png", "ok");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.max_cases = 2;
    config.invocation = helper_invocation(HELPER);
    let report = harness::run(&config).unwrap();

    assert!(report.cases_truncated);
    assert_eq!(
        report
            .observations
            .iter()
            .map(|observation| observation.case_id.as_str())
            .collect::<Vec<&str>>(),
        vec!["a.png", "b.png"]
    );
}

#[test]
fn record_is_deterministic_sorted_and_carries_no_decision_field() {
    let fixture = Fixture::new();
    fixture.case("b.png", "ok");
    fixture.case("a.png", "ok");
    fixture.declare(&[
        ("a.png", OutcomeClass::Decoded),
        ("b.png", OutcomeClass::Decoded),
    ]);
    let present = fixture.fake();
    let absent = Candidate {
        id: "absent".to_owned(),
        program: Program::Executable(fixture.base.join("absent-candidate.exe")),
    };
    let candidates = [absent, present];

    let first = measured(&fixture, &candidates, HELPER);
    harness::write_report(&first, &fixture.record("first")).unwrap();
    let second = measured(&fixture, &candidates, HELPER);
    harness::write_report(&second, &fixture.record("second")).unwrap();

    let first_json = std::fs::read_to_string(fixture.record("first")).unwrap();
    let second_json = std::fs::read_to_string(fixture.record("second")).unwrap();
    assert_eq!(
        normalize_wall_ms(&first_json),
        normalize_wall_ms(&second_json),
        "the record must not depend on wall time for its structure"
    );
    assert_eq!(
        json_values(&first_json, "candidate_id"),
        vec!["absent", "fake", "absent", "fake"]
    );
    assert_eq!(
        json_values(&first_json, "case_id"),
        vec!["a.png", "a.png", "b.png", "b.png"]
    );
    assert!(first_json.contains("\"schema_version\": 6"));
    assert!(first_json.contains("\"exploratory\": true"));
    assert!(first_json.contains("\"numeric_gate\": \"NOT_RATIFIED\""));
    for decision in ["\"pass\"", "\"winner\"", "\"score\"", "\"selected\""] {
        assert!(
            !first_json.contains(decision),
            "record must not carry {decision}"
        );
    }
}

#[test]
fn relative_derived_or_parent_candidate_paths_are_refused() {
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");
    let derived = format!("fake={}\\..\\fake.exe", fixture.cases.display());
    for spec in [
        "fake=ffmpeg.exe",
        "fake=.\\fake.exe",
        "fake=cases\\fake.exe",
        derived.as_str(),
        "fake=pwsh:wic_probe.ps1",
    ] {
        let error = harness::parse(vec![
            OsString::from("--cases"),
            fixture.cases.clone().into_os_string(),
            OsString::from("--out"),
            fixture.record("refused").into_os_string(),
            OsString::from("--candidate"),
            OsString::from(spec),
        ])
        .unwrap_err();
        let message = format!("{error}");
        assert!(
            message.contains("absolute path") || message.contains("parent or current"),
            "{spec} must be refused, got {message}"
        );
    }

    // A missing required argument is a documented non-zero usage exit, not a record.
    let out = fixture.record("missing");
    let code = harness::run_from_args(vec![
        OsString::from("--cases"),
        fixture.cases.clone().into_os_string(),
        OsString::from("--out"),
        out.clone().into_os_string(),
    ]);
    assert_eq!(code, 2);
    assert!(!out.exists());
}

/// NIT-1: the observation limit is bounded to a sane maximum.
#[test]
fn max_observations_is_bounded_by_the_cli() {
    let fixture = Fixture::new();
    let error = harness::parse(vec![
        OsString::from("--cases"),
        fixture.cases.clone().into_os_string(),
        OsString::from("--out"),
        fixture.record("record").into_os_string(),
        OsString::from("--candidate"),
        OsString::from(format!("fake={}", fixture.fake().program.path().display())),
        OsString::from("--max-observations"),
        OsString::from((harness::MAX_OBSERVATIONS + 1).to_string()),
    ])
    .unwrap_err();
    let message = format!("{error}");
    assert!(message.contains("--max-observations"), "{message}");
}

/// The self-reexec helper: identity probe, scripted decode behaviour, or a plain
/// pass when the suite itself runs this test.
#[test]
fn fake_candidate_helper() {
    fake_candidate(ProbeSupport::ShortFormOnly);
}

#[test]
fn fake_candidate_long_probe_helper() {
    fake_candidate(ProbeSupport::LongFormOnly);
}

#[test]
fn fake_candidate_no_probe_helper() {
    fake_candidate(ProbeSupport::None);
}

/// The descendant that outlives the candidate in the A2 test.
#[test]
fn fake_sleeper_helper() {
    if std::env::var_os(SLEEP_ENV).is_none() {
        return;
    }
    std::thread::sleep(std::time::Duration::from_millis(2_500));
}

#[derive(Clone, Copy)]
enum ProbeSupport {
    LongFormOnly,
    ShortFormOnly,
    None,
}

fn fake_candidate(support: ProbeSupport) {
    if let Some(form) = std::env::var_os(harness::ENV_PROBE) {
        let form = form.to_string_lossy().into_owned();
        let answered = match support {
            ProbeSupport::LongFormOnly => form == "--version",
            ProbeSupport::ShortFormOnly => form == "-version",
            ProbeSupport::None => false,
        };
        if answered {
            // The environment transport reads the probe answer from stderr because
            // the test runner owns this process's stdout.
            eprintln!("{IDENTITY}");
            std::process::exit(0);
        }
        std::process::exit(1);
    }
    let Some(input) = std::env::var_os(harness::ENV_INPUT) else {
        return;
    };
    let output = std::env::var_os(harness::ENV_OUTPUT).unwrap();
    let mut marker = [0_u8; 16];
    let mut file = std::fs::File::open(&input).unwrap();
    let read = file.read(&mut marker).unwrap();
    let marker = String::from_utf8_lossy(&marker[..read]).trim().to_owned();
    let code = match marker.as_str() {
        "ok" => {
            std::fs::write(&output, FAKE_OUTPUT).unwrap();
            println!("fake-decoded");
            eprintln!("fake-marked");
            0
        }
        // Phase 2: structurally valid containers the inspector must parse.
        "png16" => {
            std::fs::write(&output, png_bytes(16, 16, 2, false)).unwrap();
            0
        }
        "png32" => {
            std::fs::write(&output, png_bytes(32, 32, 2, false)).unwrap();
            0
        }
        "png-srgb" => {
            std::fs::write(&output, png_bytes(16, 16, 6, true)).unwrap();
            0
        }
        "bmp" => {
            std::fs::write(&output, bmp_bytes(8, 4, 24)).unwrap();
            0
        }
        "jpeg" => {
            std::fs::write(&output, jpeg_bytes(24, 12, 3)).unwrap();
            0
        }
        // B1: writes only when the harness gave it its own (non-shared) name.
        "unless-silent" => {
            if !output.to_string_lossy().contains("silent") {
                std::fs::write(&output, png_bytes(16, 16, 2, false)).unwrap();
            }
            0
        }
        // B2/A2/N1: containers whose headers are present but not trustworthy.
        "png-header-only" => {
            std::fs::write(&output, png_header_only(16, 16)).unwrap();
            0
        }
        "png-illegal" => {
            std::fs::write(&output, png_bytes_with(16, 16, 2, 4, false)).unwrap();
            0
        }
        "png-bad-iend" => {
            std::fs::write(&output, png_with_iend_length(16, 16, 5)).unwrap();
            0
        }
        "png-dup-ihdr" => {
            std::fs::write(&output, png_duplicate_ihdr(16, 16)).unwrap();
            0
        }
        "jpeg-sof-only" => {
            std::fs::write(&output, jpeg_sof_only(24, 12, 3)).unwrap();
            0
        }
        "bmp-header-only" => {
            std::fs::write(&output, bmp_header_only(8, 4, 24)).unwrap();
            0
        }
        "bmp-paletted" => {
            std::fs::write(&output, bmp_bytes(8, 4, 8)).unwrap();
            0
        }
        // B-02: exit 0 with no output, and exit 0 with a zero-byte output.
        "nooutput" => 0,
        "emptyoutput" => {
            std::fs::write(&output, b"").unwrap();
            0
        }
        // B-03: a burst that finishes before a poll can observe it, and an output
        // file above the operator bound.
        "burst" => {
            let chunk = [b'B'; 2048];
            let _ = std::io::stdout().write_all(&chunk);
            0
        }
        "bigoutput" => {
            std::fs::write(&output, vec![b'O'; 8192]).unwrap();
            0
        }
        "oversize" => 4,
        "corrupt" => 3,
        "unsupported" => 2,
        "slow" => {
            std::thread::sleep(std::time::Duration::from_millis(4_000));
            0
        }
        // A2: leave a descendant holding the inherited capture handles, then exit.
        "orphan" => {
            std::fs::write(&output, FAKE_OUTPUT).unwrap();
            let _ = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", SLEEPER, "--nocapture", "--quiet"])
                .env(SLEEP_ENV, "1")
                .spawn();
            0
        }
        // NB-1: a whitelisted and an unknown reason token on stdout.
        "reason-known" => {
            println!("{{\"reason_code\":\"win_bad_header\"}}");
            3
        }
        "reason-unknown" => {
            println!("{{\"reason_code\":\"totally_made_up\"}}");
            3
        }
        // A6/read cap: below the scratch bound, so the case completes normally.
        "flood" => {
            std::fs::write(&output, FAKE_OUTPUT).unwrap();
            let chunk = [b'x'; 250];
            for _ in 0..6 {
                let _ = std::io::stdout().write_all(&chunk);
            }
            0
        }
        // NB-2: keep writing until the harness kills the child.
        "floodcap" => {
            let chunk = [b'x'; 1024];
            for _ in 0..FLOOD_LIMIT_CHUNKS {
                if std::io::stdout().write_all(&chunk).is_err() {
                    break;
                }
            }
            0
        }
        _ => 1,
    };
    std::process::exit(code);
}

fn normalize_wall_ms(json: &str) -> String {
    const NEEDLE: &str = "\"wall_ms\": ";
    let mut normalized = String::with_capacity(json.len());
    let mut rest = json;
    while let Some(position) = rest.find(NEEDLE) {
        normalized.push_str(&rest[..position + NEEDLE.len()]);
        rest = &rest[position + NEEDLE.len()..];
        let digits = rest
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(rest.len());
        if digits > 0 {
            normalized.push('0');
        }
        rest = &rest[digits..];
    }
    normalized.push_str(rest);
    normalized
}

fn json_values(json: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\": \"");
    let mut values = Vec::new();
    let mut rest = json;
    while let Some(position) = rest.find(&needle) {
        rest = &rest[position + needle.len()..];
        let end = rest.find('"').unwrap_or(rest.len());
        values.push(rest[..end].to_owned());
        rest = &rest[end..];
    }
    values
}

/// CRC-32 as PNG defines it, so the crafted fixtures are structurally valid.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn png_chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(kind);
    bytes.extend_from_slice(payload);
    let mut crc_input = Vec::from(*kind);
    crc_input.extend_from_slice(payload);
    bytes.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    bytes
}

/// Minimal but structurally valid PNG: signature, IHDR, optional sRGB, IDAT, IEND.
fn png_bytes(width: u32, height: u32, colour_type: u8, with_srgb: bool) -> Vec<u8> {
    png_bytes_with(width, height, colour_type, 8, with_srgb)
}

fn png_signature() -> Vec<u8> {
    vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
}

fn png_ihdr(width: u32, height: u32, colour_type: u8, bit_depth: u8) -> Vec<u8> {
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[bit_depth, colour_type, 0, 0, 0]);
    png_chunk(b"IHDR", &ihdr)
}

fn png_bytes_with(
    width: u32,
    height: u32,
    colour_type: u8,
    bit_depth: u8,
    with_srgb: bool,
) -> Vec<u8> {
    let mut bytes = png_signature();
    bytes.extend_from_slice(&png_ihdr(width, height, colour_type, bit_depth));
    if with_srgb {
        bytes.extend_from_slice(&png_chunk(b"sRGB", &[0]));
    }
    bytes.extend_from_slice(&png_chunk(
        b"IDAT",
        &[0x78, 0x9C, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
    ));
    bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
    bytes
}

/// A 33-byte PNG: signature and IHDR only, no IDAT and no IEND (B2).
fn png_header_only(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = png_signature();
    bytes.extend_from_slice(&png_ihdr(width, height, 2, 8));
    bytes
}

/// A PNG whose IEND states a non-zero length, so it does not terminate the image (N1).
fn png_with_iend_length(width: u32, height: u32, iend_length: u32) -> Vec<u8> {
    let mut bytes = png_signature();
    bytes.extend_from_slice(&png_ihdr(width, height, 2, 8));
    let mut chunk = Vec::new();
    chunk.extend_from_slice(&iend_length.to_be_bytes());
    chunk.extend_from_slice(b"IEND");
    chunk.extend_from_slice(&[0_u8; 4]);
    bytes.extend_from_slice(&chunk);
    bytes
}

/// A PNG with two IHDR chunks, which the format forbids (N1).
fn png_duplicate_ihdr(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = png_signature();
    bytes.extend_from_slice(&png_ihdr(width, height, 2, 8));
    bytes.extend_from_slice(&png_ihdr(width, height, 2, 8));
    bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
    bytes
}

/// Minimal structurally valid BMP with a 40-byte BITMAPINFOHEADER.
fn bmp_bytes(width: i32, height: i32, bit_count: u16) -> Vec<u8> {
    let header_size = 40_u32;
    let data_offset = 14 + header_size;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"BM");
    bytes.extend_from_slice(&(data_offset + 16).to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&data_offset.to_le_bytes());
    bytes.extend_from_slice(&header_size.to_le_bytes());
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&height.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&bit_count.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&2835_i32.to_le_bytes());
    bytes.extend_from_slice(&2835_i32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&[0_u8; 16]);
    bytes
}

/// A JPEG that stops after its SOF segment: no EOI, no scan (B2).
fn jpeg_sof_only(width: u16, height: u16, components: u8) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.push(8);
    payload.extend_from_slice(&height.to_be_bytes());
    payload.extend_from_slice(&width.to_be_bytes());
    payload.push(components);
    for index in 0..components {
        payload.extend_from_slice(&[index + 1, 0x11, 0]);
    }
    let mut bytes = vec![0xFF, 0xD8];
    bytes.extend_from_slice(&[0xFF, 0xC0]);
    bytes.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
    bytes.extend_from_slice(&payload);
    bytes
}

/// A 34-byte BMP whose header claims 40 bytes and whose data offset is 54 (B2).
fn bmp_header_only(width: i32, height: i32, bit_count: u16) -> Vec<u8> {
    let mut bytes = bmp_bytes(width, height, bit_count);
    bytes.truncate(34);
    bytes
}

/// Minimal structurally valid JPEG: SOI, SOF0 with the given geometry, EOI.
fn jpeg_bytes(width: u16, height: u16, components: u8) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.push(8);
    payload.extend_from_slice(&height.to_be_bytes());
    payload.extend_from_slice(&width.to_be_bytes());
    payload.push(components);
    for index in 0..components {
        payload.extend_from_slice(&[index + 1, 0x11, 0]);
    }
    let mut bytes = vec![0xFF, 0xD8];
    bytes.extend_from_slice(&[0xFF, 0xC0]);
    bytes.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
    bytes.extend_from_slice(&payload);
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}
