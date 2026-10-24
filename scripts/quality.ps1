# Runs the TESTING.md quality gates 1-5 plus the dependency-rules check, in documented order.
# Windows-first, repository root resolved from this script's location. The native startup
# smoke (gate 6) stays a recorded manual gate on this workstation and is not simulated here.
#
# Interpreter and toolchain resolution is proven, not assumed. Python must answer a bounded `-c`
# version probe before the dependency-rules gates run; its shell launchers stay allowed because each
# of those gates is also held to its own success line. cargo must be a real cargo.exe -- a .cmd/.bat
# wrapper is refused, because a batch file can answer `--version` and still no-op every subcommand --
# and it must answer both `cargo --version` and `cargo fmt --version`. Every cargo gate then runs
# through the proven absolute path and must emit the signature its command really prints (metadata
# JSON with `packages`, Checking/Compiling/Finished, `test result:`), so a silent no-op that exits 0
# cannot pass. A bare `python`/`cargo` lookup is not proof on Windows: PATH may put an extensionless
# POSIX launcher or a store alias first, which PowerShell cannot execute, so a gate built on it
# reports nothing and passes vacuously.
#
# Usage:  pwsh scripts/quality.ps1             # required quality gates
#         pwsh scripts/quality.ps1 -CheckOnly  # local feedback; no linking or tests

[CmdletBinding()]
param([switch]$CheckOnly)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

$script:PythonPath = $null
$script:CargoPath = $null


# Runs one concrete executable with a bounded wait and captured streams.
# ExitCode is $null when the process could not be started or the wait timed out.
function Invoke-BoundedProcess {
    param(
        [Parameter(Mandatory)][string]$FilePath,
        [string[]]$Arguments = @(),
        [int]$TimeoutMs = 15000,
        [string]$WorkingDirectory
    )

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    # Probe from an explicit directory: rustup and other tool dispatchers resolve the toolchain
    # relative to cwd, so inheriting the caller's cwd could prove a different cargo than the one
    # the gates use after Push-Location to the repository root.
    if ($WorkingDirectory) {
        $startInfo.WorkingDirectory = $WorkingDirectory
    }

    if ([System.IO.Path]::GetExtension($FilePath) -in @(".cmd", ".bat")) {
        # A shell launcher is a batch file, not a PE image: run it through cmd.exe so the
        # launcher itself (and whatever it forwards to) is what executes and reports the status.
        $startInfo.FileName = if ($env:ComSpec) { $env:ComSpec } else { Join-Path $env:SystemRoot "System32\cmd.exe" }
        $quotedArguments = ($Arguments | ForEach-Object { '"' + ($_ -replace '"', '""') + '"' }) -join ' '
        $startInfo.Arguments = '/d /c ""{0}" {1}"' -f $FilePath, $quotedArguments
    }
    else {
        $startInfo.FileName = $FilePath
        foreach ($argument in $Arguments) {
            $startInfo.ArgumentList.Add($argument)
        }
    }

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    try {
        if (-not $process.Start()) {
            return [pscustomobject]@{ ExitCode = $null; StdOut = ""; StdErr = "process did not start"; TimedOut = $false }
        }
        $stdOut = $process.StandardOutput.ReadToEndAsync()
        $stdErr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit($TimeoutMs)) {
            try { $process.Kill($true) } catch { try { $process.Kill() } catch { } }
            return [pscustomobject]@{ ExitCode = $null; StdOut = ""; StdErr = ""; TimedOut = $true }
        }
        $process.WaitForExit()
        return [pscustomobject]@{ ExitCode = $process.ExitCode; StdOut = $stdOut.Result; StdErr = $stdErr.Result; TimedOut = $false }
    }
    catch {
        return [pscustomobject]@{ ExitCode = $null; StdOut = ""; StdErr = $_.Exception.Message; TimedOut = $false }
    }
    finally {
        try { $process.Dispose() } catch { }
    }
}


# Returns the PATH directories in shell order. Plain filesystem probes are used because Test-Path
# throws for ACL-restricted PATH entries such as ...\config\systemprofile\...\WindowsApps.
function Get-PathDirectories {
    @(
        foreach ($entry in ($env:PATH -split ';')) {
            $directory = $entry.Trim().Trim('"')
            if ($directory -and [System.IO.Directory]::Exists($directory)) { $directory }
        }
    )
}


