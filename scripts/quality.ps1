# Runs the TESTING.md quality gates 1-5 plus the dependency-rules check, in documented order.
# Windows-first, repository root resolved from this script's location. The native startup
# smoke (gate 6) stays a recorded manual gate on this workstation and is not simulated here.
#
# Interpreter and toolchain resolution is proven, not assumed. Python (python.exe, then the
# python.cmd/python.bat launchers, then py.exe/py.cmd/py.bat) and cargo (cargo.exe, then
# cargo.cmd/cargo.bat, resolved in shell order) must each answer a bounded version probe with
# exit 0 and a recognized version line before any gate runs, and every gate then invokes the
# proven absolute path. A bare `python`/`cargo` lookup is not proof on Windows: PATH may put an
# extensionless POSIX launcher or a store alias first, which PowerShell cannot execute, so a gate
# built on it reports nothing and passes vacuously. An unproven tool fails the run closed.
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


# Resolves one tool executable from PATH and proves it before any gate uses it.
#
# Extensionless PATH hits are never candidates: Windows PowerShell resolves them but cannot execute
# them (no output, $? True, $LASTEXITCODE untouched), which is the vacuous-gate defect this guards
# against. `-Resolution name-major` scans every candidate name across PATH in the given order, so a
# later python.exe still wins over an earlier python.cmd (the documented Python order) and an
# unprovable candidate is rejected rather than used. `-Resolution path-order` takes the first
# candidate the shell itself would find and fails closed when that one cannot be proven, so a
# shadowing launcher earlier on PATH (.bat/.cmd) can never be silently skipped for a later binary.
# `.cmd`/`.bat` candidates run through cmd.exe (see Invoke-BoundedProcess). Every candidate must
# answer a bounded probe with exit 0 and a version line matching -VersionPattern.
function Get-ProvenTool {
    param(
        [Parameter(Mandatory)][string]$DisplayName,
        [Parameter(Mandatory)][string[]]$ExecutableNames,
        [Parameter(Mandatory)][string[]]$ProbeArguments,
        [Parameter(Mandatory)][string]$VersionPattern,
        [Parameter(Mandatory)][string]$Consequence,
        [Parameter(Mandatory)][string]$InstallHint,
        [Parameter(Mandatory)][string]$WorkingDirectory,
        [ValidateSet("name-major", "path-order")][string]$Resolution = "name-major",
        [int]$ProbeTimeoutMs = 20000
    )

    # Plain filesystem probes: Test-Path throws for ACL-restricted PATH entries such as
    # ...\config\systemprofile\AppData\Local\Microsoft\WindowsApps.
    $pathDirectories = @(
        foreach ($entry in ($env:PATH -split ';')) {
            $directory = $entry.Trim().Trim('"')
            if ($directory -and [System.IO.Directory]::Exists($directory)) { $directory }
        }
    )

    $candidates = [System.Collections.Generic.List[string]]::new()
    $seen = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    if ($Resolution -eq "path-order") {
        $firstHit = $null
        :pathDirectories foreach ($directory in $pathDirectories) {
            foreach ($executableName in $ExecutableNames) {
                $candidate = Join-Path $directory $executableName
                if ([System.IO.File]::Exists($candidate)) {
                    $firstHit = $candidate
                    break pathDirectories
                }
            }
        }
        if ($firstHit) { $candidates.Add($firstHit) }
    }
    else {
        foreach ($executableName in $ExecutableNames) {
            foreach ($directory in $pathDirectories) {
                $candidate = Join-Path $directory $executableName
                if ([System.IO.File]::Exists($candidate) -and $seen.Add($candidate)) {
                    $candidates.Add($candidate)
                }
            }
        }
    }
    if ($candidates.Count -eq 0) {
        throw ("no usable $($DisplayName): PATH has no $($ExecutableNames -join ', '), so $Consequence cannot run" +
            " ($InstallHint); refusing to skip the gates")
    }

    $rejections = [System.Collections.Generic.List[string]]::new()
    foreach ($candidate in $candidates) {
        $result = Invoke-BoundedProcess -FilePath $candidate -Arguments $ProbeArguments -TimeoutMs $ProbeTimeoutMs -WorkingDirectory $WorkingDirectory
        if ($result.TimedOut) {
            $rejections.Add("$candidate (probe timed out after ${ProbeTimeoutMs}ms)")
            continue
        }
        if ($null -eq $result.ExitCode) {
            $rejections.Add("$candidate (could not be started: $($result.StdErr.Trim()))")
            continue
        }
        if ($result.ExitCode -ne 0) {
            $rejections.Add("$candidate (probe exited $($result.ExitCode))")
            continue
        }
        $version = $result.StdOut -split "`r?`n" | Where-Object { $_ -match $VersionPattern } | Select-Object -First 1
        if (-not $version) {
            $rejections.Add("$candidate (probe exited 0 but printed no line matching /$VersionPattern/)")
            continue
        }
        return [pscustomobject]@{ Path = $candidate; Version = $version.Trim() }
    }

    throw ("no usable $($DisplayName): every candidate failed a bounded probe that must exit 0 and print a" +
        " version line; refusing to skip $Consequence -- " + ($rejections -join "; "))
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

Push-Location $root
try {
    $python = Get-ProvenTool -DisplayName "Python interpreter" `
        -ExecutableNames @("python.exe", "python.cmd", "python.bat", "py.exe", "py.cmd", "py.bat") `
        -ProbeArguments @("-c", "import sys; print('Python ' + sys.version.split()[0])") `
        -VersionPattern '^Python \d+\.\d+' `
        -Consequence "the dependency-rules gates" `
        -InstallHint "install CPython or add its directory to PATH" `
        -WorkingDirectory $root `
        -Resolution name-major
    $script:PythonPath = $python.Path
    Write-Host "python: $($python.Path) ($($python.Version))" -ForegroundColor DarkGray

    $cargo = Get-ProvenTool -DisplayName "cargo executable" `
        -ExecutableNames @("cargo.exe", "cargo.cmd", "cargo.bat") `
        -ProbeArguments @("--version") `
        -VersionPattern '^cargo \d+\.\d+\.\d+' `
        -Consequence "the fmt, metadata, check, clippy, test and build gates" `
        -InstallHint "install the Rust toolchain or add its bin directory to PATH" `
        -WorkingDirectory $root `
        -Resolution path-order
    $script:CargoPath = $cargo.Path
    Write-Host "cargo: $($cargo.Path) ($($cargo.Version))" -ForegroundColor DarkGray

    $gates = [ordered]@{
        "fmt"                          = { & $script:CargoPath fmt --all -- --check }
        "metadata"                     = { & $script:CargoPath metadata --locked --no-deps --format-version 1 | Out-Null }
        "dependency rules (self-test)" = { Invoke-Python -Arguments @("-B", "scripts/check_dependency_rules.py", "--self-test") -ExpectedPattern 'Ran \d+ tests?' }
        "dependency rules (workspace)" = { Invoke-Python -Arguments @("-B", "scripts/check_dependency_rules.py") -ExpectedPattern 'allowlist edges verified for \d+ workspace member' }
    }
    if ($CheckOnly) {
        $gates["check (no link)"] = { & $script:CargoPath check --locked --workspace --all-targets }
    }
    else {
        $gates["clippy"] = { & $script:CargoPath clippy --locked --workspace --all-targets -- -D warnings }
        $gates["tests"] = { & $script:CargoPath test --locked --workspace }
        $gates["build"] = { & $script:CargoPath build --locked --workspace }
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
