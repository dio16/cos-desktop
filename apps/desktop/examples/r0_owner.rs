//! MP-R0A-S2A development-only ownership helper. Never a production startup path.
//! Windows paths are pinned component-by-component with the existing safe filepath API.
//! The driver assigns this process to a kill-on-close job before sending `go`.

#[cfg(windows)]
#[path = "r0_owner/ownership.rs"]
mod ownership;
#[cfg(any(windows, test))]
#[path = "r0_owner/sha256.rs"]
mod sha256;

#[cfg(not(windows))]
fn main() {
    eprintln!("r0-owner is Windows-only; failing closed");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        // Paths, supplied values and native errors never enter public diagnostics.
        eprintln!("ownership helper failed; no successful lifecycle claim");
        if std::env::var_os("R0_OWNER_DEBUG").is_some() {
            eprintln!("debug: {error:?}");
        }
        std::process::exit(2);
    }
}

#[cfg(windows)]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use ownership::PinnedPath;
    use std::{
        io::{BufRead, Read, Write},
        path::PathBuf,
        process::{Child, ExitStatus, Stdio},
        sync::mpsc,
        time::{Duration, Instant},
    };

    fn wait_until(child: &mut Child, deadline: Instant) -> std::io::Result<Option<ExitStatus>> {
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(Some(status));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    // Error/timeout cleanup is bounded. The driver's job is the final crash/orphan boundary.
    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(Some(_))) {
                let _ = self.0.kill();
                let _ = wait_until(&mut self.0, Instant::now() + Duration::from_secs(2));
            }
        }
    }
    fn hex(value: &str, length: usize) -> bool {
        value.len() == length
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
    fn digest(value: &str) -> bool {
        value.strip_prefix("sha256:").is_some_and(|v| hex(v, 64))
    }

    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.as_slice() == ["selftest"] {
        ownership::selftest()?;
        return Ok(());
    }
    if args.len() != 17 || args[0] != "run" {
        return Err("invalid invocation".into());
    }
    let mut values = std::collections::BTreeMap::new();
    for pair in args[1..].chunks(2) {
        let (name, value) = pair.first().zip(pair.get(1)).ok_or("invalid option pair")?;
        if values.insert(name.as_str(), value.as_str()).is_some() {
            return Err("duplicate option".into());
        }
    }
    let root = PathBuf::from(*values.get("--root").ok_or("missing root")?);
    let profile = PathBuf::from(*values.get("--profile").ok_or("missing profile")?);
    let exe = PathBuf::from(*values.get("--exe").ok_or("missing exe")?);
    let fixture = PathBuf::from(*values.get("--fixture").ok_or("missing fixture")?);
    let approved = *values.get("--approved-sha256").ok_or("missing digest")?;
    let expected_fixture = *values
        .get("--fixture-sha256")
        .ok_or("missing fixture digest")?;
    let execution = *values.get("--execution-id").ok_or("missing execution id")?;
    let timeout: u64 = values
        .get("--timeout-ms")
        .ok_or("missing timeout")?
        .parse()?;
    if !digest(approved)
        || !digest(expected_fixture)
        || !hex(execution, 32)
        || !(1..=120_000).contains(&timeout)
        || exe.file_name().is_none_or(|name| name != "cos-desktop.exe")
    {
        return Err("invalid binding".into());
    }

    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut input = std::io::stdin().lock();
        for _ in 0..2 {
            let mut line = Vec::with_capacity(16);
            let result = input.by_ref().take(32).read_until(b'\n', &mut line);
            if sender.send(result.map(|_| line)).is_err() {
                break;
            }
        }
    });
    // No child can exist before the caller's JobObject assignment has succeeded.
    if receiver.recv_timeout(Duration::from_secs(5))?? != b"go-v2\n" {
        return Err("start handshake refused".into());
    }

    let root = PinnedPath::existing(&root, true)?;
    let names = |path: &std::path::Path| -> std::io::Result<std::collections::BTreeSet<std::ffi::OsString>> {
        std::fs::read_dir(path)?
            .map(|entry| entry.map(|e| e.file_name()))
            .collect()
    };
    if names(root.path())?
        != ["run-id"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect()
        || std::fs::read_to_string(root.path().join("run-id"))? != execution
    {
        return Err("output root is not this run".into());
    }
    let profile = PinnedPath::existing(&profile, true)?;
    if profile.path().file_name().and_then(|name| name.to_str()) != Some(execution)
        || names(profile.path())?
            != ["roaming", "temp", "run-id"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect()
        || std::fs::read_to_string(profile.path().join("run-id"))? != execution
        || names(&profile.path().join("roaming"))?
            .iter()
            .next()
            .is_some()
        || names(&profile.path().join("temp"))?.iter().next().is_some()
    {
        return Err("profile identity is not the per-run profile".into());
    }
    let roaming = PinnedPath::existing(&profile.path().join("roaming"), true)?;
    let temp = PinnedPath::existing(&profile.path().join("temp"), true)?;
    let mut executable = PinnedPath::existing(&exe, false)?;
    let mut fixture = PinnedPath::existing(&fixture, false)?;
    let (observed, bytes) = sha256::hash(&mut executable.file)?;
    let (fixture_sha, fixture_bytes) = sha256::hash(&mut fixture.file)?;
    if observed != approved || fixture_sha != expected_fixture {
        return Err("content binding refused".into());
    }
    executable.verify()?;
    fixture.verify()?;
    root.verify()?;

    let started = Instant::now();
    let mut child = OwnedChild(
        std::process::Command::new(executable.path())
            .arg("--fixture")
            .arg("--fixture-file")
            .arg(fixture.path())
            .env_clear()
            .env(
                "SystemRoot",
                std::env::var_os("SystemRoot").ok_or("missing SystemRoot")?,
            )
            .env("LOCALAPPDATA", profile.path())
            .env("APPDATA", roaming.path())
            .env("TEMP", temp.path())
            .env("TMP", temp.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let pid = child.0.id();
    let initial_status = wait_until(&mut child.0, started + Duration::from_millis(timeout))?;
    let timed_out = initial_status.is_none();
    let status = if let Some(status) = initial_status {
        status
    } else {
        // A concurrent normal exit is allowed; otherwise kill failure is not hidden.
        if child.0.kill().is_err() && child.0.try_wait()?.is_none() {
            return Err("termination failed".into());
        }
        wait_until(&mut child.0, Instant::now() + Duration::from_secs(2))?
            .ok_or("cleanup incomplete")?
    };
    let exit_code = status.code().ok_or("exit status unavailable")?;
    let elapsed_ms = started.elapsed().as_millis();
    let cleanup = "complete";
    let termination = if timed_out {
        "requested"
    } else {
        "not-requested"
    };
    executable.verify()?;
    fixture.verify()?;
    root.verify()?;
    println!(
        "{{\"schema_version\":2,\"execution_id\":\"{execution}\",\"pid\":{pid},\"exit_code\":{exit_code},\"timed_out\":{timed_out},\"approved\":true,\"executable_sha256\":\"{observed}\",\"executable_bytes\":{bytes},\"fixture_sha256\":\"{fixture_sha}\",\"fixture_bytes\":{fixture_bytes},\"elapsed_ms\":{elapsed_ms},\"child_reaped\":true,\"termination_status\":\"{termination}\",\"cleanup\":\"{cleanup}\",\"publication\":\"driver-owned\"}}"
    );
    std::io::stdout().flush()?;
    // Only the validated driver may release the ownership pins. A dead driver cannot hang us.
    if receiver.recv_timeout(Duration::from_secs(10))?? != b"release-v2\n" {
        return Err("publication handshake refused".into());
    }
    root.verify()?;
    Ok(())
}
