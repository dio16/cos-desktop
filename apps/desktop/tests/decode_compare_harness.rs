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
#[path = "../examples/decode_compare/pixels.rs"]
mod pixels;

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
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// One manifest row: case name, optional `expected:WxH`, declared outcome class.
type Declaration<'a> = (&'a str, Option<(u32, u32)>, OutcomeClass);

/// One manifest row with an optional `pixels:<sha256>` expectation.
type PixelDeclaration<'a> = (&'a str, Option<(u32, u32)>, Option<[u8; 32]>, OutcomeClass);

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

    /// Write a manifest with optional `expected:WxH` and `pixels:<hex>` per case.
    fn declare_pixels(&self, declarations: &[PixelDeclaration<'_>]) {
        let mut text =
            String::from("# case file name[|expected:WxH][|pixels:<sha256>][|declared-class]\n");
        for (name, expected, digest, class) in declarations {
            text.push_str(name);
            if let Some((width, height)) = expected {
                text.push_str(&format!("|expected:{width}x{height}"));
            }
            if let Some(digest) = digest {
                text.push_str(&format!("|pixels:{}", pixels::hex(digest)));
            }
            text.push_str(&format!("|{}\n", class.as_str()));
        }
        std::fs::write(self.cases.join(harness::MANIFEST_FILE), text).unwrap();
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

/// Run the harness in an explicitly selected output-container policy.
fn measured_in(
    fixture: &Fixture,
    candidates: &[Candidate],
    helper: &str,
    policy: harness::OutputPolicy,
) -> Report {
    let mut config = harness::parse(arguments(fixture, candidates)).unwrap();
    config.invocation = helper_invocation(helper);
    config.output_policy = policy;
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
    assert!(observation.scratch_max_bytes.unwrap() > 0);
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
    assert_eq!(observation.scratch_max_bytes, None);
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

/// BLOCKING 1: `--output-policy` selects the output-container mode explicitly, defaults to
/// `native`, accepts its documented spellings case-insensitively, and refuses an unknown
/// token instead of silently guessing a mode.
#[test]
fn output_policy_flag_is_explicit_and_refuses_unknown_modes() {
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");
    let candidate = fixture.fake();

    let mut args = arguments(&fixture, std::slice::from_ref(&candidate));
    args.push(OsString::from("--output-policy"));
    args.push(OsString::from("canonical-png"));
    let config = harness::parse(args).unwrap();
    assert_eq!(config.output_policy, harness::OutputPolicy::CanonicalPng);
    assert_eq!(config.output_policy.as_str(), "canonical-png");

    let mut args = arguments(&fixture, std::slice::from_ref(&candidate));
    args.push(OsString::from("--output-policy"));
    args.push(OsString::from("Native"));
    assert_eq!(
        harness::parse(args).unwrap().output_policy,
        harness::OutputPolicy::Native
    );

    let mut args = arguments(&fixture, std::slice::from_ref(&candidate));
    args.push(OsString::from("--output-policy"));
    args.push(OsString::from("png"));
    let error = harness::parse(args).unwrap_err();
    let message = format!("{error}");
    assert!(
        message.contains("canonical-png"),
        "the refusal must name the accepted modes: {message}"
    );
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
            None,
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
            Some(1_048_576),
            None
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
    // Budget mode forwards all five resolved limits and never the legacy single flag.
    let limits = media::DecodeLimits {
        max_dimension: 512,
        max_pixels: 262_144,
        max_bit_depth: 8,
        max_decoded_bytes: 1_048_576,
        max_scratch_bytes: 1_048_576,
    };
    assert_eq!(
        as_strings(&harness::measurement_argv(
            &plain,
            &scripted,
            &input,
            &output,
            None,
            Some(limits)
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
            "262144",
            "-MaxDimension",
            "512",
            "-MaxBitDepth",
            "8",
            "-MaxDecodedBytes",
            "1048576",
            "-MaxScratchBytes",
            "1048576",
        ]
    );
    assert_eq!(
        as_strings(&harness::measurement_argv(
            &plain, &scripted, &input, &output, None, None
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
            None,
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

/// BLOCKING 1: the output-container policy is explicit and the **default is `native`**.
/// `native` keeps the historical suffix-derived executable name (this is the tamper
/// assertion: it fails if the default silently reverts to `.png`), while the explicit
/// `canonical-png` mode asks every leg for the same lossless PNG container. The script
/// transport is already PNG in both modes, and the candidate id stays part of every name
/// (B1), so two candidates can never share a path.
#[test]
fn output_policy_defaults_to_native_and_canonical_mode_is_explicit() {
    let work = PathBuf::from(r"C:\work");
    let scripted = Program::PwshScript(PathBuf::from(r"C:\tools\wic_probe.ps1"));
    let executable = Program::Executable(PathBuf::from(r"C:\tools\ffmpeg.exe"));

    // Default policy, from the CLI: no --output-policy means native.
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");
    let config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    assert_eq!(config.output_policy, harness::OutputPolicy::Native);
    assert_eq!(config.output_policy.as_str(), "native");

    for suffix in ["jpg", "jpeg", "tif", "tiff", "gif", "png", "bmp"] {
        let case_id = format!("photo.{suffix}");
        // Native (default): the executable output keeps the case suffix, exactly as before.
        let native = harness::output_path(
            &executable,
            &work,
            3,
            &case_id,
            "ffmpeg8",
            suffix,
            harness::OutputPolicy::Native,
        );
        assert_eq!(
            native.file_name().and_then(|name| name.to_str()),
            Some(format!("003_photo.{suffix}.ffmpeg8.{suffix}").as_str()),
            "the native default must keep the suffix-derived output name"
        );
        // Canonical comparison mode: one lossless PNG container for every leg.
        let canonical = harness::output_path(
            &executable,
            &work,
            3,
            &case_id,
            "ffmpeg8",
            suffix,
            harness::OutputPolicy::CanonicalPng,
        );
        assert_eq!(
            canonical.file_name().and_then(|name| name.to_str()),
            Some(format!("003_photo.{suffix}.ffmpeg8.png").as_str()),
            "canonical-png mode must ask every executable for a PNG"
        );
        // The script transport is already PNG in both modes.
        for policy in [
            harness::OutputPolicy::Native,
            harness::OutputPolicy::CanonicalPng,
        ] {
            let scripted_path =
                harness::output_path(&scripted, &work, 3, &case_id, "wic", suffix, policy);
            assert_eq!(
                scripted_path.file_name().and_then(|name| name.to_str()),
                Some(format!("003_photo.{suffix}.wic.wic.png").as_str()),
                "the script transport is PNG in every mode"
            );
        }
    }

    // The tamper assertion: one exact native name, so a silent switch to `.png` fails here.
    assert_eq!(
        harness::output_path(
            &executable,
            &work,
            3,
            "photo.jpg",
            "ffmpeg8",
            "jpg",
            harness::OutputPolicy::Native
        ),
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

#[test]
fn admission_refusal_token_is_whitelisted_and_classified_oversize() {
    let fixture = Fixture::new();
    fixture.case("a-refused.png", "reason-budget");
    let report = measured(&fixture, &[fixture.fake()], HELPER);
    let observation = &report.observations[0];
    assert_eq!(observation.candidate_reason, "max_dimension_exceeded");
    assert_eq!(observation.observed_outcome, OutcomeClass::Oversize);
    // A refusal contributes no capability or resource evidence.
    assert!(report.capabilities.is_empty());
    let json = harness::render(&report).0;
    assert!(json.contains("\"candidate_reason\": \"max_dimension_exceeded\""));
    assert!(json.contains("\"decode_budget\": null"));
}

/// W1: budget mode is opt-in, resolves from the ratified bundle, forwards all five
/// limits and records them; the legacy `--max-pixels` run keeps `decode_budget: null`
/// and a byte-identical argv.
#[test]
fn decode_budget_is_opt_in_recorded_and_rejects_unusable_values() {
    let fixture = Fixture::new();
    let base = vec![
        OsString::from("--cases"),
        fixture.cases.clone().into_os_string(),
        OsString::from("--out"),
        fixture.record("record").into_os_string(),
        OsString::from("--candidate"),
        OsString::from(r"wic=pwsh:C:\tools\wic_probe.ps1"),
    ];
    let legacy = harness::parse(
        base.iter()
            .cloned()
            .chain([OsString::from("--max-pixels"), OsString::from("4096")]),
    )
    .unwrap();
    assert_eq!(legacy.max_pixels, Some(4096));
    assert!(legacy.decode_limits.is_none());

    let mut budget = base.clone();
    budget.extend([
        OsString::from("--max-dimension"),
        OsString::from("512"),
        OsString::from("--max-bit-depth"),
        OsString::from("8"),
        OsString::from("--max-decoded-bytes"),
        OsString::from("1048576"),
        OsString::from("--max-scratch-bytes"),
        OsString::from("2097152"),
    ]);
    let config = harness::parse(budget).unwrap();
    let limits = config.decode_limits.expect("budget resolved");
    assert_eq!(limits.max_dimension, 512);
    // Untouched fields keep the ratified declared value, never a probe-local literal.
    assert_eq!(limits.max_pixels, media::DecodeBudget::MAX_PIXELS);
    assert_eq!(limits.max_bit_depth, 8);
    assert_eq!(limits.max_decoded_bytes, 1_048_576);
    assert_eq!(limits.max_scratch_bytes, 2_097_152);

    // A zero override is unusable and refused instead of silently clamped.
    let mut zero = base.clone();
    zero.extend([
        OsString::from("--max-dimension"),
        OsString::from("0"),
        OsString::from("--max-bit-depth"),
        OsString::from("8"),
        OsString::from("--max-decoded-bytes"),
        OsString::from("1048576"),
        OsString::from("--max-scratch-bytes"),
        OsString::from("2097152"),
    ]);
    let error = harness::parse(zero).expect_err("zero dimension is unusable");
    assert!(format!("{error:?}").contains("nothing is clamped"));

    // The resolved bundle is visible in the record header, with its declared basis.
    fixture.case("a.png", "ok");
    let mut report = measured(&fixture, &[fixture.fake()], HELPER);
    report.decode_limits = Some(limits);
    let json = harness::render(&report).0;
    assert!(json.contains("\"decode_budget\": {"));
    assert!(json.contains("\"declared_basis\": \"RATIFIED_DECLARED_BASIS_NOT_MEASURED\""));
    assert!(json.contains("\"max_dimension\": 512"));
    assert!(json.contains("\"max_scratch_bytes\": 2097152"));
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

/// NB-2: exceeding the observed scratch threshold cancels a still-running child,
/// independently of how quickly it writes before the supervisor gets scheduled.
#[test]
fn scratch_cap_kills_a_flooding_candidate() {
    let fixture = Fixture::new();
    fixture.case("flood.png", "floodcap");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.max_output_bytes = 64;
    // Sampling spawns a tool per interval and would slow the parent enough to change
    // how much the child writes before the cap is noticed; the cap still applies.
    config.sample_interval_ms = 0;
    config.invocation = helper_invocation(HELPER);
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert_eq!(observation.observed_outcome, OutcomeClass::Cancelled);
    assert_eq!(observation.refusal, Some("scratch_capped"));
    assert!(!observation.timeout_fired);
    assert_eq!(observation.exit_code, None);
    let scratch = observation.scratch_max_bytes.unwrap();
    assert!(
        scratch > 64 * harness::SCRATCH_FACTOR,
        "the observed scratch size must exceed the bound ({scratch})"
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

/// Schema 10: a declared raster that cannot be checked because the leg produced no output
/// must say so — the row is `not_measured` with `pixel_reason: "no_output"`, not a silent
/// absence of a digest. This covers both a silent success (exit 0, no file) and a leg that
/// rejected the input.
#[test]
fn a_declared_raster_with_no_output_is_explicitly_not_measured() {
    let fixture = Fixture::new();
    fixture.case("a.png", "nooutput");
    let digest = pixels::sha256(&rgba_rgb_16(10));
    fixture.declare_pixels(&[("a.png", Some((16, 16)), Some(digest), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);
    let observation = &report.observations[0];
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("output_missing"));
    assert_eq!(observation.pixel_digest, None);
    assert_eq!(observation.pixel_reason, Some("no_output"));
    assert!(!observation.pixel_verified);
    assert_eq!(observation.validity, "not_measured");

    // A leg that rejects the input (non-zero exit, no output) keeps its own class and
    // exit code, and the declared raster is still explained as `no_output`.
    fixture.case("a.png", "corrupt");
    let report = measured(&fixture, &[fixture.fake()], HELPER);
    let observation = &report.observations[0];
    assert_eq!(observation.observed_outcome, OutcomeClass::Corrupt);
    assert_eq!(observation.exit_code, Some(3));
    assert_eq!(observation.pixel_reason, Some("no_output"));
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
    let scratch = observation.scratch_max_bytes.unwrap();
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
fn structure_verified_when_the_output_matches_the_expectation() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png16");
    fixture.declare_expected(&[("a.png", Some((16, 16)), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.observed_outcome, OutcomeClass::Decoded);
    assert_eq!(observation.validity, "structure_verified");
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
    let first = harness::output_path(
        &scripted,
        &work,
        0,
        "a.png",
        "wic",
        "png",
        harness::OutputPolicy::Native,
    );
    let second = harness::output_path(
        &scripted,
        &work,
        0,
        "a.png",
        "wic-alt",
        "png",
        harness::OutputPolicy::Native,
    );
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
    assert_eq!(writer_row.validity, "structure_verified");

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
    assert_ne!(silent_row.validity, "structure_verified");

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
    assert_eq!(by_case("a.png").validity, "structure_verified");
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
    config.invocation = helper_invocation(HELPER);
    let mut full = harness::run(&config).unwrap();
    assert_eq!(full.observations.len(), 3);
    // Calibrate inside the concrete summary-header overhead for two rows, rather
    // than assuming any checkout path length or fixed serialized metadata size.
    full.observations.truncate(2);
    let with_summaries = harness::render(&full).0.len();
    full.capabilities.clear();
    full.resources.clear();
    let without_summaries = harness::render(&full).0.len();
    config.max_record_bytes = (with_summaries + without_summaries) / 2;
    let report = harness::run(&config).unwrap();

    assert!(!report.observations.is_empty());
    assert!(
        report.observations.len() < 3,
        "the record must be truncated"
    );
    assert_eq!(
        harness::render(&report).1,
        report.observations.len(),
        "the saved evidence population must match the returned report"
    );
    harness::write_report(&report, &fixture.record("bounded")).unwrap();
    for observation in &report.observations {
        assert_eq!(observation.observed_outcome, OutcomeClass::Decoded);
        assert!(observation.inspected_complete);
        assert_eq!(observation.inspected_format, inspect::Format::Png);
        assert_eq!(observation.inspected_width, Some(16));
        assert_eq!(observation.inspected_height, Some(16));
    }
    assert_eq!(report.capabilities.len(), 1);
    assert_eq!(report.capabilities[0].format, inspect::Format::Png);
    assert_eq!(report.capabilities[0].max_width, Some(16));
    assert_eq!(report.capabilities[0].max_height, Some(16));
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

/// B-R1-01 positive controls: the hand-built fixtures really are structurally
/// complete, so the adversarial fixtures below differ only in the rule they break.
#[test]
fn structurally_complete_fixtures_are_verified() {
    let mut png = std::io::Cursor::new(png_bytes(16, 16, 2, false));
    let inspected = inspect::inspect(&mut png);
    assert!(inspected.complete, "IHDR + IDAT + IEND is complete");
    assert_eq!(inspected.dimensions(), Some((16, 16)));

    let mut jpeg = std::io::Cursor::new(jpeg_bytes(24, 12, 3));
    let inspected = inspect::inspect(&mut jpeg);
    assert!(inspected.complete, "SOF + SOS + EOI is complete");
    assert_eq!(inspected.dimensions(), Some((24, 12)));

    let mut bmp = std::io::Cursor::new(bmp_bytes(8, 4, 24));
    let inspected = inspect::inspect(&mut bmp);
    assert!(
        inspected.complete,
        "the declared pixel buffer fits the file"
    );
    assert_eq!(inspected.dimensions(), Some((8, 4)));
}

/// B-R1-01: a PNG with IHDR and IEND but no image data is never verified.
#[test]
fn png_without_image_data_is_incomplete() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png-no-idat");
    fixture.declare_expected(&[("a.png", Some((16, 16)), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.exit_code, Some(0));
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("output_incomplete"));
    assert_eq!(observation.validity, "not_measured");
    assert_eq!(observation.validity_reason, Some("output_incomplete"));
    assert_eq!(observation.inspected_format, inspect::Format::Png);
    assert_eq!(observation.inspected_width, Some(16));
    assert!(!observation.inspected_complete);
    assert!(report.capabilities.is_empty());
}

/// B-R1-01: a JPEG that reaches its scan but never terminates it is never verified.
#[test]
fn jpeg_without_terminator_is_incomplete() {
    let fixture = Fixture::new();
    fixture.case("a.jpg", "jpeg-no-eoi");
    fixture.declare_expected(&[("a.jpg", Some((24, 12)), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.exit_code, Some(0));
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("output_incomplete"));
    assert_eq!(observation.validity, "not_measured");
    assert_eq!(observation.inspected_format, inspect::Format::Jpeg);
    assert_eq!(observation.inspected_width, Some(24));
    assert!(!observation.inspected_complete);
    assert!(report.capabilities.is_empty());
}

/// B-R1-01: a BMP whose declared pixel buffer does not fit the file is never
/// verified.
#[test]
fn bmp_with_short_pixel_data_is_incomplete() {
    let fixture = Fixture::new();
    fixture.case("a.bmp", "bmp-short-pixels");
    fixture.declare_expected(&[("a.bmp", Some((8, 4)), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.exit_code, Some(0));
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("output_incomplete"));
    assert_eq!(observation.validity, "not_measured");
    assert_eq!(observation.inspected_format, inspect::Format::Bmp);
    assert_eq!(observation.inspected_width, Some(8));
    assert!(!observation.inspected_complete);
    assert!(report.capabilities.is_empty());
}

/// B-R1-01 residual: a PNG whose terminator chunk has no CRC bytes is not complete,
/// even though the IHDR and a non-empty IDAT are present.
#[test]
fn png_iend_without_crc_is_incomplete() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png-iend-no-crc");
    fixture.declare_expected(&[("a.png", Some((16, 16)), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.exit_code, Some(0));
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("output_incomplete"));
    assert_eq!(observation.validity, "not_measured");
    assert_eq!(observation.inspected_format, inspect::Format::Png);
    assert_eq!(observation.inspected_width, Some(16));
    assert!(!observation.inspected_complete);
    assert!(report.capabilities.is_empty());
}

/// B-R1-01 residual: `FF DA FF D9` has no SOS length field, so it never counts as a
/// scan start and the file is not complete.
#[test]
fn jpeg_sos_without_length_is_incomplete() {
    let fixture = Fixture::new();
    fixture.case("a.jpg", "jpeg-sos-nolen");
    fixture.declare_expected(&[("a.jpg", Some((24, 12)), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.exit_code, Some(0));
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("output_incomplete"));
    assert_eq!(observation.validity, "not_measured");
    assert_eq!(observation.inspected_format, inspect::Format::Jpeg);
    assert!(!observation.inspected_complete);
    assert!(report.capabilities.is_empty());
}

/// B-R2-01: `FF DA 00 03 01 FF D9` declares a 3-byte scan header, so it can never
/// start a scan and the file is never complete, even after a valid SOF.
#[test]
fn jpeg_sos_with_short_length_is_incomplete() {
    let fixture = Fixture::new();
    fixture.case("a.jpg", "jpeg-sos-short");
    fixture.declare_expected(&[("a.jpg", Some((24, 12)), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.exit_code, Some(0));
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("output_incomplete"));
    assert_eq!(observation.validity, "not_measured");
    assert_eq!(observation.inspected_format, inspect::Format::Jpeg);
    assert!(!observation.inspected_complete);
    assert!(report.capabilities.is_empty());
}

/// B-R3-01 hole 1: the only `FF D9` in the file sits inside the SOS header, so it is
/// not an `EOI` and the image is never complete.
#[test]
fn jpeg_eoi_inside_the_scan_header_is_incomplete() {
    let fixture = Fixture::new();
    fixture.case("a.jpg", "jpeg-eoi-in-sos");
    fixture.declare_expected(&[("a.jpg", Some((16, 16)), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.exit_code, Some(0));
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("output_incomplete"));
    assert_eq!(observation.validity, "not_measured");
    assert_eq!(observation.inspected_format, inspect::Format::Jpeg);
    assert_eq!(observation.inspected_width, Some(16));
    assert!(!observation.inspected_complete);
    assert!(report.capabilities.is_empty());
}

/// B-R3-01 hole 2: a SOF that declares components without their descriptors is not a
/// full frame header, so a following well-formed scan cannot complete the file.
#[test]
fn jpeg_sof_without_component_descriptors_is_incomplete() {
    let fixture = Fixture::new();
    fixture.case("a.jpg", "jpeg-sof-nodesc");
    fixture.declare_expected(&[("a.jpg", Some((16, 16)), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.exit_code, Some(0));
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("output_incomplete"));
    assert_eq!(observation.validity, "not_measured");
    assert_eq!(observation.inspected_format, inspect::Format::Jpeg);
    assert_eq!(observation.inspected_width, Some(16));
    assert!(!observation.inspected_complete);
    assert!(report.capabilities.is_empty());
}

/// Phase 3: SHA-256 known-answer vectors (FIPS 180-4), independent of the harness.
#[test]
fn sha256_matches_the_fips_vectors() {
    assert_eq!(
        pixels::hex(&pixels::sha256(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        pixels::hex(&pixels::sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        pixels::hex(&pixels::sha256(
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
        )),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
}

/// Phase 3: the oracle decodes every supported colour type to the expected RGBA.
#[test]
fn png_oracle_decodes_all_supported_colour_types() {
    // Gray: two pixels 7 and 9.
    let gray = png_with_pixels(2, 1, 0, &[vec![7, 9]]);
    let image = pixels::read_png(&mut std::io::Cursor::new(gray)).unwrap();
    assert_eq!((image.width, image.height), (2, 1));
    assert_eq!(image.pixels, vec![7, 7, 7, 255, 9, 9, 9, 255]);

    // Gray + alpha.
    let gray_alpha = png_with_pixels(1, 1, 4, &[vec![7, 200]]);
    let image = pixels::read_png(&mut std::io::Cursor::new(gray_alpha)).unwrap();
    assert_eq!(image.pixels, vec![7, 7, 7, 200]);

    // RGB.
    let rgb = png_with_pixels(1, 1, 2, &[vec![1, 2, 3]]);
    let image = pixels::read_png(&mut std::io::Cursor::new(rgb)).unwrap();
    assert_eq!(image.pixels, vec![1, 2, 3, 255]);

    // RGBA, and the full 16x16 fixture used by the harness tests.
    let rgba = png_with_pixels(1, 1, 6, &[vec![1, 2, 3, 4]]);
    let image = pixels::read_png(&mut std::io::Cursor::new(rgba)).unwrap();
    assert_eq!(image.pixels, vec![1, 2, 3, 4]);

    let image = pixels::read_png(&mut std::io::Cursor::new(png_rgb_16(10))).unwrap();
    assert_eq!(image.pixels, rgba_rgb_16(10));
    assert_eq!(image.digest(), pixels::sha256(&rgba_rgb_16(10)));
}

/// Phase 3: every scanline filter (0-4) round-trips through the oracle. The encoder
/// below is an independent implementation of the PNG filter definitions.
#[test]
fn png_oracle_reverses_every_scanline_filter() {
    let previous = [1_u8, 2, 3, 4, 5, 6];
    let row = [10_u8, 20, 30, 40, 50, 60];
    for filter in 0..=4_u8 {
        let mut encoded = Vec::new();
        encoded.push(filter);
        for index in 0..row.len() {
            let left = if index >= 3 { row[index - 3] } else { 0 };
            let up = previous[index];
            let corner = if index >= 3 { previous[index - 3] } else { 0 };
            let predict = match filter {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                _ => png_paeth(left, up, corner),
            };
            encoded.push(row[index].wrapping_sub(predict));
        }
        let mut raw = Vec::new();
        raw.push(0);
        raw.extend_from_slice(&previous);
        raw.extend_from_slice(&encoded);

        let mut bytes = png_signature();
        bytes.extend_from_slice(&png_ihdr(2, 2, 2, 8));
        bytes.extend_from_slice(&png_chunk(b"IDAT", &zlib_stored(&raw)));
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
        let image = pixels::read_png(&mut std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(
            image.pixels,
            vec![1, 2, 3, 255, 4, 5, 6, 255, 10, 20, 30, 255, 40, 50, 60, 255],
            "filter {filter} must round-trip"
        );
    }
}

/// The PNG Paeth predictor, written independently of the oracle.
fn png_paeth(left: u8, up: u8, corner: u8) -> u8 {
    let estimate = i32::from(left) + i32::from(up) - i32::from(corner);
    let to_left = (estimate - i32::from(left)).abs();
    let to_up = (estimate - i32::from(up)).abs();
    let to_corner = (estimate - i32::from(corner)).abs();
    if to_left <= to_up && to_left <= to_corner {
        left
    } else if to_up <= to_corner {
        up
    } else {
        corner
    }
}

/// Phase 3: the in-repo inflate handles stored, fixed and dynamic Huffman blocks,
/// and the dynamic one is a real stream produced by an independent compressor.
#[test]
fn inflate_handles_stored_fixed_and_dynamic_blocks() {
    let raw = b"hello hello hello";
    assert_eq!(pixels::inflate(&zlib_stored(raw), raw.len()).unwrap(), raw);

    // Fixed Huffman: zlib stream from an independent compressor (BTYPE=1).
    let fixed: Vec<u8> = vec![
        0x78, 0xda, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc8, 0x40, 0x27, 0x01, 0x68, 0x03, 0x08,
        0xb1,
    ];
    assert_eq!(
        pixels::inflate(&fixed, 23).unwrap(),
        b"hello hello hello hello"
    );

    // Dynamic Huffman (BTYPE=2), 111-byte zlib stream for 200 bytes of text.
    let stream: Vec<u8> = vec![
        0x78, 0xda, 0x0d, 0x8d, 0xc9, 0x01, 0x00, 0x30, 0x10, 0x01, 0x6b, 0x75, 0xec, 0x21, 0xfd,
        0x17, 0x90, 0xf5, 0x64, 0x90, 0x24, 0x88, 0xb6, 0xf0, 0x5a, 0x78, 0x2f, 0xaa, 0x5e, 0x4f,
        0x94, 0x85, 0xdd, 0x5e, 0xf0, 0xbd, 0x4e, 0x6f, 0x85, 0x21, 0x4a, 0x47, 0x07, 0x9e, 0x67,
        0x1d, 0xe3, 0xda, 0xb7, 0x80, 0xe9, 0x0e, 0xb7, 0xa6, 0xae, 0x8c, 0x94, 0xf9, 0xda, 0x03,
        0x8c, 0xa4, 0x81, 0xce, 0x23, 0x04, 0xcd, 0xb6, 0x4c, 0xee, 0x09, 0x8f, 0xd3, 0x36, 0xd3,
        0x00, 0xee, 0x78, 0x2a, 0x76, 0x5e, 0x63, 0xe9, 0xdb, 0x45, 0xcf, 0x75, 0x79, 0x9f, 0x25,
        0x46, 0x42, 0x76, 0xd6, 0xbe, 0x19, 0x8a, 0x47, 0x5f, 0xae, 0xc9, 0x72, 0xdd, 0x45, 0xd5,
        0xfa, 0x7d, 0x1b, 0xf4, 0x4f, 0x03,
    ];
    let expected: Vec<u8> = vec![
        0x69, 0x69, 0x69, 0x69, 0x61, 0x69, 0x63, 0x68, 0x65, 0x61, 0x6a, 0x66, 0x63, 0x61, 0x6a,
        0x6a, 0x69, 0x63, 0x65, 0x66, 0x68, 0x64, 0x67, 0x69, 0x63, 0x69, 0x68, 0x61, 0x64, 0x64,
        0x66, 0x64, 0x68, 0x61, 0x62, 0x6a, 0x6a, 0x66, 0x69, 0x66, 0x68, 0x65, 0x69, 0x62, 0x69,
        0x62, 0x61, 0x65, 0x63, 0x69, 0x61, 0x69, 0x69, 0x61, 0x64, 0x67, 0x6a, 0x64, 0x63, 0x69,
        0x63, 0x69, 0x64, 0x65, 0x68, 0x6a, 0x68, 0x61, 0x61, 0x64, 0x62, 0x64, 0x66, 0x69, 0x62,
        0x68, 0x65, 0x67, 0x65, 0x65, 0x66, 0x68, 0x61, 0x69, 0x65, 0x64, 0x62, 0x6a, 0x66, 0x64,
        0x67, 0x61, 0x61, 0x67, 0x63, 0x63, 0x63, 0x67, 0x61, 0x63, 0x66, 0x68, 0x61, 0x62, 0x61,
        0x63, 0x61, 0x63, 0x67, 0x68, 0x66, 0x63, 0x64, 0x62, 0x62, 0x68, 0x68, 0x68, 0x68, 0x61,
        0x6a, 0x62, 0x67, 0x66, 0x64, 0x64, 0x62, 0x69, 0x66, 0x61, 0x61, 0x61, 0x66, 0x63, 0x61,
        0x67, 0x65, 0x69, 0x64, 0x64, 0x69, 0x6a, 0x66, 0x61, 0x68, 0x62, 0x64, 0x69, 0x63, 0x69,
        0x61, 0x66, 0x67, 0x67, 0x63, 0x63, 0x62, 0x65, 0x69, 0x62, 0x65, 0x63, 0x62, 0x69, 0x63,
        0x63, 0x61, 0x69, 0x68, 0x67, 0x68, 0x64, 0x64, 0x61, 0x62, 0x61, 0x62, 0x63, 0x62, 0x63,
        0x61, 0x67, 0x61, 0x66, 0x67, 0x63, 0x67, 0x69, 0x68, 0x62, 0x68, 0x64, 0x66, 0x65, 0x62,
        0x63, 0x65, 0x68, 0x64, 0x6a,
    ];
    assert_eq!(expected.len(), 200);
    assert_eq!(((stream[2] >> 1) & 3), 2, "the fixture must be dynamic");
    assert_eq!(pixels::inflate(&stream, expected.len()).unwrap(), expected);
}

/// Phase 3: malformed streams are refused, never panicked on.
#[test]
fn inflate_rejects_malformed_streams() {
    // Bad zlib header.
    assert_eq!(
        pixels::inflate(&[0x00, 0x00, 0x00], 16),
        Err(pixels::PixelError::InflateCorrupt)
    );
    // Bad FCHECK.
    assert_eq!(
        pixels::inflate(&[0x78, 0x9D], 16),
        Err(pixels::PixelError::InflateCorrupt)
    );
    // Truncated stored block.
    assert_eq!(
        pixels::inflate(&[0x78, 0x01, 0x01, 0x10, 0x00], 16),
        Err(pixels::PixelError::InflateCorrupt)
    );
    // Stored block whose NLEN does not complement LEN.
    assert_eq!(
        pixels::inflate(
            &[
                0x78, 0x01, 0x01, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00
            ],
            16
        ),
        Err(pixels::PixelError::InflateCorrupt)
    );
    // Reserved block type 3.
    assert_eq!(
        pixels::inflate(&[0x78, 0x01, 0x07], 16),
        Err(pixels::PixelError::InflateCorrupt)
    );
    // Preset dictionary is outside the supported subset.
    assert_eq!(
        pixels::inflate(&[0x78, 0x20, 0x00, 0x00], 16),
        Err(pixels::PixelError::UnsupportedSubset)
    );
}

/// Phase 3 (4): a corrupt IDAT after a valid IHDR never verifies, never panics.
#[test]
fn corrupt_idat_never_verifies() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png-corrupt-idat");
    let expected = pixels::sha256(&rgba_rgb_16(10));
    fixture.declare_pixels(&[(
        "a.png",
        Some((16, 16)),
        Some(expected),
        OutcomeClass::Decoded,
    )]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.exit_code, Some(0));
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("pixel_unavailable"));
    assert_eq!(observation.validity, "not_measured");
    assert!(!observation.pixel_verified);
    assert_eq!(
        observation.pixel_digest, None,
        "no digest may be fabricated"
    );
    assert_eq!(observation.pixel_status, harness::PixelStatus::Unavailable);
    assert!(
        observation.pixel_reason.is_some(),
        "the oracle must explain itself"
    );
    assert!(report.capabilities.is_empty());

    // The oracle itself refuses the same bytes.
    let mut bytes = png_signature();
    bytes.extend_from_slice(&png_ihdr(16, 16, 6, 8));
    bytes.extend_from_slice(&png_chunk(b"IDAT", &[0x78, 0x9C, 0xFF, 0x00, 0x13, 0x37]));
    bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
    assert!(pixels::read_png(&mut std::io::Cursor::new(bytes)).is_err());
}

/// Phase 3 (5): a huge claimed dimension is refused by the pixel cap before any
/// allocation happens.
#[test]
fn huge_claimed_dimension_is_refused_by_the_pixel_cap() {
    let mut bytes = png_signature();
    bytes.extend_from_slice(&png_ihdr(40_000, 40_000, 6, 8));
    bytes.extend_from_slice(&png_chunk(b"IDAT", &zlib_stored(&[0_u8; 8])));
    bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
    assert_eq!(
        pixels::read_png(&mut std::io::Cursor::new(bytes)),
        Err(pixels::PixelError::TooLarge),
        "the cap is checked before the RGBA buffer exists"
    );

    let fixture = Fixture::new();
    fixture.case("a.png", "png-huge-dims");
    let expected = pixels::sha256(b"anything");
    fixture.declare_pixels(&[(
        "a.png",
        Some((40_000, 40_000)),
        Some(expected),
        OutcomeClass::Decoded,
    )]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);
    let observation = &report.observations[0];
    assert_eq!(observation.refusal, Some("pixel_unavailable"));
    assert_eq!(observation.pixel_reason, Some("too_large"));
    assert!(!observation.pixel_verified);
    assert!(report.capabilities.is_empty());
}

/// Phase 3 (6): the inflate never produces more than the declared geometry needs.
#[test]
fn inflate_stops_at_the_bounded_output_cap() {
    let bomb = zlib_stored(&vec![0_u8; 200 * 1024]);
    assert_eq!(
        pixels::inflate(&bomb, 1024),
        Err(pixels::PixelError::InflateLimit),
        "a bomb must stop at the cap, not grow to 200 KiB"
    );
    // The same stream is fine at its real size.
    assert_eq!(
        pixels::inflate(&bomb, 200 * 1024).unwrap().len(),
        200 * 1024
    );

    let fixture = Fixture::new();
    fixture.case("a.png", "png-inflate-bomb");
    let expected = pixels::sha256(b"anything");
    fixture.declare_pixels(&[(
        "a.png",
        Some((16, 16)),
        Some(expected),
        OutcomeClass::Decoded,
    )]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);
    let observation = &report.observations[0];
    assert_eq!(observation.refusal, Some("pixel_unavailable"));
    assert_eq!(observation.pixel_reason, Some("inflate_limit"));
    assert!(!observation.pixel_verified);
    assert!(report.capabilities.is_empty());
}

/// Phase 3 (1): identical dimensions, different pixels — one verifies, one mismatches.
#[test]
fn pixel_mismatch_when_dimensions_match_but_content_differs() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png-rgb-a");
    fixture.case("b.png", "png-rgb-b");
    let digest_a = pixels::sha256(&rgba_rgb_16(10));
    let digest_b = pixels::sha256(&rgba_rgb_16(200));
    assert_ne!(digest_a, digest_b);
    fixture.declare_pixels(&[
        (
            "a.png",
            Some((16, 16)),
            Some(digest_a),
            OutcomeClass::Decoded,
        ),
        (
            "b.png",
            Some((16, 16)),
            Some(digest_a),
            OutcomeClass::Decoded,
        ),
    ]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let by_case = |case: &str| {
        report
            .observations
            .iter()
            .find(|observation| observation.case_id == case)
            .unwrap()
    };
    let matched = by_case("a.png");
    assert_eq!(matched.validity, "pixels_verified");
    assert!(matched.pixel_verified);
    assert_eq!(matched.observed_outcome, OutcomeClass::Decoded);
    assert_eq!(matched.pixel_digest, Some(digest_a));

    let mismatched = by_case("b.png");
    assert_eq!(mismatched.validity, "pixel_mismatch");
    assert!(!mismatched.pixel_verified);
    assert_eq!(mismatched.observed_outcome, OutcomeClass::Error);
    assert_eq!(mismatched.refusal, Some("pixel_mismatch"));
    assert_eq!(mismatched.pixel_digest, Some(digest_b));

    // Only the verified observation contributes a capability row, and it is counted.
    assert_eq!(report.capabilities.len(), 1);
    assert_eq!(report.capabilities[0].observations, 1);
    assert_eq!(report.capabilities[0].pixels_verified, 1);
}

/// Schema 10 closes the gap this slice exists for: one declared `pixels:<sha256>`
/// expectation verifies **every** leg that decoded, not only the legs whose output
/// container the oracle happened to be able to read, because every transport is asked for
/// a canonical PNG. Two independent candidates on the same manifest, on a case whose name
/// carries a non-PNG suffix, must both reach `pixels_verified` against the same
/// independently computed raster.
#[test]
fn one_declared_raster_verifies_every_leg() {
    let fixture = Fixture::new();
    // The case is named `.bmp` on purpose: the declared raster is container-independent,
    // so the input suffix must not restrict which legs can be verified. The helper writes
    // the container the requested output name implies, exactly like the real transports.
    fixture.case("photo.bmp", "suffix-honest");
    let digest = pixels::sha256(&rgba_rgb_16(10));
    fixture.declare_pixels(&[(
        "photo.bmp",
        Some((16, 16)),
        Some(digest),
        OutcomeClass::Decoded,
    )]);
    let candidates = [
        Candidate {
            id: "wic".to_owned(),
            program: Program::Executable(std::env::current_exe().unwrap()),
        },
        Candidate {
            id: "ffmpeg8".to_owned(),
            program: Program::Executable(std::env::current_exe().unwrap()),
        },
    ];
    let report = measured_in(
        &fixture,
        &candidates,
        HELPER,
        harness::OutputPolicy::CanonicalPng,
    );

    assert_eq!(report.output_policy, "canonical-png");
    assert_eq!(report.observations.len(), 2);
    for observation in &report.observations {
        assert_eq!(
            observation.observed_outcome,
            OutcomeClass::Decoded,
            "{}",
            observation.candidate_id
        );
        assert_eq!(
            observation.validity, "pixels_verified",
            "{} must be verified against the same raster",
            observation.candidate_id
        );
        assert!(observation.pixel_verified);
        assert_eq!(observation.pixel_digest, Some(digest));
    }
    assert_eq!(
        report.observations[0].pixel_digest, report.observations[1].pixel_digest,
        "both legs decoded the same raster"
    );
    // Pixel evidence is now per leg, not per one-candidate accident: every candidate has
    // its own capability row and each counts its own verified observation.
    assert_eq!(report.capabilities.len(), 2);
    for row in &report.capabilities {
        assert_eq!(row.observations, 1);
        assert_eq!(row.pixels_verified, 1);
        assert_eq!(row.working_set_max_bytes, None);
    }
}

/// Schema 10: a leg that ignores the canonical PNG request can never pass silently. A BMP
/// container written to the `.png` output path is a complete container, so the structural
/// claim survives — but with a declared raster the verdict is `error` + `pixel_unavailable`
/// carrying the oracle's own token, and without a declared raster the same token explains
/// the missing digest instead of leaving it unexplained.
#[test]
fn a_non_png_container_never_passes_pixel_verification() {
    let fixture = Fixture::new();
    fixture.case("a.bmp", "bmp");

    // Shape 1 — no expectation declared at all. This is also the BLOCKING 2 tamper
    // assertion: accepting the non-PNG output would make the row `presence_only`.
    let canonical = measured_in(
        &fixture,
        &[fixture.fake()],
        HELPER,
        harness::OutputPolicy::CanonicalPng,
    );
    assert_eq!(canonical.output_policy, "canonical-png");
    let observation = &canonical.observations[0];
    assert_eq!(observation.inspected_format, inspect::Format::Bmp);
    assert!(observation.inspected_complete);
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("output_container_not_canonical"));
    assert_eq!(observation.validity, "not_measured");
    assert_eq!(observation.pixel_digest, None);
    assert_eq!(
        observation.pixel_reason,
        Some("output_container_not_canonical")
    );
    assert!(!observation.pixel_verified);
    assert!(
        canonical.capabilities.is_empty(),
        "a failed canonicalisation must not contribute a capability row"
    );

    // Shape 2 — `expected:WxH` only: no structure claim either.
    fixture.declare_expected(&[("a.bmp", Some((8, 4)), OutcomeClass::Decoded)]);
    let canonical = measured_in(
        &fixture,
        &[fixture.fake()],
        HELPER,
        harness::OutputPolicy::CanonicalPng,
    );
    let observation = &canonical.observations[0];
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("output_container_not_canonical"));
    assert_eq!(observation.validity, "not_measured");
    assert_eq!(observation.pixel_digest, None);
    assert!(canonical.capabilities.is_empty());

    // Shape 3 — a declared raster: the refusal replaces any pixel verdict.
    fixture.declare_pixels(&[(
        "a.bmp",
        Some((8, 4)),
        Some(pixels::sha256(&rgba_rgb_16(10))),
        OutcomeClass::Decoded,
    )]);
    let canonical = measured_in(
        &fixture,
        &[fixture.fake()],
        HELPER,
        harness::OutputPolicy::CanonicalPng,
    );
    let observation = &canonical.observations[0];
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("output_container_not_canonical"));
    assert_eq!(observation.pixel_digest, None);
    assert_eq!(
        observation.pixel_reason,
        Some("output_container_not_canonical")
    );
    assert!(!observation.pixel_verified);
    assert!(canonical.capabilities.is_empty());

    // Native mode (the default) is untouched: the same non-PNG output keeps the
    // historical presence/structure behaviour and only the pixel claim is refused.
    fixture.declare(&[("a.bmp", OutcomeClass::Decoded)]);
    let native = measured(&fixture, &[fixture.fake()], HELPER);
    assert_eq!(native.output_policy, "native");
    let observation = &native.observations[0];
    assert_eq!(observation.validity, "presence_only");
    assert_eq!(
        observation.pixel_reason,
        Some(pixels::PixelError::NotPng.as_str()),
        "the absent digest still carries the oracle's own reason"
    );

    fixture.declare_expected(&[("a.bmp", Some((8, 4)), OutcomeClass::Decoded)]);
    let native = measured(&fixture, &[fixture.fake()], HELPER);
    assert_eq!(native.observations[0].validity, "structure_verified");

    fixture.declare_pixels(&[(
        "a.bmp",
        None,
        Some(pixels::sha256(&rgba_rgb_16(10))),
        OutcomeClass::Decoded,
    )]);
    let native = measured(&fixture, &[fixture.fake()], HELPER);
    let observation = &native.observations[0];
    assert_eq!(observation.refusal, Some("pixel_unavailable"));
    assert_eq!(observation.validity_reason, Some("pixel_unavailable"));
    assert_eq!(
        observation.pixel_reason,
        Some(pixels::PixelError::NotPng.as_str())
    );
}

/// Phase 3 (2): same visible pixels but a different colour type/alpha — the digest
/// compares decoded RGBA, so an alpha difference across colour types mismatches.
#[test]
fn pixel_mismatch_across_colour_types() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png-rgb-a");
    fixture.case("b.png", "png-rgba-low-a");
    let rgb_digest = pixels::sha256(&rgba_rgb_16(10));
    fixture.declare_pixels(&[
        (
            "a.png",
            Some((16, 16)),
            Some(rgb_digest),
            OutcomeClass::Decoded,
        ),
        (
            "b.png",
            Some((16, 16)),
            Some(rgb_digest),
            OutcomeClass::Decoded,
        ),
    ]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let by_case = |case: &str| {
        report
            .observations
            .iter()
            .find(|observation| observation.case_id == case)
            .unwrap()
    };
    // RGB expands to alpha 255, so it matches the expectation.
    assert_eq!(by_case("a.png").validity, "pixels_verified");
    // The RGBA file has the same RGB values but alpha 128 at one pixel.
    let mismatched = by_case("b.png");
    assert_eq!(mismatched.validity, "pixel_mismatch");
    assert_eq!(mismatched.refusal, Some("pixel_mismatch"));
    assert_ne!(mismatched.pixel_digest, Some(rgb_digest));
}

/// Phase 3 (3): a single wrong alpha value is a pixel mismatch.
#[test]
fn single_pixel_alpha_mismatch_is_detected() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png-rgba-one-a");
    let expected = pixels::sha256(&rgba_rgb_16(10));
    fixture.declare_pixels(&[(
        "a.png",
        Some((16, 16)),
        Some(expected),
        OutcomeClass::Decoded,
    )]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.validity, "pixel_mismatch");
    assert_eq!(observation.refusal, Some("pixel_mismatch"));
    assert!(!observation.pixel_verified);

    // The oracle sees the difference in exactly one pixel.
    let image = pixels::read_png(&mut std::io::Cursor::new(png_rgba_16(10, &[(137, 0)]))).unwrap();
    let reference = rgba_rgb_16(10);
    let differing = image
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .zip(reference.as_chunks::<4>().0.iter())
        .filter(|(left, right)| left != right)
        .count();
    assert_eq!(differing, 1);
}

/// ANB-1: the pixel expectation wins over a dimension expectation. The output is a
/// 16x16 image whose digest matches, while `expected:32x32` is wrong — the verdict
/// must still be `pixels_verified`, which fails if the dimension rule runs first.
#[test]
fn pixel_expectation_takes_precedence_over_dimensions() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png-rgb-a");
    let digest = pixels::sha256(&rgba_rgb_16(10));
    fixture.declare_pixels(&[("a.png", Some((32, 32)), Some(digest), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.inspected_width, Some(16));
    assert_eq!(observation.expected_dimensions, Some((32, 32)));
    assert_eq!(observation.validity, "pixels_verified");
    assert!(observation.pixel_verified);
    assert_eq!(observation.observed_outcome, OutcomeClass::Decoded);
    assert_eq!(observation.refusal, None);
    assert_eq!(report.capabilities.len(), 1);
    assert_eq!(report.capabilities[0].pixels_verified, 1);
}

/// ANB-1: a mismatching digest wins over a matching dimension expectation, so the
/// verdict is `pixel_mismatch` and never `structure_verified`.
#[test]
fn pixel_mismatch_takes_precedence_over_matching_dimensions() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png-rgb-a");
    let wrong = pixels::sha256(&rgba_rgb_16(200));
    fixture.declare_pixels(&[("a.png", Some((16, 16)), Some(wrong), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.expected_dimensions, Some((16, 16)));
    assert_eq!(observation.inspected_width, Some(16));
    assert_eq!(observation.validity, "pixel_mismatch");
    assert!(!observation.pixel_verified);
    assert_eq!(observation.observed_outcome, OutcomeClass::Error);
    assert_eq!(observation.refusal, Some("pixel_mismatch"));
    assert_ne!(observation.pixel_digest, Some(wrong));
    assert!(report.capabilities.is_empty());
}

/// ANB-2: a present-but-unparsable output reports the same token in the refusal and
/// in `pixel_reason` (`output_unparsable`), never `output_incomplete`.
#[test]
fn unparsable_output_reports_output_unparsable_for_pixels() {
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");
    let digest = pixels::sha256(&rgba_rgb_16(10));
    fixture.declare_pixels(&[("a.png", Some((16, 16)), Some(digest), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert!(observation.output_present);
    assert_eq!(observation.inspected_width, None, "no dimensions were read");
    assert_eq!(observation.refusal, Some("output_unparsable"));
    assert_eq!(observation.pixel_reason, Some("output_unparsable"));
    assert_eq!(observation.validity_reason, Some("output_unparsable"));
    assert_eq!(observation.validity, "not_measured");
    assert_eq!(observation.pixel_digest, None);
    assert!(!observation.pixel_verified);
}

/// A parsed-but-incomplete container keeps the `output_incomplete` token (ANB-2).
#[test]
fn incomplete_container_reports_output_incomplete_for_pixels() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png-header-only");
    let digest = pixels::sha256(&rgba_rgb_16(10));
    fixture.declare_pixels(&[("a.png", Some((16, 16)), Some(digest), OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);

    let observation = &report.observations[0];
    assert_eq!(observation.inspected_width, Some(16));
    assert_eq!(observation.refusal, Some("output_incomplete"));
    assert_eq!(observation.pixel_reason, Some("output_incomplete"));
}

/// NIT-2: the manifest refuses a repeated `pixels:` field and a digest that is not 64
/// hex digits.
#[test]
fn manifest_rejects_bad_pixel_digests() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png16");
    let valid = "789a0a2cba97f193f858638bb8d980e85d62d39a2a681e5b4c29585cd19046a8";

    std::fs::write(
        fixture.cases.join(harness::MANIFEST_FILE),
        format!("a.png|pixels:{valid}|pixels:{valid}|decoded\n"),
    )
    .unwrap();
    let error = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap_err();
    assert!(
        format!("{error}").contains("more than one pixel digest"),
        "unexpected refusal: {error}"
    );

    for bad in [
        "abc",
        "",
        "zz8a0a2cba97f193f858638bb8d980e85d62d39a2a681e5b4c29585cd19046a8",
    ] {
        std::fs::write(
            fixture.cases.join(harness::MANIFEST_FILE),
            format!("a.png|pixels:{bad}|decoded\n"),
        )
        .unwrap();
        let error = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap_err();
        assert!(
            format!("{error}").contains("pixel digest"),
            "digest {bad:?} must be refused: {error}"
        );
    }
}

/// NIT-2 + NIT-3, extended by schema 10: every pixel field is present in each
/// observation row with the documented nullability, a capability row counts
/// `pixels_verified`, and — the schema-10 difference from schema 9 — a decodable raster
/// without a declared expectation carries its digest plus `no_pixel_expectation`, while a
/// missing digest always carries a reason instead of two bare nulls.
#[test]
fn record_carries_the_pixel_fields_with_documented_nullability() {
    let fixture = Fixture::new();
    // Schema 10: no expectation at all, but the raster is decodable, so the digest is
    // recorded and the reason says why nothing is claimed. Schema 9 left both null.
    fixture.case("a.png", "png-rgb-a");
    let report = measured(&fixture, &[fixture.fake()], HELPER);
    let observation = &report.observations[0];
    assert_eq!(report.schema_version, 12);
    assert_eq!(observation.pixel_expectation, None);
    assert_eq!(
        observation.pixel_digest,
        Some(pixels::sha256(&rgba_rgb_16(10)))
    );
    assert!(!observation.pixel_verified);
    assert_eq!(observation.pixel_reason, Some("no_pixel_expectation"));
    // No expectation at all was declared for this case, so the claim stays as narrow as
    // it was before: presence of a decode, no structure or dimension claim.
    assert_eq!(observation.validity, "presence_only");
    let json = harness::render(&report).0;
    for field in [
        "\"schema_version\": 12",
        "\"pixel_expectation\": null",
        "\"pixel_verified\": false",
        "\"pixel_reason\": \"no_pixel_expectation\"",
    ] {
        assert!(json.contains(field), "missing {field} in {json}");
    }
    assert!(json.contains("\"pixel_digest\": \""), "{json}");

    // An output the oracle cannot decode still leaves the digest null, and the reason
    // token (never a bare null) says why.
    fixture.case("b.png", "png16");
    let report = measured(&fixture, &[fixture.fake()], HELPER);
    let observation = report
        .observations
        .iter()
        .find(|observation| observation.case_id == "b.png")
        .expect("b.png was measured");
    assert_eq!(observation.pixel_expectation, None);
    assert_eq!(observation.pixel_digest, None);
    assert!(
        observation.pixel_reason.is_some(),
        "a missing digest must carry a reason"
    );
    assert_eq!(observation.validity, "presence_only");

    // With a matching expectation: digest and expectation are hex, verified is true.
    fixture.declare_pixels(&[(
        "a.png",
        Some((16, 16)),
        Some(pixels::sha256(&rgba_rgb_16(10))),
        OutcomeClass::Decoded,
    )]);
    std::fs::remove_file(fixture.cases.join("b.png")).unwrap();
    // The 16x16 fixture with a real IDAT: reuse the pixel fake here.
    fixture.case("a.png", "png-rgb-a");
    let report = measured(&fixture, &[fixture.fake()], HELPER);
    let json = harness::render(&report).0;
    let digest = pixels::hex(&pixels::sha256(&rgba_rgb_16(10)));
    assert!(json.contains(&format!("\"pixel_expectation\": \"{digest}\"")));
    assert!(json.contains(&format!("\"pixel_digest\": \"{digest}\"")));
    assert!(json.contains("\"pixel_verified\": true"));
    assert!(json.contains("\"validity\": \"pixels_verified\""));
    // NIT-3: the capability row counts, the observation row is boolean.
    assert!(json.contains("\"pixels_verified\": 1"), "{json}");
    // Schema 11: the record says which output policy produced it.
    assert!(
        json.contains("\"output_policy\": \"native\""),
        "the default policy must be recorded: {json}"
    );

    // ACTIONABLE shape (b): no expectation and no output at all — the digest is absent and
    // the reason is still closed (`no_output`), not a bare null.
    std::fs::remove_file(fixture.cases.join("a.png")).unwrap();
    fixture.case("c.png", "nooutput");
    fixture.declare(&[("c.png", OutcomeClass::Decoded)]);
    let report = measured(&fixture, &[fixture.fake()], HELPER);
    let observation = &report.observations[0];
    assert_eq!(observation.pixel_expectation, None);
    assert!(!observation.output_present);
    assert_eq!(observation.pixel_digest, None);
    assert_eq!(observation.pixel_reason, Some("no_output"));

    // ACTIONABLE shape (a): a declared raster, an output file that exists but is not a
    // complete container, and an outcome that is not `Decoded` (a cancelled leg that left
    // a partial file). The digest is absent and its reason is recorded.
    std::fs::remove_file(fixture.cases.join("c.png")).unwrap();
    fixture.case("partial.png", "partial-sleep");
    fixture.declare_pixels(&[(
        "partial.png",
        Some((16, 16)),
        Some(pixels::sha256(&rgba_rgb_16(10))),
        OutcomeClass::Cancelled,
    )]);
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    config.timeout_ms = 10_000;
    config.cancel_after_ms = Some(150);
    let report = harness::run(&config).unwrap();
    let observation = &report.observations[0];
    assert_eq!(observation.observed_outcome, OutcomeClass::Cancelled);
    assert!(
        observation.output_present,
        "the cancelled leg left a partial file"
    );
    assert!(!observation.inspected_complete);
    assert_eq!(observation.pixel_digest, None);
    assert_eq!(observation.pixel_reason, Some("output_incomplete"));
    assert_eq!(observation.validity, "not_measured");
}

/// Item 1: a helper that allocates a known amount is sampled at least that large.
#[test]
fn working_set_sample_lower_bounds_a_known_allocation() {
    let fixture = Fixture::new();
    fixture.case("a.png", "alloc-64m");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    config.timeout_ms = 10_000;
    config.sample_tool = Some(system_tasklist());
    let report = run_until_sampled(&config, 3);

    let observation = &report.observations[0];
    assert!(observation.working_set_samples >= 1, "at least one sample");
    assert_eq!(observation.working_set_sampling, "sampled_lower_bound");
    let bytes = observation
        .working_set_max_bytes
        .expect("a sample must produce a value");
    assert!(
        bytes >= 16 * 1024 * 1024,
        "the sampled working set ({bytes}) must be at least the allocation"
    );
    assert_eq!(observation.working_set_reason, None);
}

/// Item 2: a helper that ends before the first sample is `not_measured`, never 0.
#[test]
fn working_set_is_not_measured_without_a_sample() {
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    // Opt in, but the first sample waits a full interval and the child ends first.
    config.sample_tool = Some(system_tasklist());
    config.sample_interval_ms = 5_000;
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert_eq!(observation.working_set_samples, 0);
    assert_eq!(observation.working_set_max_bytes, None, "never a guessed 0");
    assert_eq!(observation.working_set_sampling, "not_measured");
    assert_eq!(observation.working_set_reason, Some("no_sample"));
}

/// Item 3: the configured sample cap is enforced.
#[test]
fn working_set_sampling_respects_the_cap() {
    let _guard = sampling_guard();
    let fixture = Fixture::new();
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    // The injected helper keeps the per-sample cost low and the candidate waits for both
    // samples, so the cap is what is measured here rather than the machine's load.
    await_sampler(&fixture, &mut config, "fake_sampler_valid_helper", 2);
    config.max_samples = 2;
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert_eq!(
        observation.working_set_attempts, 2,
        "the cap bounds attempts"
    );
    assert_eq!(
        observation.working_set_samples, 2,
        "exactly the configured cap"
    );
    assert!(observation.working_set_max_bytes.is_some());
}

/// Item 4: an operator cancellation records both latencies and the `cancelled` class.
#[test]
fn cancellation_records_latency_and_a_cancelled_class() {
    let fixture = Fixture::new();
    fixture.case("a.png", "sleep-long");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    config.timeout_ms = 10_000;
    config.cancel_after_ms = Some(150);
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert_eq!(observation.observed_outcome, OutcomeClass::Cancelled);
    assert_eq!(observation.validity, "not_measured");
    assert!(!observation.timeout_fired, "the operator deadline fired");
    assert_eq!(observation.exit_code, None);
    let requested = observation
        .cancellation_requested_at_ms
        .expect("the kill request must be timed");
    assert!(
        (140..600).contains(&requested),
        "the request should land near the 150 ms deadline ({requested})"
    );
    let observed = observation
        .cancellation_observed_ms
        .expect("the kill round trip must be timed");
    assert!(observed < 600, "cancel round trip took {observed} ms");
    assert!(!observation.output_present);
    assert_eq!(
        observation.cancelled_at_reason, None,
        "the helper said nothing"
    );

    // A candidate that claims cancellation and then hangs is killed, so the claim is
    // recorded: the observed class really is `cancelled`.
    fixture.case("c.png", "cancel-hang");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    config.cancel_after_ms = Some(300);
    let report = harness::run(&config).unwrap();
    let observation = &report
        .observations
        .iter()
        .find(|observation| observation.case_id == "c.png")
        .unwrap();
    assert_eq!(observation.observed_outcome, OutcomeClass::Cancelled);
    assert_eq!(
        observation.cancelled_at_reason,
        Some("cancelled_at_cancel"),
        "a real cancellation must keep the candidate claim"
    );
}

/// Item 5: a hard timeout is `cancelled` and can never be `decoded`.
#[test]
fn timeout_never_yields_decoded() {
    let fixture = Fixture::new();
    fixture.case("a.png", "sleep-long");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    config.timeout_ms = 200;
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert!(observation.timeout_fired);
    assert_eq!(observation.observed_outcome, OutcomeClass::Cancelled);
    assert_ne!(observation.observed_outcome, OutcomeClass::Decoded);
    assert_eq!(observation.validity, "not_measured");
    assert!(observation.cancellation_requested_at_ms.is_some());
    assert!(observation.cancellation_observed_ms.is_some());
    assert!(report.capabilities.is_empty());
    assert_eq!(report.resources[0].observations, 0);
    assert_eq!(
        report.resources[0].working_set_reason,
        Some("no_contributing_observation")
    );
}

/// Item 6 / A4: a cancelled run contributes neither a capability nor a resource row.
/// One run, two cases: a fast verified decode and a slow one that the operator deadline
/// cancels, both under the same candidate. The fast helper finishes in tens of
/// milliseconds against a 400 ms deadline, so the split is deterministic.
#[test]
fn resource_and_capability_rows_exclude_cancelled_runs() {
    let fixture = Fixture::new();
    fixture.case("a.png", "png-rgb-a");
    fixture.case("b.png", "sleep-long");
    fixture.declare_pixels(&[
        (
            "a.png",
            Some((16, 16)),
            Some(pixels::sha256(&rgba_rgb_16(10))),
            OutcomeClass::Decoded,
        ),
        ("b.png", Some((16, 16)), None, OutcomeClass::Decoded),
    ]);
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    config.timeout_ms = 10_000;
    config.cancel_after_ms = Some(400);
    let report = harness::run(&config).unwrap();

    let by_case = |case: &str| {
        report
            .observations
            .iter()
            .find(|observation| observation.case_id == case)
            .unwrap()
    };
    assert_eq!(by_case("a.png").validity, "pixels_verified");
    let cancelled = by_case("b.png");
    assert_eq!(cancelled.observed_outcome, OutcomeClass::Cancelled);
    assert_eq!(cancelled.validity, "not_measured");

    assert_eq!(
        report.capabilities.len(),
        1,
        "only the verified case counts"
    );
    assert_eq!(report.capabilities[0].observations, 1);
    assert_eq!(report.resources.len(), 1);
    assert_eq!(report.resources[0].observations, 1);
    assert!(report.resources[0].scratch_max_bytes.is_some());
}

/// Item 7 / A4: two sampled runs differ only in the documented volatile fields. The
/// candidate lives long enough that the sampler really produces samples.
#[test]
fn sampled_record_is_deterministic_apart_from_documented_fields() {
    let _guard = sampling_guard();
    let fixture = Fixture::new();
    // The candidate ends the run once a sample has landed, so neither a slow spawn nor a
    // late poll can empty the sampling window.
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    config.timeout_ms = 10_000;
    await_sampler(&fixture, &mut config, "fake_sampler_valid_helper", 1);
    let first = harness::run(&config).unwrap();
    let second = harness::run(&config).unwrap();
    assert!(
        first.observations[0].working_set_samples >= 1,
        "the determinism case must actually sample"
    );
    harness::write_report(&first, &fixture.record("first")).unwrap();
    harness::write_report(&second, &fixture.record("second")).unwrap();
    let first_json = std::fs::read_to_string(fixture.record("first")).unwrap();
    let second_json = std::fs::read_to_string(fixture.record("second")).unwrap();
    assert_eq!(
        normalize_volatile(&first_json),
        normalize_volatile(&second_json),
        "only the documented timing/sampling fields may differ between runs"
    );
    for field in [
        "\"wall_ms\"",
        "\"scratch_max_bytes\"",
        "\"working_set_attempts\"",
        "\"working_set_samples\"",
        "\"working_set_max_bytes\"",
        "\"working_set_sampling\"",
        "\"working_set_reason\"",
        "\"cancellation_requested_at_ms\"",
        "\"cancellation_observed_ms\"",
    ] {
        assert!(first_json.contains(field), "missing {field}");
    }
}

/// N5: a non-UTF8 tool path renders lossily instead of `null`; the digest beside it is
/// what carries the exact identity.
#[test]
fn non_utf8_tool_path_renders_lossily() {
    use std::os::windows::ffi::OsStringExt;
    let raw = std::ffi::OsString::from_wide(&[b'C' as u16, b':' as u16, b'\\' as u16, 0xD800]);
    let path = std::path::PathBuf::from(raw);
    assert!(
        path.to_str().is_none(),
        "the fixture path is not valid UTF-8"
    );
    let rendered = harness::lossy_path_field_for_tests(Some(&path));
    assert_ne!(rendered, "null");
    assert!(rendered.contains('C'));
    assert!(rendered.contains('\u{fffd}'));
    assert_eq!(harness::lossy_path_field_for_tests(None), "null");
}

/// BLOCKING (3): a hash failure is `not_measured` with a reason, never a fabricated
/// digest. An unreadable file is simulated with a directory-shaped path.
#[test]
fn sampler_digest_failure_is_not_measured() {
    assert_eq!(
        harness::read_sample_tool_for_tests(std::path::Path::new("C:\\Windows\\System32")),
        Err("sample_tool_unreadable")
    );
}

/// N2/NIT N3: the bounded read sits exactly at the cap and refuses one byte over.
#[test]
fn oversized_sampler_is_refused_by_the_cap() {
    let fixture = Fixture::new();
    let at_cap = fixture.base.join("at-cap-tool.exe");
    std::fs::write(&at_cap, vec![0_u8; 32 * 1024 * 1024]).unwrap();
    assert_eq!(
        harness::read_sample_tool_for_tests(&at_cap).map(|bytes| bytes.len()),
        Ok(32 * 1024 * 1024),
        "a file exactly at the cap is read"
    );

    let over_cap = fixture.base.join("over-cap-tool.exe");
    std::fs::write(&over_cap, vec![0_u8; 32 * 1024 * 1024 + 1]).unwrap();
    assert_eq!(
        harness::read_sample_tool_for_tests(&over_cap),
        Err("sample_tool_oversize"),
        "one byte over the cap is refused"
    );
}

/// A3: the sampler bookkeeping for both mixed orders. A success clears an earlier
/// failure, and a later failure never clears a value.
#[test]
fn sampler_bookkeeping_keeps_status_and_reason_consistent() {
    // fail-then-success
    let mut sampler = harness::Sampler::for_tests(4);
    assert_eq!(sampler.sampling, "not_measured");
    sampler.attempt();
    sampler.failed("sample_parse_failed");
    assert_eq!(sampler.sampling, "not_measured");
    assert_eq!(sampler.reason, Some("sample_parse_failed"));
    sampler.attempt();
    sampler.succeeded(1_024);
    assert_eq!(sampler.sampling, "sampled_lower_bound");
    assert_eq!(sampler.reason, None, "a success clears the failure reason");
    assert_eq!(sampler.max_bytes, Some(1_024));

    // success-then-fail
    let mut sampler = harness::Sampler::for_tests(4);
    sampler.attempt();
    sampler.succeeded(2_048);
    sampler.attempt();
    sampler.failed("sample_timeout");
    assert_eq!(sampler.sampling, "sampled_lower_bound");
    assert_eq!(sampler.reason, None, "a failure never clears a value");
    assert_eq!(sampler.max_bytes, Some(2_048));

    // never sampled: no_sample, and the cap is truthful
    let mut sampler = harness::Sampler::for_tests(4);
    sampler.finish();
    assert_eq!(sampler.sampling, "not_measured");
    assert_eq!(sampler.reason, Some("no_sample"));

    let mut sampler = harness::Sampler::for_tests(1);
    sampler.attempt();
    sampler.capped();
    assert!(!sampler.may_attempt());
    sampler.finish();
    assert_eq!(sampler.reason, Some("sample_capped"));
}

/// NIT-6: `cancelled_at_reason` is recorded only for a class that really is `cancelled`.
#[test]
fn cancelled_at_reason_requires_a_cancelled_class() {
    let fixture = Fixture::new();
    fixture.case("a.png", "reason-cancelled");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    let report = harness::run(&config).unwrap();
    let observation = &report.observations[0];
    // The candidate claims cancellation but exits successfully: not a cancelled class.
    assert_ne!(observation.observed_outcome, OutcomeClass::Cancelled);
    assert_eq!(observation.cancelled_at_reason, None);

    fixture.case("b.png", "sleep-long");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    config.cancel_after_ms = Some(150);
    let report = harness::run(&config).unwrap();
    let observation = &report
        .observations
        .iter()
        .find(|observation| observation.case_id == "b.png")
        .unwrap();
    assert_eq!(observation.observed_outcome, OutcomeClass::Cancelled);
    assert_eq!(
        observation.cancelled_at_reason, None,
        "the helper said nothing"
    );
}

/// A1: a failure *after* a successful sample is decisive. The first invocation yields a
/// value, the second exits non-zero; the observation must drop the value and keep the
/// token instead of ending `sampled_lower_bound`.
#[test]
fn failure_after_a_success_discards_the_value_on_nonzero_exit() {
    let fixture = Fixture::new();
    fixture.case("a.png", "sleep-long");
    let counter = fixture.base.join("spawns-success-then-fail.txt");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    inject_sampler(&mut config, "fake_sampler_success_then_failure_helper");
    config.sample_argv_prefix.push(counter.into_os_string());
    config.sample_interval_ms = 10;
    config.max_samples = 3;
    config.cancel_after_ms = Some(2_500);
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert!(
        observation.working_set_attempts >= 2,
        "a success and then a failure were attempted ({} attempts)",
        observation.working_set_attempts
    );
    assert_eq!(
        observation.working_set_samples, 0,
        "the earlier value is discarded"
    );
    assert_eq!(observation.working_set_max_bytes, None);
    assert_eq!(observation.working_set_sampling, "not_measured");
    assert_eq!(
        observation.working_set_reason,
        Some("sample_tool_failed"),
        "the later failure is the observation's verdict"
    );
}

/// A1: the same for a truncated capture after an earlier success.
#[test]
fn failure_after_a_success_discards_the_value_on_truncation() {
    let fixture = Fixture::new();
    fixture.case("a.png", "sleep-long");
    let counter = fixture.base.join("spawns-success-then-truncate.txt");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    inject_sampler(&mut config, "fake_sampler_success_then_truncating_helper");
    config.sample_argv_prefix.push(counter.into_os_string());
    config.sample_interval_ms = 10;
    config.max_samples = 3;
    config.cancel_after_ms = Some(2_500);
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert!(
        observation.working_set_attempts >= 2,
        "a success and then a truncation were attempted ({} attempts)",
        observation.working_set_attempts
    );
    assert_eq!(observation.working_set_samples, 0);
    assert_eq!(observation.working_set_max_bytes, None);
    assert_eq!(observation.working_set_sampling, "not_measured");
    assert_eq!(
        observation.working_set_reason,
        Some("sample_output_truncated")
    );
}

/// BLOCKING B-R1-01 (a): a valid-looking row printed by a sampler that exits non-zero is
/// refused; the attempt is counted and no value is taken.
#[test]
fn nonzero_sampler_exit_is_sample_tool_failed() {
    let _guard = sampling_guard();
    let fixture = Fixture::new();
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    await_sampler(&fixture, &mut config, "fake_sampler_failing_helper", 1);
    config.max_samples = 1;
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert_eq!(observation.working_set_attempts, 1, "the attempt was made");
    assert_eq!(observation.working_set_samples, 0);
    assert_eq!(
        observation.working_set_max_bytes, None,
        "no value from a failed run"
    );
    assert_eq!(observation.working_set_sampling, "not_measured");
    assert_eq!(
        observation.working_set_reason,
        Some("sample_tool_failed"),
        "a non-zero exit is a sample failure"
    );
}

/// BLOCKING B-R1-01 (b): a row parsed out of a truncated capture is incomplete evidence
/// and is refused.
#[test]
fn truncated_sample_capture_is_refused() {
    let _guard = sampling_guard();
    let fixture = Fixture::new();
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    await_sampler(&fixture, &mut config, "fake_sampler_truncating_helper", 1);
    config.sample_interval_ms = 10;
    config.max_samples = 1;
    config.cancel_after_ms = Some(60_000);
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert_eq!(observation.working_set_attempts, 1);
    assert_eq!(observation.working_set_samples, 0);
    assert_eq!(observation.working_set_max_bytes, None);
    assert_eq!(observation.working_set_sampling, "not_measured");
    assert_eq!(
        observation.working_set_reason,
        Some("sample_output_truncated"),
        "a truncated capture is not a complete row"
    );
}

/// Positive control: the same seam, a sampler that exits 0 with a small capture, still
/// produces the value — the new checks must not reject good samples.
#[test]
fn successful_sampler_still_records_the_value() {
    let _guard = sampling_guard();
    let fixture = Fixture::new();
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    // The candidate ends the run once the sampler has landed, so a slow spawn cannot
    // empty the window; the interval stays small and the cap is the default.
    await_sampler(&fixture, &mut config, "fake_sampler_valid_helper", 1);
    // A cap well above what the short window can reach, so  stays empty.
    config.max_samples = 256;
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert!(observation.working_set_attempts >= 1);
    assert!(observation.working_set_samples >= 1);
    assert_eq!(observation.working_set_max_bytes, Some(12_000 * 1024));
    assert_eq!(observation.working_set_sampling, "sampled_lower_bound");
    assert_eq!(observation.working_set_reason, None);
}

/// A2: the post-spawn digest check is load-bearing. The sampler helper replaces its own
/// program file while it runs and then emits a valid row; the harness must discard that
/// value because the executed bytes are no longer the verified ones.
#[test]
fn tool_replaced_while_the_sampler_runs_discards_the_value() {
    let _guard = sampling_guard();
    let fixture = Fixture::new();
    let tool = fixture.base.join("tool.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &tool).unwrap();
    // This test supplies its own tool path, so it sets the candidate mode directly.
    fixture.case("a.png", "await-sampler");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    config.sample_tool = Some(tool.clone());
    let sentinel = fixture.base.join("sampler.done");
    let _ = std::fs::remove_file(&sentinel);
    config.sample_argv_prefix = vec![
        std::ffi::OsString::from("--exact"),
        std::ffi::OsString::from("fake_sampler_replacing_helper"),
        std::ffi::OsString::from("--nocapture"),
        std::ffi::OsString::from("--quiet"),
        sentinel.into_os_string(),
    ];
    config.sample_interval_ms = 10;
    // Exactly one attempt, so no later pre-spawn check can mask a missing post-spawn
    // check: only the harvest-time verification can discard this value.
    config.max_samples = 1;
    // The candidate ends the run once the replaced sampler has finished; the bound only
    // stops a broken helper from hanging the suite.
    config.cancel_after_ms = Some(60_000);
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert_eq!(
        observation.working_set_attempts, 1,
        "the single sample ran before the program was replaced"
    );
    assert_eq!(
        observation.working_set_samples, 0,
        "the value must be discarded"
    );
    assert_eq!(observation.working_set_max_bytes, None);
    assert_eq!(observation.working_set_sampling, "not_measured");
    assert_eq!(
        observation.working_set_reason,
        Some("sample_tool_changed"),
        "the post-spawn check must catch the replacement"
    );
}

/// A3: the real record rendering carries the three sampler identity fields, in both the
/// opt-out and the opt-in shape.
#[test]
fn rendered_header_records_the_sampler_identity() {
    // Opt-out shape.
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    let report = harness::run(&config).unwrap();
    let json = harness::render(&report).0;
    assert!(json.contains("\"sample_tool\": null"), "{json}");
    assert!(json.contains("\"sample_tool_sha256\": null"), "{json}");
    assert!(
        json.contains("\"sample_tool_reason\": \"sampling_disabled\""),
        "{json}"
    );

    // Opt-in shape with the real tasklist.
    let tool = system_tasklist();
    fixture.case("a.png", "alloc-64m");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    config.timeout_ms = 10_000;
    config.sample_tool = Some(tool.clone());
    config.sample_interval_ms = 10;
    let report = harness::run(&config).unwrap();
    let json = harness::render(&report).0;
    let digest = report
        .sample_tool_sha256
        .clone()
        .expect("the tool is hashed");
    let expected_path = tool.to_string_lossy().replace('\\', "\\\\");
    assert!(
        json.contains(&format!("\"sample_tool\": \"{expected_path}\"")),
        "{json}"
    );
    assert!(
        json.contains(&format!("\"sample_tool_sha256\": \"{digest}\"")),
        "{json}"
    );
    assert!(json.contains("\"sample_tool_reason\": null"), "{json}");
}

/// The CLI surface: sampling is opt-in through `--sample-tool`, and a relative or
/// non-local value is refused before anything runs.
#[test]
fn sample_tool_flag_is_opt_in_and_validated() {
    let fixture = Fixture::new();
    fixture.case("a.png", "ok");

    // No flag: nothing is configured, so no sampler can be spawned at all.
    let config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    assert_eq!(config.sample_tool, None);

    let with_tool = |value: &str| {
        harness::parse(
            arguments(&fixture, &[fixture.fake()])
                .into_iter()
                .chain(["--sample-tool".into(), std::ffi::OsString::from(value)])
                .collect::<Vec<std::ffi::OsString>>(),
        )
    };

    // The accepted form: an absolute local disk path, used exactly as given.
    let absolute = r"C:\\Windows\\System32\\tasklist.exe";
    assert_eq!(
        with_tool(absolute).unwrap().sample_tool,
        Some(std::path::PathBuf::from(absolute))
    );

    // A relative value is refused outright, before any run.
    let error = with_tool("tasklist.exe").unwrap_err();
    assert!(
        format!("{error}").contains("--sample-tool"),
        "unexpected refusal: {error}"
    );

    // Every non-local shape is refused with the canonical literal: UNC, verbatim,
    // device, drive-relative and parent traversal.
    for bad in [
        r"\\server\share\x.exe",
        r"\\?\C:\x.exe",
        r"\\.\C:\x.exe",
        r"C:x.exe",
        r"C:\Windows\..\x.exe",
    ] {
        assert!(with_tool(bad).is_err(), "{bad} must be refused");
    }

    // A `.` component cannot change the target and the path parser normalises it
    // away, so it is accepted (documented behaviour of `is_local_disk_absolute`).
    assert_eq!(
        with_tool(r"C:\Windows\.\System32\tasklist.exe")
            .unwrap()
            .sample_tool,
        Some(std::path::PathBuf::from(
            r"C:\Windows\.\System32\tasklist.exe"
        ))
    );
}

/// Opt-in with the real system tasklist path supplied on the command line: samples are
/// taken from that path and the header digest is that file's digest.
#[test]
fn opt_in_samples_the_operator_supplied_tasklist() {
    let fixture = Fixture::new();
    fixture.case("a.png", "alloc-64m");
    let tool = std::path::Path::new(&std::env::var("SystemRoot").unwrap())
        .join("System32")
        .join("tasklist.exe");
    let mut config = harness::parse(
        arguments(&fixture, &[fixture.fake()])
            .into_iter()
            .chain([
                "--sample-tool".into(),
                std::ffi::OsString::from(tool.as_os_str()),
            ])
            .collect::<Vec<std::ffi::OsString>>(),
    )
    .unwrap();
    config.invocation = helper_invocation(HELPER);
    config.timeout_ms = 10_000;
    config.sample_interval_ms = 10;
    config.cancel_after_ms = Some(3_000);
    let report = run_until_sampled(&config, 3);

    assert_eq!(report.sample_tool.as_ref(), Some(&tool));
    let digest = report
        .sample_tool_sha256
        .clone()
        .expect("the tool is hashed");
    assert_eq!(
        digest,
        pixels::hex(&pixels::sha256(&std::fs::read(&tool).unwrap())),
        "the recorded digest is the operator-supplied file's digest"
    );
    let observation = &report.observations[0];
    assert!(observation.working_set_attempts >= 1);
    assert!(observation.working_set_samples >= 1);
    assert_eq!(observation.working_set_sampling, "sampled_lower_bound");
    assert_eq!(observation.working_set_reason, None);
    assert!(observation.working_set_max_bytes.unwrap() > 0);
}

/// Default off: without `--sample-tool` no sampler is spawned, and a decoy
/// `DECODE_COMPARE_SAMPLE_TOOL` in the environment changes nothing. `attempts == 0` is
/// the spawn count, because every spawn increments it at the single spawn site.
#[test]
fn default_off_spawns_no_sampler_and_ignores_the_environment() {
    let fixture = Fixture::new();
    fixture.case("a.png", "sleep-long");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    config.sample_interval_ms = 10;
    config.cancel_after_ms = Some(400);
    let report = harness::run(&config).unwrap();

    assert_eq!(report.sample_tool, None);
    assert_eq!(report.sample_tool_sha256, None);
    assert_eq!(report.sample_tool_reason, Some("sampling_disabled"));
    let observation = &report.observations[0];
    assert_eq!(
        observation.working_set_attempts, 0,
        "no sampler was spawned"
    );
    assert_eq!(observation.working_set_samples, 0);
    assert_eq!(observation.working_set_max_bytes, None);
    assert_eq!(observation.working_set_sampling, "not_measured");
    assert_eq!(observation.working_set_reason, Some("sampling_disabled"));

    // Same run in a child process whose environment names a decoy sampler: still nothing.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "sampler_probe_helper", "--nocapture", "--quiet"])
        .env(
            "DECODE_COMPARE_SAMPLE_TOOL",
            r"C:\\tmp\\a-decoy-sampler\\tasklist.exe",
        )
        .env("DC_PROBE_CASES", fixture.cases.as_os_str())
        .env(
            "DC_PROBE_CANDIDATE",
            fixture.fake().program.path().as_os_str(),
        )
        .env("DC_PROBE_OUT", fixture.record("probe"))
        .output()
        .expect("the probe subprocess runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("tool=<none>") && stdout.contains("attempts=0"),
        "the environment must not select or spawn a sampler; got {stdout}"
    );
}

/// A `subst` mapping chosen at runtime and released on drop, so a failing assertion can
/// never leave a drive letter behind.
struct SubstDrive {
    letter: String,
}

impl SubstDrive {
    fn create(target: &std::path::Path) -> Option<Self> {
        for letter in ('D'..='Z').map(|character| format!("{character}:")) {
            // Only a letter that is not in use can be mapped.
            if std::fs::metadata(format!("{letter}\\")).is_ok() {
                continue;
            }
            let mapped = std::process::Command::new("subst")
                .arg(&letter)
                .arg(target.as_os_str())
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            if mapped {
                return Some(Self { letter });
            }
        }
        None
    }

    fn root(&self) -> std::ffi::OsString {
        std::ffi::OsString::from(format!("{}\\", self.letter))
    }
}

impl Drop for SubstDrive {
    fn drop(&mut self) {
        let _ = std::process::Command::new("subst")
            .arg(&self.letter)
            .arg("/D")
            .status();
    }
}

/// The subst/decoy-root scenario cannot fabricate any more: no environment value is
/// consulted, so a decoy `SystemRoot` on a substituted drive changes nothing. With
/// `--sample-tool` the operator path is the only sampler. The mapping is released on
/// drop, whatever the assertions do.
#[test]
fn subst_decoy_root_cannot_select_the_sampler() {
    let fixture = Fixture::new();
    fixture.case("a.png", "alloc-64m");
    let decoy = fixture.base.join("decoy-root");
    std::fs::create_dir_all(decoy.join("System32")).unwrap();
    std::fs::write(
        decoy.join("System32").join("tasklist.exe"),
        b"a decoy that would fabricate a value",
    )
    .unwrap();

    let drive = SubstDrive::create(&decoy);
    let decoy_root = match drive.as_ref() {
        Some(drive) => drive.root(),
        None => decoy.clone().into_os_string(),
    };

    let probe = |tool: Option<&std::path::Path>| {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "sampler_probe_helper", "--nocapture", "--quiet"])
            .env("SystemRoot", decoy_root.as_os_str())
            .env(
                "DECODE_COMPARE_SAMPLE_TOOL",
                decoy.join("System32").join("tasklist.exe"),
            )
            .env("DC_PROBE_CASES", fixture.cases.as_os_str())
            .env(
                "DC_PROBE_CANDIDATE",
                fixture.fake().program.path().as_os_str(),
            )
            .env("DC_PROBE_OUT", fixture.record("probe"));
        if let Some(tool) = tool {
            command.env("DC_PROBE_TOOL", tool.as_os_str());
        }
        let output = command.output().expect("the probe subprocess runs");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    // Default off under a decoy root: nothing is spawned, nothing is fabricated.
    let off = probe(None);
    assert!(
        off.contains("tool=<none>") && off.contains("attempts=0") && off.contains("fabricated=0"),
        "a decoy root must not select a sampler; got {off}"
    );

    // Opt in with the real tasklist: only the operator path is used.
    let real = system_tasklist();
    let on = probe(Some(&real));
    assert!(
        on.contains(&format!("tool={}", real.display())) && on.contains("fabricated=0"),
        "the operator path must win and the decoy must never be used; got {on}"
    );
}

/// Child harness for the opt-in probes: it prints what actually happened, so the parent
/// can assert on a run whose environment it controls.
#[test]
fn sampler_probe_helper() {
    let Ok(cases) = std::env::var("DC_PROBE_CASES") else {
        return;
    };
    let candidate = std::env::var("DC_PROBE_CANDIDATE").unwrap_or_default();
    let out = std::env::var("DC_PROBE_OUT").unwrap_or_default();
    let mut argv: Vec<std::ffi::OsString> = vec![
        "--cases".into(),
        cases.into(),
        "--candidate".into(),
        format!("fake={candidate}").into(),
        "--out".into(),
        out.into(),
        "--sample-interval-ms".into(),
        "10".into(),
        "--cancel-after-ms".into(),
        "500".into(),
    ];
    if let Ok(tool) = std::env::var("DC_PROBE_TOOL") {
        argv.push("--sample-tool".into());
        argv.push(tool.into());
    }
    let mut config = harness::parse(argv).expect("the probe command line parses");
    config.invocation = helper_invocation(HELPER);
    let report = harness::run(&config).expect("the probe run completes");
    let tool = match report.sample_tool.clone() {
        Some(path) => path.display().to_string(),
        None => "<none>".to_owned(),
    };
    let attempts = report
        .observations
        .iter()
        .map(|observation| observation.working_set_attempts)
        .max()
        .unwrap_or(0);
    let fabricated = report
        .observations
        .iter()
        .filter(|observation| observation.working_set_max_bytes == Some(12_288_000))
        .count();
    println!(
        "probe tool={tool} digest={} attempts={attempts} fabricated={fabricated} reason={:?}",
        report.sample_tool_sha256.is_some(),
        report.sample_tool_reason
    );
}

/// NIT N1 + N3: the digest is verified at the supervise call sites, and a changed tool
/// discards a value that already existed instead of silently keeping it.
#[test]
fn changing_tool_after_a_success_discards_the_value() {
    let mut sampler = harness::Sampler::for_tests(8);
    sampler.attempt();
    sampler.succeeded(4_096);
    assert_eq!(sampler.sampling, "sampled_lower_bound");
    sampler.discarded("sample_tool_changed");
    assert_eq!(sampler.samples, 0);
    assert_eq!(sampler.max_bytes, None, "the untrusted value is discarded");
    assert_eq!(sampler.sampling, "not_measured");
    assert_eq!(sampler.reason, Some("sample_tool_changed"));
}

/// NIT N3: a sampler program rewritten while the run is in flight records
/// `sample_tool_changed` and never yields a value. Either comparison can produce the
/// token (refused before the spawn, or discarded after it), so this test bounds the
/// attempt count instead of pinning it; the distinct proof for the *pre-spawn* call site
/// is `pre_spawn_digest_check_refuses_a_rewritten_then_restored_tool`, which restores the
/// file before the post-spawn check would run.
#[test]
fn tool_rewritten_before_the_spawn_records_sample_tool_changed() {
    let fixture = Fixture::new();
    let tool = fixture.base.join("tool.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &tool).unwrap();
    fixture.case("a.png", "mutate-tool");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    // A late first attempt makes the ordering deterministic: the candidate has long
    // finished rewriting the tool before the sampler is considered, and the child
    // outlives the deadline, so neither a slow start nor a slow poll can flip the
    // verdict. Both orderings still end as `sample_tool_changed` (refused before the
    // spawn, or the sample is discarded after the swap).
    config.sample_interval_ms = 2_000;
    config.cancel_after_ms = Some(6_000);
    config.sample_tool = Some(tool.clone());
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    // The invariant is that a rewritten tool never yields a value and the swap is
    // attributed. The attempt count depends on whether the candidate had already
    // rewritten the file when the first attempt was scheduled (0 = refused before the
    // spawn, 1 = the sample ran and was discarded), so it is bounded, not pinned.
    assert!(
        observation.working_set_attempts <= 1,
        "at most the first attempt can race the rewrite ({} attempts)",
        observation.working_set_attempts
    );
    assert_eq!(observation.working_set_samples, 0);
    assert_eq!(
        observation.working_set_max_bytes, None,
        "no value from a swapped tool"
    );
    assert_eq!(observation.working_set_sampling, "not_measured");
    assert_eq!(observation.working_set_reason, Some("sample_tool_changed"));
}

/// How the rewrite window closed, so a failure names the reason instead of a write error.
enum Window {
    /// The rewrite and the restore both happened inside the run.
    Opened,
    /// The sampler was already running from the rewritten file.
    SamplerAlreadyRunning,
    /// The candidate never announced its start.
    Timeout,
}

/// The pre-spawn digest check has its own proof, and the discriminator is the **attempt
/// count**, not the value. The sampler is rewritten after the harness hashed it and
/// restored later in the run, so:
///
/// * with the pre-spawn comparison, the attempt is refused before any spawn and the
///   observation records `working_set_attempts == 0`;
/// * without it, the sample is spawned and its *post*-spawn comparison also sees the
///   rewritten file (the sample's post-check runs roughly 1.7 s into the run, well before
///   the 3 s restore), so it discards the value with `attempts == 1`.
///
/// Both paths end `not_measured` / `sample_tool_changed` with no value, so the token alone
/// does not distinguish the call sites -- only `working_set_attempts == 0` does. The
/// restore at roughly 3 s does not rescue the missing pre-spawn comparison either: it
/// happens after a spawned sample's post-check at roughly 1.7 s, so the swap would still
/// be discarded, merely with `attempts == 1`.
///
/// Ordering is causal, not a sleep: the harness hashes the tool, then spawns the
/// candidate, and the candidate's sentinel is what releases the rewrite, so the rewrite
/// is provably after the hash. The first attempt is scheduled at 1.5 s and the restore at
/// roughly 3 s, so the attempt falls inside the rewritten window. Scheduling margin: the
/// 1.5 s delay is what keeps the rewrite ahead of the attempt; under extreme load a
/// candidate that takes longer than ~1.5 s to start would let the attempt precede the
/// rewrite, in which case the pre-spawn comparison legitimately passes, a sample runs and
/// this test fails as a false negative. That margin is a deliberate trade against a
/// longer test, not a correctness assumption of the harness.
#[test]
fn pre_spawn_digest_check_refuses_a_rewritten_then_restored_tool() {
    let fixture = Fixture::new();
    let tool = fixture.base.join("tool.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &tool).unwrap();
    let original = std::fs::read(&tool).unwrap();
    let sentinel = fixture.base.join("candidate-started");
    fixture.case("a.png", "announce-sleep");

    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    config.sample_tool = Some(tool.clone());
    config.sample_argv_prefix = vec![
        std::ffi::OsString::from("--exact"),
        std::ffi::OsString::from("fake_sampler_valid_helper"),
        std::ffi::OsString::from("--nocapture"),
        std::ffi::OsString::from("--quiet"),
    ];
    // The first attempt is late enough that the rewrite normally precedes it; the restore
    // is later still, so the rewritten state is what the pre-spawn check sees. The 1.5 s
    // margin is the false-failure risk documented above, not a guarantee.
    config.sample_interval_ms = 1_500;
    config.cancel_after_ms = Some(5_000);

    let mutator = {
        let tool = tool.clone();
        let original = original.clone();
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !sentinel.exists() {
                if std::time::Instant::now() > deadline {
                    return Window::Timeout;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            // Rewrite while the harness is running, then restore before the run ends.
            let mut rewritten = original.clone();
            rewritten.extend_from_slice(b"rewritten-during-the-run");
            if std::fs::write(&tool, &rewritten).is_err() {
                // The sampler is already executing this image: the attempt was not
                // refused before the spawn, which is exactly what this test forbids.
                return Window::SamplerAlreadyRunning;
            }
            std::thread::sleep(std::time::Duration::from_millis(3_000));
            if std::fs::write(&tool, &original).is_err() {
                return Window::SamplerAlreadyRunning;
            }
            Window::Opened
        })
    };
    let report = harness::run(&config).unwrap();
    match mutator.join().unwrap() {
        Window::Opened => {}
        Window::SamplerAlreadyRunning => panic!(
            "the sampler was already executing the rewritten file: the pre-spawn digest \
             comparison did not refuse the attempt"
        ),
        Window::Timeout => panic!("the candidate never announced its start"),
    }

    // The file is back to its original bytes, so the run ended in the restored state; the
    // attempt count below is still what distinguishes the two call sites.
    assert_eq!(
        std::fs::read(&tool).unwrap(),
        original,
        "the sampler file was restored"
    );

    let observation = &report.observations[0];
    assert_eq!(
        observation.working_set_attempts, 0,
        "the attempt was refused before any spawn"
    );
    assert_eq!(
        observation.working_set_samples, 0,
        "no value from an unverified tool"
    );
    assert_eq!(observation.working_set_max_bytes, None);
    assert_eq!(observation.working_set_sampling, "not_measured");
    assert_eq!(observation.working_set_reason, Some("sample_tool_changed"));
}

/// A1: the parser table. Every row shape is either strictly a tasklist row for the
/// supervised pid, or it is refused — never turned into a byte count.
#[test]
fn working_set_parser_accepts_only_well_formed_rows() {
    let pid = 4_242_u32;
    let cases: Vec<(&str, Option<u64>)> = vec![
        // valid rows
        (
            "\"System\",\"4242\",\"Services\",\"0\",\"6,408 K\"",
            Some(6_408 * 1024),
        ),
        (
            "\"pwsh.exe\",\"4242\",\"Console\",\"1\",\"12,000 K\"",
            Some(12_000 * 1024),
        ),
        (
            "\"a.exe\",\"4242\",\"Console\",\"1\",\"12000 K\"",
            Some(12_000 * 1024),
        ),
        ("\"a.exe\",\"4242\",\"Console\",\"1\",\"0 K\"", Some(0)),
        // OEM-byte image name decoded lossily: the row is still valid
        (
            "\"\u{00ea}\u{00e3}x\u{00e3}mple.exe\",\"4242\",\"Console\",\"1\",\"3,000 K\"",
            Some(3_000 * 1024),
        ),
        // same row with surrounding noise (a header line first): the valid row wins
        (
            "\"Image Name\",\"PID\",\"Session Name\",\"Session#\",\"Mem Usage\"\n\
             \"a.exe\",\"4242\",\"Console\",\"1\",\"7,500 K\"",
            Some(7_500 * 1024),
        ),
        // foreign pid
        ("\"a.exe\",\"4243\",\"Console\",\"1\",\"12,000 K\"", None),
        // short row
        ("\"a.exe\",\"4242\"", None),
        // row without a memory column
        ("\"a.exe\",\"4242\",\"Console\",\"1\"", None),
        // localized / renamed column header
        (
            "\"a.exe\",\"4242\",\"Console\",\"1\",\"Speicher 12.000 K\"",
            None,
        ),
        // last field is a session number, not memory
        ("\"a.exe\",\"4242\",\"Console\",\"1\",\"2\"", None),
        // fabricated extra column: the 999,999 K is not a Mem Usage column
        (
            "\"a.exe\",\"4242\",\"Console\",\"1\",\"12,000 K\",\"999,999 K\"",
            None,
        ),
        // malformed digit grouping
        ("\"a.exe\",\"4242\",\"Console\",\"1\",\"12,00 K\"", None),
        ("\"a.exe\",\"4242\",\"Console\",\"1\",\"1,2345 K\"", None),
        // negative-looking and unit-less forms
        ("\"a.exe\",\"4242\",\"Console\",\"1\",\"-12 K\"", None),
        ("\"a.exe\",\"4242\",\"Console\",\"1\",\"12000\"", None),
        // empty output
        ("", None),
    ];
    for (line, expected) in cases {
        let parsed = harness::parse_working_set_for_tests(line, pid);
        assert_eq!(
            parsed, expected,
            "line {line:?} parsed as {parsed:?}, expected {expected:?}"
        );
    }
}

/// A1/A4: an injected sampler that prints a valid row for the supervised pid yields a
/// sampled working set; one that reports a foreign pid is `sample_parse_failed`.
#[test]
fn injected_sampler_valid_row_is_measured() {
    let fixture = Fixture::new();
    fixture.case("a.png", "sleep-long");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    inject_sampler(&mut config, "fake_sampler_valid_helper");
    config.sample_interval_ms = 10;
    config.cancel_after_ms = Some(2_000);
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert!(observation.working_set_attempts >= 1);
    assert!(observation.working_set_samples >= 1);
    assert_eq!(observation.working_set_sampling, "sampled_lower_bound");
    assert_eq!(observation.working_set_max_bytes, Some(12_000 * 1024));
    assert_eq!(observation.working_set_reason, None);
}

/// A4: a sampler that reports a foreign pid is refused as `sample_parse_failed`.
#[test]
fn injected_sampler_foreign_pid_is_sample_parse_failed() {
    let fixture = Fixture::new();
    fixture.case("a.png", "sleep-long");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    inject_sampler(&mut config, "fake_sampler_foreign_helper");
    config.sample_interval_ms = 10;
    config.cancel_after_ms = Some(1_500);
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert_eq!(observation.working_set_sampling, "not_measured");
    assert_eq!(observation.working_set_reason, Some("sample_parse_failed"));
    assert_eq!(
        observation.working_set_max_bytes, None,
        "never a fabricated value"
    );
    assert_eq!(observation.working_set_samples, 0);
}

/// A4: a missing sampler tool is `sample_tool_missing`, never a value.
#[test]
fn missing_sample_tool_is_not_measured() {
    let fixture = Fixture::new();
    fixture.case("a.png", "sleep-long");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    config.invocation = helper_invocation(HELPER);
    config.sample_tool = Some(fixture.base.join("no-such-sampler.exe"));
    config.sample_interval_ms = 10;
    config.cancel_after_ms = Some(1_000);
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert_eq!(observation.working_set_sampling, "not_measured");
    assert_eq!(
        observation.working_set_reason,
        Some("sample_tool_unreadable")
    );
    assert_eq!(observation.working_set_max_bytes, None);
}

/// A4: a sampler that never returns hits the per-sample timeout, is killed and reaped,
/// and the observation records `sample_timeout`.
#[test]
fn hanging_sampler_hits_the_sample_timeout() {
    let fixture = Fixture::new();
    fixture.case("a.png", "sleep-long");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    inject_sampler(&mut config, "fake_sampler_hang_helper");
    config.sample_interval_ms = 10;
    config.cancel_after_ms = Some(3_000);
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    assert_eq!(observation.working_set_sampling, "not_measured");
    assert_eq!(observation.working_set_reason, Some("sample_timeout"));
    assert_eq!(observation.working_set_samples, 0);
    assert_eq!(
        observation.working_set_attempts, 1,
        "one attempt, then no retry"
    );
}

/// A2/NIT: the cap bounds attempts and really stops spawning. The counter file proves
/// how many times the sampler program was started, independent of what it printed, and
/// the child lives far longer than the two allowed attempts.
#[test]
fn sample_cap_bounds_attempts_and_stops_spawning() {
    let _guard = sampling_guard();
    let fixture = Fixture::new();
    let sentinel = fixture.base.join("sampler.done");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    // The completion sentinel is also the spawn log: one line per sampler run, written by
    // the sampler itself, so the count is independent of the harness's bookkeeping.
    await_sampler(&fixture, &mut config, "fake_sampler_counting_helper", 2);
    config.max_samples = 2;
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    let spawns = std::fs::read_to_string(&sentinel)
        .map(|text| text.lines().count())
        .unwrap_or(0);
    assert_eq!(
        spawns, 2,
        "the sampler program ran exactly the allowed attempts"
    );
    assert_eq!(observation.working_set_attempts, 2);
    assert_eq!(
        observation.working_set_samples, 2,
        "both attempts succeeded"
    );
    assert_eq!(observation.working_set_sampling, "sampled_lower_bound");
    assert_eq!(
        observation.working_set_reason,
        Some("sample_capped"),
        "the cap, not a failure, stopped sampling"
    );
}

/// A2: a failure stops scheduling immediately, so the cap is never reached and
/// `sample_capped` is not claimed.
#[test]
fn sampler_failure_stops_scheduling_before_the_cap() {
    let _guard = sampling_guard();
    let fixture = Fixture::new();
    let sentinel = fixture.base.join("sampler.done");
    let mut config = harness::parse(arguments(&fixture, &[fixture.fake()])).unwrap();
    await_sampler(
        &fixture,
        &mut config,
        "fake_sampler_counting_garbage_helper",
        1,
    );
    config.max_samples = 5;
    let report = harness::run(&config).unwrap();

    let observation = &report.observations[0];
    let spawns = std::fs::read_to_string(&sentinel)
        .map(|text| text.lines().count())
        .unwrap_or(0);
    assert_eq!(spawns, 1, "one failing attempt, then no retry");
    assert_eq!(observation.working_set_attempts, 1);
    assert_eq!(observation.working_set_sampling, "not_measured");
    assert_eq!(observation.working_set_reason, Some("sample_parse_failed"));
    assert_ne!(
        observation.working_set_reason,
        Some("sample_capped"),
        "the cap did not stop sampling"
    );
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

    // NIT-1: the serialized byte budget and truncation are both explicit.
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

/// N6, updated for schema 10: the case suffix no longer reaches the output path (every
/// transport writes a canonical PNG), so the reason an extensionless case is refused is
/// the recorded input format, not an unusable output name: the manifest token is what
/// lets a row say which codec was measured, and without it nothing is claimed.
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

    // Sampling is disabled here so the only legitimately variable field left is
    // `wall_ms`; the module doc lists every field that may vary in a normal run.
    let mut config = harness::parse(arguments(&fixture, &candidates)).unwrap();
    config.invocation = helper_invocation(HELPER);
    config.sample_interval_ms = 0;
    let first = harness::run(&config).unwrap();
    harness::write_report(&first, &fixture.record("first")).unwrap();
    let second = harness::run(&config).unwrap();
    harness::write_report(&second, &fixture.record("second")).unwrap();

    let first_json = std::fs::read_to_string(fixture.record("first")).unwrap();
    let second_json = std::fs::read_to_string(fixture.record("second")).unwrap();
    assert_eq!(
        normalize_wall_ms(&first_json),
        normalize_wall_ms(&second_json),
        "the record must not depend on wall time for its structure"
    );
    assert_eq!(
        json_values(&first_json, "case_id"),
        vec!["a.png", "a.png", "b.png", "b.png"]
    );
    // The resources section carries one row per candidate, so `candidate_id` appears
    // in both arrays; the observation rows are the first four.
    let mut candidate_ids = json_values(&first_json, "candidate_id");
    candidate_ids.truncate(4);
    assert_eq!(candidate_ids, vec!["absent", "fake", "absent", "fake"]);
    assert!(first_json.contains("\"resources\": ["));
    assert!(first_json.contains("\"schema_version\": 12"));
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

/// Run the harness until at least one sample landed, tolerating the documented
/// `sample_timeout`/`no_sample` safe failures: a heavily loaded machine can exceed the
/// harness's per-sample bound, and that is a recorded `not_measured`, not a defect. The
/// caller still asserts the real proof (a value, a digest) on the successful run.
fn run_until_sampled(config: &harness::Config, attempts: usize) -> harness::Report {
    let mut last = None;
    for _ in 0..attempts {
        let report = harness::run(config).unwrap();
        if report
            .observations
            .iter()
            .any(|observation| observation.working_set_samples >= 1)
        {
            return report;
        }
        last = Some(report);
    }
    last.expect("at least one run was attempted")
}

/// The real system tasklist, the program an operator passes with `--sample-tool`.
fn system_tasklist() -> std::path::PathBuf {
    std::path::Path::new(&std::env::var("SystemRoot").unwrap())
        .join("System32")
        .join("tasklist.exe")
}

/// Sampling tests spawn this test binary as the sampler. Run concurrently on a loaded
/// machine, several of those spawns can exceed the harness's per-sample bound (a
/// documented `sample_timeout`), so the handshake tests take this lock and measure one at a
/// time; the serialisation is test-local and does not change what they prove.
static SAMPLING_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn sampling_guard() -> std::sync::MutexGuard<'static, ()> {
    SAMPLING_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Wire a sampling test so the candidate cannot exit before the sampler has finished:
/// the test binary becomes the sample tool (with the sentinel as its extra argv element),
/// the candidate waits for `samples` completions, and the run ends when the candidate
/// exits instead of at a deadline. Nothing here depends on how fast the machine is.
fn await_sampler(fixture: &Fixture, config: &mut harness::Config, helper: &str, samples: usize) {
    // The requested count *is* the candidate mode, so a test cannot ask for two samples
    // and then run a candidate that waits for one.
    fixture.case(
        "a.png",
        if samples >= 2 {
            "await2-sampler"
        } else {
            "await-sampler"
        },
    );
    let sentinel = fixture.base.join("sampler.done");
    let _ = std::fs::remove_file(&sentinel);
    inject_sampler(config, helper);
    config.sample_argv_prefix.push(sentinel.into_os_string());
    config.sample_interval_ms = 10;
    // Upper bounds only: the candidate ends the run once the samples have landed, and
    // neither the child timeout nor the cancellation deadline may cut it short.
    config.timeout_ms = 60_000;
    config.cancel_after_ms = Some(60_000);
}

/// Point the harness at an in-test sampler helper: the test binary becomes the sample
/// tool, with `--exact <helper>` as its argv prefix (the same seam shape as the
/// candidate `Invocation::argv_prefix`).
fn inject_sampler(config: &mut harness::Config, helper: &str) {
    config.sample_tool = Some(std::env::current_exe().unwrap());
    config.sample_argv_prefix = vec![
        OsString::from("--exact"),
        OsString::from(helper),
        OsString::from("--nocapture"),
        OsString::from("--quiet"),
    ];
    config.invocation = helper_invocation(HELPER);
}

/// Read the supervised pid out of the fixed `/FI "PID eq <id>"` argv the harness uses.
fn sampler_pid() -> u32 {
    let args: Vec<String> = std::env::args().collect();
    for (index, argument) in args.iter().enumerate() {
        if argument == "/FI"
            && let Some(filter) = args.get(index + 1)
            && let Some(rest) = filter.strip_prefix("PID eq ")
            && let Ok(pid) = rest.trim().parse::<u32>()
        {
            return pid;
        }
    }
    0
}

/// Count this spawn and report whether it is the first one (the counter file starts
/// empty), so a helper can behave differently on its first and later invocations.
fn count_spawn_first() -> bool {
    let Some(path) = sampler_counter_path() else {
        return true;
    };
    let first = std::fs::read(&path)
        .map(|bytes| bytes.is_empty())
        .unwrap_or(true);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write as _;
        let _ = file.write_all(b"x");
    }
    first
}

/// A sampler that succeeds on its first invocation and exits non-zero afterwards, so a
/// value already exists when the failure arrives.
#[test]
fn fake_sampler_success_then_failure_helper() {
    let pid = sampler_pid();
    if pid == 0 {
        return;
    }
    let first = count_spawn_first();
    println!("\"fake.exe\",\"{pid}\",\"Console\",\"1\",\"12,000 K\"");
    if !first {
        std::process::exit(1);
    }
}

/// A sampler that succeeds once and then overflows the capture on later invocations.
#[test]
fn fake_sampler_success_then_truncating_helper() {
    let pid = sampler_pid();
    if pid == 0 {
        return;
    }
    let first = count_spawn_first();
    println!("\"fake.exe\",\"{pid}\",\"Console\",\"1\",\"12,000 K\"");
    if !first {
        let padding = "x".repeat(1024);
        for _ in 0..16 {
            println!("{padding}");
        }
    }
}

/// A sampler that prints a perfectly valid row and then exits non-zero: the row is not
/// evidence, because the program that produced it failed.
#[test]
fn fake_sampler_failing_helper() {
    let pid = sampler_pid();
    if pid == 0 {
        return;
    }
    println!("\"fake.exe\",\"{pid}\",\"Console\",\"1\",\"12,000 K\"");
    mark_sampler_done();
    std::process::exit(1);
}

/// A sampler that prints a valid row and then floods the capture, so the row sits in a
/// truncated buffer and must be refused.
#[test]
fn fake_sampler_truncating_helper() {
    let pid = sampler_pid();
    if pid == 0 {
        return;
    }
    println!("\"fake.exe\",\"{pid}\",\"Console\",\"1\",\"12,000 K\"");
    mark_sampler_done();
    let padding = "x".repeat(1024);
    for _ in 0..16 {
        println!("{padding}");
    }
}

/// A sampler that replaces its own program file while it runs and still emits a valid
/// row, so only the post-spawn digest check can catch it.
#[test]
fn fake_sampler_replacing_helper() {
    let pid = sampler_pid();
    if pid == 0 {
        return;
    }
    if let Ok(own) = std::env::current_exe() {
        let retired = own.with_extension("retired");
        let _ = std::fs::rename(&own, &retired);
        let _ = std::fs::write(&own, b"a different program");
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    println!("\"fake.exe\",\"{pid}\",\"Console\",\"1\",\"12,000 K\"");
    mark_sampler_done();
}

/// A sampler that prints the exact Windows row for the supervised pid and announces
/// completion. When it runs as an ordinary test (no `/FI` argv) it does nothing, so the
/// suite is unaffected.
#[test]
fn fake_sampler_valid_helper() {
    let pid = sampler_pid();
    if pid == 0 {
        return;
    }
    println!("\"fake.exe\",\"{pid}\",\"Console\",\"1\",\"12,000 K\"");
    mark_sampler_done();
}

/// Append one byte to the counter file passed as the last argument, so the test can
/// count real spawns independently of the printed row.
fn sampler_counter_path() -> Option<std::path::PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    let index = args.iter().position(|argument| argument == "/FI")?;
    // The counter is the extra argv element injected right before the fixed sample
    // argv; a bare `--quiet` (no counter injected) is not a path.
    let candidate = args.get(index.checked_sub(1)?)?;
    candidate
        .ends_with(".txt")
        .then(|| std::path::PathBuf::from(candidate))
}

/// The completion sentinel passed as the extra argv element right before `/FI`, so the
/// sampler can tell the candidate that it has finished. This is what removes the timing
/// dependency: the candidate cannot exit until the sampler has actually run.
fn sampler_done_path() -> Option<std::path::PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    let index = args.iter().position(|argument| argument == "/FI")?;
    let candidate = args.get(index.checked_sub(1)?)?;
    candidate
        .ends_with(".done")
        .then(|| std::path::PathBuf::from(candidate))
}

/// Announce one finished sample; a line per run lets a candidate wait for the second one.
fn mark_sampler_done() {
    if let Some(path) = sampler_done_path() {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        use std::io::Write as _;
        let _ = file.write_all(b"x\n");
    }
}

/// Wait until at least `samples` samples have announced completion, then linger briefly so
/// the harness harvests the last one before the candidate exits. The bound only stops a
/// broken helper from hanging the suite; it is never the reason the test passes.
fn await_samples(sentinel: &std::path::Path, samples: usize) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(60) {
        if let Ok(text) = std::fs::read_to_string(sentinel)
            && text.lines().count() >= samples
        {
            // One poll interval is enough for the harness to reap the sample; a short
            // linger keeps the window below the attempt cap of the positive control.
            std::thread::sleep(std::time::Duration::from_millis(150));
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

fn count_spawn() {
    if let Some(path) = sampler_counter_path() {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        use std::io::Write as _;
        let _ = file.write_all(b"x");
    }
}

/// A sampler that counts its spawns and prints a valid row.
#[test]
fn fake_sampler_counting_helper() {
    let pid = sampler_pid();
    if pid == 0 {
        return;
    }
    count_spawn();
    println!("\"fake.exe\",\"{pid}\",\"Console\",\"1\",\"12,000 K\"");
    mark_sampler_done();
}

/// A sampler that counts its spawns and prints an unparseable row.
#[test]
fn fake_sampler_counting_garbage_helper() {
    if sampler_pid() == 0 {
        return;
    }
    count_spawn();
    println!("\"fake.exe\",\"1\",\"Console\",\"1\",\"twelve megabytes\"");
    mark_sampler_done();
}

/// A sampler whose row is garbage.
#[test]
fn fake_sampler_garbage_helper() {
    if sampler_pid() == 0 {
        return;
    }
    println!("\"fake.exe\",\"1\",\"Console\",\"1\",\"twelve megabytes\"");
}

/// A sampler that reports a foreign pid.
#[test]
fn fake_sampler_foreign_helper() {
    let pid = sampler_pid();
    if pid == 0 {
        return;
    }
    println!(
        "\"fake.exe\",\"{}\",\"Console\",\"1\",\"12,000 K\"",
        pid + 1
    );
}

/// A sampler that never finishes inside the per-sample timeout.
#[test]
fn fake_sampler_hang_helper() {
    if sampler_pid() == 0 {
        return;
    }
    std::thread::sleep(std::time::Duration::from_secs(10));
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
        // B-R1-01: structurally short containers that must never verify.
        "png-no-idat" => {
            std::fs::write(&output, png_without_idat(16, 16)).unwrap();
            0
        }
        "jpeg-no-eoi" => {
            std::fs::write(&output, jpeg_without_eoi(24, 12, 3)).unwrap();
            0
        }
        "png-iend-no-crc" => {
            std::fs::write(&output, png_iend_without_crc(16, 16)).unwrap();
            0
        }
        "alloc-64m" => {
            // A known allocation the parent's sampler must see (at least) as its
            // working set, held long enough for a few samples.
            // Filled, not just reserved: zero pages are not resident, so the sampled
            // working set would otherwise stay far below the allocation.
            let block = vec![1_u8; 16 * 1024 * 1024];
            std::fs::write(&output, png_bytes(16, 16, 2, false)).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1_200));
            drop(block);
            0
        }
        "await-sampler" | "await2-sampler" => {
            // Stay alive until the sampler has actually finished (one or two samples), so
            // no assertion depends on how fast the machine schedules a spawn.
            let samples = if marker == "await2-sampler" { 2 } else { 1 };
            let sentinel = std::path::Path::new(&input)
                .parent()
                .and_then(|cases| cases.parent())
                .map(|base| base.join("sampler.done"));
            let Some(sentinel) = sentinel else {
                std::process::exit(9);
            };
            if !await_samples(&sentinel, samples) {
                // A helper that never announces completion must not hang the suite.
                std::process::exit(9);
            }
            std::fs::write(&output, png_bytes(16, 16, 2, false)).unwrap();
            0
        }
        "announce-sleep" => {
            // Announce that the candidate started (which happens strictly after the
            // harness hashed the sampler), then outlive the deadline.
            if let Some(base) = std::path::Path::new(&input)
                .parent()
                .and_then(|dir| dir.parent())
            {
                std::fs::write(base.join("candidate-started"), b"started").unwrap();
            }
            std::fs::write(&output, png_bytes(16, 16, 2, false)).unwrap();
            std::thread::sleep(std::time::Duration::from_secs(5));
            0
        }
        "mutate-tool" => {
            // The sampler program sits one level above the case directory; rewrite it
            // before the first sample attempt so the digest no longer matches.
            if let Some(base) = std::path::Path::new(&input)
                .parent()
                .and_then(|dir| dir.parent())
            {
                let tool = base.join("tool.exe");
                if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(&tool) {
                    use std::io::Write as _;
                    let _ = file.write_all(b"mutated-by-the-candidate");
                }
            }
            std::fs::write(&output, png_bytes(16, 16, 2, false)).unwrap();
            // Outlive the harness deadline, so the first sample attempt is scheduled
            // while this child is alive no matter how loaded the machine is.
            std::thread::sleep(std::time::Duration::from_secs(5));
            0
        }
        "reason-cancelled" => {
            println!("{{\"reason_code\":\"cancelled_at_cancel\"}}");
            0
        }
        "cancel-hang" => {
            // Claims cancellation, then hangs: the harness kills it, so the observed
            // class really is `cancelled` and the claim must be recorded.
            println!("{{\"reason_code\":\"cancelled_at_cancel\"}}");
            std::thread::sleep(std::time::Duration::from_secs(5));
            0
        }
        "sleep-long" => {
            std::thread::sleep(std::time::Duration::from_secs(5));
            0
        }
        "png-rgb-a" => {
            std::fs::write(&output, png_rgb_16(10)).unwrap();
            0
        }
        // ACTIONABLE shape (a): write a complete IHDR and then an unfinished chunk, and
        // outlive the deadline. The output file exists but is not a complete container, and
        // the observed class is `cancelled` — the digest must still carry a reason.
        "partial-sleep" => {
            let mut bytes = png_signature();
            bytes.extend_from_slice(&png_ihdr(16, 16, 2, 8));
            bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x20]);
            std::fs::write(&output, bytes).unwrap();
            std::thread::sleep(std::time::Duration::from_secs(5));
            0
        }
        // Schema 10: behaves like the real transports — it writes the container the
        // requested output name implies. If the harness ever names the output after the
        // case suffix, this leg writes BMP and can no longer be verified, which is exactly
        // the gap the canonical PNG transport closes.
        "suffix-honest" => {
            let requested_png = std::path::Path::new(&output)
                .extension()
                .and_then(|extension| extension.to_str())
                == Some("png");
            let bytes = if requested_png {
                png_rgb_16(10)
            } else {
                bmp_bytes(16, 16, 24)
            };
            std::fs::write(&output, bytes).unwrap();
            0
        }
        "png-rgb-b" => {
            std::fs::write(&output, png_rgb_16(200)).unwrap();
            0
        }
        "png-rgba-alpha" => {
            std::fs::write(&output, png_rgba_16(10, &[])).unwrap();
            0
        }
        "png-rgba-low-a" => {
            std::fs::write(&output, png_rgba_16(10, &[(0, 128)])).unwrap();
            0
        }
        "png-rgba-one-a" => {
            std::fs::write(&output, png_rgba_16(10, &[(137, 0)])).unwrap();
            0
        }
        "png-corrupt-idat" => {
            let mut bytes = png_signature();
            bytes.extend_from_slice(&png_ihdr(16, 16, 6, 8));
            bytes.extend_from_slice(&png_chunk(b"IDAT", &[0x78, 0x9C, 0xFF, 0x00, 0x13, 0x37]));
            bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
            std::fs::write(&output, bytes).unwrap();
            0
        }
        "png-huge-dims" => {
            let mut bytes = png_signature();
            bytes.extend_from_slice(&png_ihdr(40_000, 40_000, 6, 8));
            bytes.extend_from_slice(&png_chunk(b"IDAT", &zlib_stored(&[0_u8; 8])));
            bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
            std::fs::write(&output, bytes).unwrap();
            0
        }
        "png-inflate-bomb" => {
            let mut bytes = png_signature();
            bytes.extend_from_slice(&png_ihdr(16, 16, 6, 8));
            // Declared geometry needs 16 * 65 = 1040 bytes; this stream carries 200 KiB.
            bytes.extend_from_slice(&png_chunk(b"IDAT", &zlib_stored(&vec![0_u8; 200 * 1024])));
            bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
            std::fs::write(&output, bytes).unwrap();
            0
        }
        "jpeg-eoi-in-sos" => {
            std::fs::write(&output, jpeg_eoi_inside_sos(16, 16, 3)).unwrap();
            0
        }
        "jpeg-sof-nodesc" => {
            std::fs::write(&output, jpeg_sof_without_descriptors(16, 16)).unwrap();
            0
        }
        "jpeg-sos-short" => {
            std::fs::write(&output, jpeg_sos_short_length(24, 12, 3)).unwrap();
            0
        }
        "jpeg-sos-nolen" => {
            std::fs::write(&output, jpeg_sos_without_length(24, 12, 3)).unwrap();
            0
        }
        "bmp-short-pixels" => {
            std::fs::write(&output, bmp_short_pixels(8, 4, 24)).unwrap();
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
        "reason-budget" => {
            println!(
                "{{\"reason_code\":\"max_dimension_exceeded\",\"admission_status\":\"refused\",\
                 \"materialization\":\"not_requested\",\"wic_internal_scratch\":\"not_measured\"}}"
            );
            4
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
        // NB-2: bounded output crosses the threshold, then only the supervisor can
        // end this live child. A finite flood cannot establish that kill boundary.
        "floodcap" => {
            let mut stdout = std::io::stdout().lock();
            if stdout.write_all(&[b'x'; 1024]).is_err() || stdout.flush().is_err() {
                std::process::exit(1);
            }
            loop {
                std::thread::park();
            }
        }
        _ => 1,
    };
    std::process::exit(code);
}

/// Replace every documented volatile field's value with a placeholder, so two runs can
/// be compared for structural determinism.
fn normalize_volatile(json: &str) -> String {
    let mut text = json.to_owned();
    for field in [
        "\"wall_ms\":",
        "\"scratch_max_bytes\":",
        "\"working_set_samples\":",
        "\"working_set_max_bytes\":",
        "\"working_set_attempts\":",
        "\"working_set_sampling\":",
        "\"working_set_reason\":",
        "\"cancellation_requested_at_ms\":",
        "\"cancellation_observed_ms\":",
    ] {
        let mut search_from = 0;
        while let Some(offset) = text[search_from..].find(field) {
            let start = search_from + offset + field.len();
            let rest = &text[start..];
            let trimmed = rest.trim_start();
            let skip = rest.len() - trimmed.len();
            let end = trimmed
                .find([',', '}'])
                .map(|index| index + skip)
                .unwrap_or(0);
            text.replace_range(start..start + end, " volatile");
            search_from = start + " volatile".len();
        }
    }
    text
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

/// Adler-32 as zlib defines it, so the crafted streams are real zlib streams.
fn adler32(raw: &[u8]) -> u32 {
    let mut first = 1_u32;
    let mut second = 0_u32;
    for &byte in raw {
        first = (first + u32::from(byte)) % 65521;
        second = (second + first) % 65521;
    }
    (second << 16) | first
}

/// A zlib stream made of stored (uncompressed) DEFLATE blocks.
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut chunks: Vec<&[u8]> = raw.chunks(65_535).collect();
    if chunks.is_empty() {
        chunks.push(&[]);
    }
    for (index, chunk) in chunks.iter().enumerate() {
        let last = index + 1 == chunks.len();
        out.push(u8::from(last));
        let length = u16::try_from(chunk.len()).unwrap();
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

/// A PNG whose pixels are given as raw scanline bytes per row (filter 0).
fn png_with_pixels(width: u32, height: u32, colour_type: u8, rows: &[Vec<u8>]) -> Vec<u8> {
    assert_eq!(rows.len() as u32, height);
    let mut raw = Vec::new();
    for row in rows {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut bytes = png_signature();
    bytes.extend_from_slice(&png_ihdr(width, height, colour_type, 8));
    bytes.extend_from_slice(&png_chunk(b"IDAT", &zlib_stored(&raw)));
    bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
    bytes
}

/// A 16x16 RGB PNG filled with `seed`-derived pixels.
fn png_rgb_16(seed: u8) -> Vec<u8> {
    let rows: Vec<Vec<u8>> = (0..16)
        .map(|y| {
            let mut row = Vec::with_capacity(48);
            for x in 0..16_u8 {
                row.push(seed.wrapping_add(x));
                row.push(seed.wrapping_add(y));
                row.push(seed ^ x ^ y);
            }
            row
        })
        .collect();
    png_with_pixels(16, 16, 2, &rows)
}

/// The RGBA expansion the oracle must produce for [`png_rgb_16`].
fn rgba_rgb_16(seed: u8) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16_u8 {
        for x in 0..16_u8 {
            pixels.extend_from_slice(&[
                seed.wrapping_add(x),
                seed.wrapping_add(y),
                seed ^ x ^ y,
                255,
            ]);
        }
    }
    pixels
}

/// A 16x16 RGBA PNG with the pixels of [`png_rgb_16`] and one alpha override.
fn png_rgba_16(seed: u8, overrides: &[(usize, u8)]) -> Vec<u8> {
    let rows: Vec<Vec<u8>> = (0..16)
        .map(|y: u8| {
            let mut row = Vec::with_capacity(64);
            for x in 0..16_u8 {
                row.push(seed.wrapping_add(x));
                row.push(seed.wrapping_add(y));
                row.push(seed ^ x ^ y);
                let index = usize::from(y) * 16 + usize::from(x);
                let alpha = overrides
                    .iter()
                    .find(|(pixel, _)| *pixel == index)
                    .map_or(255, |(_, alpha)| *alpha);
                row.push(alpha);
            }
            row
        })
        .collect();
    png_with_pixels(16, 16, 6, &rows)
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
    // B-R1-01: the file must really carry the declared pixel buffer, so the harness
    // can prove `data_offset + row_bytes * height <= file_size`.
    let row_bytes = (u64::from(width.unsigned_abs()) * u64::from(bit_count)).div_ceil(32) * 4;
    let pixel_bytes = row_bytes * u64::from(height.unsigned_abs());
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
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.resize(bytes.len() + usize::try_from(pixel_bytes).unwrap(), 0);
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

/// Minimal structurally valid JPEG: SOI, SOF0, SOS, a few scan bytes and EOI.
fn jpeg_bytes(width: u16, height: u16, components: u8) -> Vec<u8> {
    let mut bytes = jpeg_sof_only(width, height, components);
    let mut scan = Vec::new();
    scan.push(components);
    for index in 0..components {
        scan.extend_from_slice(&[index + 1, 0x00]);
    }
    scan.extend_from_slice(&[0, 63, 0]);
    bytes.extend_from_slice(&[0xFF, 0xDA]);
    bytes.extend_from_slice(&u16::try_from(scan.len() + 2).unwrap().to_be_bytes());
    bytes.extend_from_slice(&scan);
    // Entropy-coded bytes: the harness checks framing only, never this content.
    bytes.extend_from_slice(&[0x12, 0x34, 0x56]);
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// IHDR and IEND with no image data at all (B-R1-01).
fn png_without_idat(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = png_signature();
    bytes.extend_from_slice(&png_ihdr(width, height, 2, 8));
    bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
    bytes
}

/// IHDR and a non-empty IDAT with real CRCs, then an IEND whose 4-byte CRC is cut
/// off, so the terminator chunk is not fully inside the file (B-R1-01 residual).
fn png_iend_without_crc(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = png_signature();
    bytes.extend_from_slice(&png_ihdr(width, height, 2, 8));
    bytes.extend_from_slice(&png_chunk(
        b"IDAT",
        &[0x78, 0x9C, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
    ));
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes.extend_from_slice(b"IEND");
    bytes
}

/// A JPEG with a valid SOF followed by `FF DA FF D9`: the scan marker has no length
/// field, so it is not a well-formed SOS segment (B-R1-01 residual).
fn jpeg_sos_without_length(width: u16, height: u16, components: u8) -> Vec<u8> {
    let mut bytes = jpeg_sof_only(width, height, components);
    bytes.extend_from_slice(&[0xFF, 0xDA, 0xFF, 0xD9]);
    bytes
}

/// A valid SOF followed by `FF DA 00 08 01 01 00 00 FF D9`: the scan header is
/// internally consistent (`Ls = 8 = 6 + 2*Ns`) but its last two payload bytes are
/// `FF D9`, so the only `EOI` in the file sits inside the SOS segment (B-R3-01).
fn jpeg_eoi_inside_sos(width: u16, height: u16, components: u8) -> Vec<u8> {
    let mut bytes = jpeg_sof_only(width, height, components);
    bytes.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0xFF, 0xD9]);
    bytes
}

/// A SOF that declares three components but carries no component descriptors
/// (`payload = 6`), followed by a well-formed scan and a real `EOI` (B-R3-01).
fn jpeg_sof_without_descriptors(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8];
    let mut payload = vec![8_u8];
    payload.extend_from_slice(&height.to_be_bytes());
    payload.extend_from_slice(&width.to_be_bytes());
    payload.push(3);
    bytes.extend_from_slice(&[0xFF, 0xC0]);
    bytes.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
    bytes.extend_from_slice(&payload);
    bytes.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
    bytes.extend_from_slice(&[0x12, 0x34]);
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// A valid SOF followed by exactly `FF DA 00 03 01 FF D9`: the scan header declares
/// `Ls = 3`, which cannot carry the required SOS fields (B-R2-01).
fn jpeg_sos_short_length(width: u16, height: u16, components: u8) -> Vec<u8> {
    let mut bytes = jpeg_sof_only(width, height, components);
    bytes.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x03, 0x01, 0xFF, 0xD9]);
    bytes
}

/// A JPEG that reaches its scan but never terminates it (B-R1-01).
fn jpeg_without_eoi(width: u16, height: u16, components: u8) -> Vec<u8> {
    let mut bytes = jpeg_bytes(width, height, components);
    bytes.truncate(bytes.len() - 2);
    bytes
}

/// A BMP whose declared pixel buffer is longer than the file (B-R1-01).
fn bmp_short_pixels(width: i32, height: i32, bit_count: u16) -> Vec<u8> {
    let mut bytes = bmp_bytes(width, height, bit_count);
    let trimmed = bytes.len() - 10;
    bytes.truncate(trimmed);
    bytes
}