# Appraises one bounded probe result. A probe is proof only when the process started, exited 0 and
# printed a line matching -VersionPattern; otherwise Reason explains the rejection.
function Test-ProbeResult {
    param(
        $Result,
        [Parameter(Mandatory)][string]$VersionPattern,
        [int]$TimeoutMs = 20000
    )

    if ($Result.TimedOut) {
        return [pscustomobject]@{ Reason = "probe timed out after ${TimeoutMs}ms"; Version = $null }
    }
    if ($null -eq $Result.ExitCode) {
        return [pscustomobject]@{ Reason = "could not be started: $($Result.StdErr.Trim())"; Version = $null }
    }
    if ($Result.ExitCode -ne 0) {
        return [pscustomobject]@{ Reason = "probe exited $($Result.ExitCode)"; Version = $null }
    }
    $version = $Result.StdOut -split "`r?`n" | Where-Object { $_ -match $VersionPattern } | Select-Object -First 1
    if (-not $version) {
        return [pscustomobject]@{ Reason = "probe exited 0 but printed no line matching /$VersionPattern/"; Version = $null }
    }
    return [pscustomobject]@{ Reason = $null; Version = $version.Trim() }
}


# Resolves a Python interpreter and proves it before any gate uses it.
#
# Extensionless PATH hits are never candidates: Windows PowerShell resolves them but cannot execute
# them (no output, $? True, $LASTEXITCODE untouched), which is the vacuous-gate defect this guards
# against. Candidate order is python.exe, then the python.cmd/python.bat shell launchers, then
# py.exe, then py.cmd/py.bat; a later python.exe still wins over an earlier python.cmd, and a
# candidate that fails its probe is rejected instead of being used. Launchers stay allowed here
# because every dependency-rules gate is additionally held to its own success line, which a launcher
# cannot fake without actually running the checker. `.cmd`/`.bat` candidates run through cmd.exe
# (see Invoke-BoundedProcess).
function Get-ProvenPython {
    param(
        [Parameter(Mandatory)][string]$WorkingDirectory,
        [int]$ProbeTimeoutMs = 20000
    )

    $candidateNames = @("python.exe", "python.cmd", "python.bat", "py.exe", "py.cmd", "py.bat")
    $pathDirectories = @(Get-PathDirectories)
    $candidates = [System.Collections.Generic.List[string]]::new()
    $seen = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($candidateName in $candidateNames) {
        foreach ($directory in $pathDirectories) {
            $candidate = Join-Path $directory $candidateName
            if ([System.IO.File]::Exists($candidate) -and $seen.Add($candidate)) {
                $candidates.Add($candidate)
            }
        }
    }
    if ($candidates.Count -eq 0) {
        throw ("no usable Python interpreter: PATH has no $($candidateNames -join ', '), so the" +
            " dependency-rules gates cannot run (install CPython or add its directory to PATH); refusing to skip the gates")
    }

    $probeArguments = @("-c", "import sys; print('Python ' + sys.version.split()[0])")
    $rejections = [System.Collections.Generic.List[string]]::new()
    foreach ($candidate in $candidates) {
        $result = Invoke-BoundedProcess -FilePath $candidate -Arguments $probeArguments -TimeoutMs $ProbeTimeoutMs -WorkingDirectory $WorkingDirectory
        $appraisal = Test-ProbeResult -Result $result -VersionPattern '^Python \d+\.\d+' -TimeoutMs $ProbeTimeoutMs
        if ($appraisal.Reason) {
            $rejections.Add("$candidate ($($appraisal.Reason))")
            continue
        }
        return [pscustomobject]@{ Path = $candidate; Version = $appraisal.Version }
    }

    throw ("no usable Python interpreter: every candidate failed a bounded probe that must exit 0 and print a" +
        " version line; refusing to skip the dependency-rules gates -- " + ($rejections -join "; "))
}


# Resolves the cargo toolchain and proves that cargo itself AND its fmt subcommand really execute.
#
# Only a real cargo.exe is accepted. A .cmd/.bat wrapper is refused even when it answers --version
# with a correct version line: a batch file can forward --version and still no-op
# fmt/metadata/check/clippy/test/build, and a one-shot probe cannot prove that it forwards
# subcommands. The first cargo on PATH (shell order: cargo.exe, then cargo.cmd, then cargo.bat within
# each directory) is the command a bare `cargo` would run, so that hit is the one judged; an earlier
# wrapper fails the run closed instead of being skipped for a later binary.
#
# `cargo fmt --all -- --check` is silent on success, so the fmt subcommand family is proven here
# instead: `cargo fmt --version` must exit 0 and print a rustfmt version line. A silent fmt
# invocation therefore cannot pass unless the resolved cargo really implements the subcommand.
function Resolve-Cargo {
    param(
        [Parameter(Mandatory)][string]$WorkingDirectory,
        [int]$ProbeTimeoutMs = 20000
    )

    $candidateNames = @("cargo.exe", "cargo.cmd", "cargo.bat")
    $firstHit = $null
    :pathDirectories foreach ($directory in @(Get-PathDirectories)) {
        foreach ($candidateName in $candidateNames) {
            $candidate = Join-Path $directory $candidateName
            if ([System.IO.File]::Exists($candidate)) {
                $firstHit = $candidate
                break pathDirectories
            }
        }
    }

    $installHint = "install the Rust toolchain or add its bin directory to PATH"
    if (-not $firstHit) {
        throw ("no usable cargo executable: PATH has no cargo.exe and no cargo.cmd/cargo.bat shell launcher," +
            " so the fmt, metadata, check, clippy, test and build gates cannot run ($installHint); refusing to skip the gates")
    }
    if ([System.IO.Path]::GetExtension($firstHit) -ne ".exe") {
        throw ("no usable cargo executable: the first cargo on PATH is the shell launcher $firstHit, which is" +
            " refused because a batch wrapper can answer --version and still no-op" +
            " fmt/metadata/clippy/test/build, so only a real cargo.exe is accepted ($installHint); refusing to skip the gates")
    }

    $versionProbe = Invoke-BoundedProcess -FilePath $firstHit -Arguments @("--version") -TimeoutMs $ProbeTimeoutMs -WorkingDirectory $WorkingDirectory
    $versionAppraisal = Test-ProbeResult -Result $versionProbe -VersionPattern '^cargo \d+\.\d+\.\d+' -TimeoutMs $ProbeTimeoutMs
    if ($versionAppraisal.Reason) {
        throw "no usable cargo executable: $firstHit ($($versionAppraisal.Reason)); refusing to skip the gates"
    }

    $fmtProbe = Invoke-BoundedProcess -FilePath $firstHit -Arguments @("fmt", "--version") -TimeoutMs $ProbeTimeoutMs -WorkingDirectory $WorkingDirectory
    $fmtAppraisal = Test-ProbeResult -Result $fmtProbe -VersionPattern '^rustfmt \d+\.\d+' -TimeoutMs $ProbeTimeoutMs
    if ($fmtAppraisal.Reason) {
        throw ("no usable cargo executable: $firstHit does not prove the fmt subcommand" +
            " (cargo fmt --version: $($fmtAppraisal.Reason)); the fmt gate is silent on success and cannot be" +
            " trusted without this proof; refusing to skip the gates")
    }

    return [pscustomobject]@{ Path = $firstHit; Version = $versionAppraisal.Version; FmtVersion = $fmtAppraisal.Version }
}


# Runs one dependency-rules gate with the proven interpreter. The gate must report its own exit
# status and its documented success line; anything less fails closed instead of passing silently.
function Invoke-Python {
    param(
        [string[]]$Arguments = @(),
        [string]$ExpectedPattern
    )

    if (-not $script:PythonPath) {
        throw "python interpreter was not resolved; refusing to run $($Arguments -join ' ')"
    }
    $invocation = "$($script:PythonPath) $($Arguments -join ' ')"

    # Sentinel: a command that never actually runs leaves no exit status behind.
    $global:LASTEXITCODE = $null
    $output = & $script:PythonPath @Arguments 2>&1
    $exitCode = $LASTEXITCODE
    foreach ($line in $output) {
        Write-Host "$line"
    }

    if ($null -eq $exitCode) {
        throw "python step produced no exit status (the interpreter did not run): $invocation"
    }
    if ($exitCode -ne 0) {
        throw "python step failed: $invocation (exit $exitCode)"
    }
    if ($ExpectedPattern) {
        $text = ($output | ForEach-Object { "$_" }) -join "`n"
        if ($text -notmatch $ExpectedPattern) {
            throw "python step produced no expected output: $invocation (nothing matched /$ExpectedPattern/)"
        }
    }
}


# Runs one cargo gate through the proven absolute path and holds it to the signature its command
# really prints, so a command that exits 0 without doing the work cannot satisfy the gate. Output is
# captured and never discarded: a missing signature (or a metadata run without a parseable JSON
# document carrying `packages`) fails the run with an explicit message.
function Invoke-CargoGate {
    param(
        [string[]]$Arguments = @(),
        [string]$SignaturePattern,
        [switch]$AsMetadataJson
    )

    if (-not $script:CargoPath) {
        throw "cargo executable was not resolved; refusing to run $($Arguments -join ' ')"
    }
    $invocation = "$($script:CargoPath) $($Arguments -join ' ')"

    # Sentinel: a command that never actually runs leaves no exit status behind.
    $global:LASTEXITCODE = $null
    $output = & $script:CargoPath @Arguments 2>&1
    $exitCode = $LASTEXITCODE
    $text = ($output | ForEach-Object { "$_" }) -join "`n"

    # cargo metadata prints one large JSON document; echo it only when the run is not a success.
    $summariseMetadata = $AsMetadataJson -and ($null -ne $exitCode) -and ($exitCode -eq 0)
    if (-not $summariseMetadata) {
        foreach ($line in $output) {
            Write-Host "$line"
        }
    }

    if ($null -eq $exitCode) {
        throw "cargo step produced no exit status (the command did not run): $invocation"
    }
    if ($exitCode -ne 0) {
        throw "cargo step failed: $invocation (exit $exitCode)"
    }

    if ($AsMetadataJson) {
        if (-not $text.Trim()) {
            throw "cargo step produced no output where a JSON document was required: $invocation"
        }
        try {
            $document = $text | ConvertFrom-Json -ErrorAction Stop
        }
        catch {
            throw "cargo step produced output that is not valid JSON: $invocation ($($_.Exception.Message))"
        }
        if (-not $document.PSObject.Properties['packages']) {
            throw "cargo step output has no 'packages' field: $invocation"
        }
        Write-Host ("metadata: {0} package(s), {1} workspace member(s)" -f $document.packages.Count, $document.workspace_members.Count)
    }
    elseif ($SignaturePattern -and $text -notmatch $SignaturePattern) {
        throw ("cargo step produced no expected output signature: $invocation" +
            " (nothing matched /$SignaturePattern/); a silent or empty success is not proof that the gate ran")
    }
}

Push-Location $root
try {
    $python = Get-ProvenPython -WorkingDirectory $root
    $script:PythonPath = $python.Path
    Write-Host "python: $($python.Path) ($($python.Version))" -ForegroundColor DarkGray

    $cargo = Resolve-Cargo -WorkingDirectory $root
    $script:CargoPath = $cargo.Path
    Write-Host "cargo: $($cargo.Path) ($($cargo.Version); $($cargo.FmtVersion))" -ForegroundColor DarkGray

    $gates = [ordered]@{
        "fmt"                          = { Invoke-CargoGate -Arguments @("fmt", "--all", "--", "--check") }
        "metadata"                     = { Invoke-CargoGate -Arguments @("metadata", "--locked", "--no-deps", "--format-version", "1") -AsMetadataJson }
        "dependency rules (self-test)" = { Invoke-Python -Arguments @("-B", "scripts/check_dependency_rules.py", "--self-test") -ExpectedPattern 'Ran \d+ tests?' }
        "dependency rules (workspace)" = { Invoke-Python -Arguments @("-B", "scripts/check_dependency_rules.py") -ExpectedPattern 'allowlist edges verified for \d+ workspace member' }
    }
    if ($CheckOnly) {
        $gates["check (no link)"] = { Invoke-CargoGate -Arguments @("check", "--locked", "--workspace", "--all-targets") -SignaturePattern 'Checking|Compiling|Finished' }
    }
    else {
        $gates["clippy"] = { Invoke-CargoGate -Arguments @("clippy", "--locked", "--workspace", "--all-targets", "--", "-D", "warnings") -SignaturePattern 'Checking|Compiling|Finished' }
        $gates["tests"] = { Invoke-CargoGate -Arguments @("test", "--locked", "--workspace") -SignaturePattern 'test result:' }
        $gates["build"] = { Invoke-CargoGate -Arguments @("build", "--locked", "--workspace") -SignaturePattern 'Compiling|Finished' }
    }

    foreach ($name in $gates.Keys) {
        Write-Host "==> $name" -ForegroundColor Cyan
        # Reset the status per gate: an inherited $LASTEXITCODE must never satisfy a gate, and a
        # gate that leaves it untouched is reported instead of being trusted.
        $global:LASTEXITCODE = $null
        & $gates[$name]
        if ($null -eq $LASTEXITCODE) {
            throw "gate produced no exit status (the command did not run): $name"
        }
        if ($LASTEXITCODE -ne 0) {
            throw "gate failed: $name"
        }
    }
    if ($CheckOnly) {
        Write-Host "local checks passed; clippy, tests, build and native smoke NOT_RUN" -ForegroundColor Yellow
    }
    else {
        Write-Host "all quality gates passed (native smoke is separate)" -ForegroundColor Green
    }
}
catch {
    Write-Host "quality gate failure: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
finally {
    Pop-Location
}
