# MP-SEC2 limited native verification: abnormal-exit secret exposure.
#
# Scope (deliberately bounded — this is NOT MP-SEC2 completion):
#   * launches the current cos-desktop.exe in runtime mode with a SYNTHETIC
#     observation token against a refused loopback endpoint,
#   * terminates one run abnormally (hard kill) and one run normally (a single,
#     required window-close control),
#   * scans the app-managed files it can name (isolated app-data tree, temp tree,
#     child stdout/stderr) for the synthetic token,
#   * records Windows Error Reporting user-dump configuration READ-ONLY and reports
#     it as NOT_RUN (no crash is induced here and no OS setting is changed).
#
# PASS is strict: every required root and required file must be present and fully
# scanned, with zero matches, zero unreadable/oversize/excluded entries and no walk
# cap. An evidence gap (for example the synthetic credential never reached the OS
# store) downgrades the result to HOLD, never PASS. Every per-area status is derived
# from the same evidence as the overall verdict and the exit code.
#
# Safety:
#   * the credential store is read as present / absent / unreadable; an existing
#     entry for the synthetic origin or an unreadable store aborts the run before any
#     launch (nothing is overwritten or deleted),
#   * only entries this run created are removed (verified present after launch, or
#     created by the negative-test switch),
#   * only child PIDs this script started are stopped; no process is killed by name,
#   * the work directory is GUID-unique and created exclusively; the negative-test
#     switch is a closed enum whose resolved path is re-validated inside that tree,
#   * evidence is written to partial files and promoted by rename only after every
#     output succeeded; a partial failure deletes the partials and any promoted file,
#     so a PASS record can never survive a write failure.
#
# Negative-test switches (regression proof only; a normal run never sets them):
#   -SimulateMissingArtifact <name>   delete one known required artifact before the
#                                     scan, proving a missing file cannot PASS
#   -SimulateCredentialLoss           drop the synthetic entry just before the
#                                     post-start check, proving a missing store write
#                                     downgrades to HOLD
#   -SimulatePreexistingCredential    create an entry for the synthetic origin before
#                                     launch, proving the pre-check aborts (an entry
#                                     that already exists is never overwritten)
#   -SimulateEvidenceFailure          make the evidence write fail after the first
#                                     partial, proving no PASS evidence is left behind
#
# It never uses a real credential, never touches the Runtime DB, the frozen
# fixtures or any OS setting, and never turns on WER. Evidence is written to a
# private directory outside the repository.
#
# Exit codes: 0 = PASS, 1 = FAIL, 2 = HOLD (evidence gap, no leak proven),
#             3 = precondition failure (exe/instance/credential store/environment).
[CmdletBinding()]
param(
    [string]$Exe = "",
    [string]$EvidenceDir = "C:\dev\cos-desktop-verification\rel1-2026-10-30",
    [int]$Port = 0,
    [int]$ReadinessTimeoutMs = 30000,
    [int]$SettleMs = 2500,
    [int]$CloseTimeoutMs = 15000,
    [int]$MaxFiles = 5000,
    [int]$MaxFileBytes = 4194304,
    [int]$MaxDepth = 16,
    [ValidateSet('desktop.instance', 'preferences.json', 'normal-1.stderr')]
    [string]$SimulateMissingArtifact = "",
    [switch]$SimulateCredentialLoss,
    [switch]$SimulatePreexistingCredential,
    [switch]$SimulateEvidenceFailure,
    [switch]$KeepWork
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

$stamp = (Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssfffZ')
$runId = [guid]::NewGuid().ToString('N').Substring(0, 8)
$repo = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($Exe)) { $Exe = Join-Path $repo 'target\debug\cos-desktop.exe' }

$script:childProcesses = [System.Collections.Generic.List[System.Diagnostics.Process]]::new()
$script:credentialTarget = $null
$script:credentialCleanup = 'not-attempted'
$script:credentialCreatedByTest = $false
$script:credentialPrecheckAbsent = $false
$script:simulatedPreexisting = $false
$script:lastCredentialError = 0

function Write-Section([string]$text) { Write-Host "== $text" }

function Get-Sha256File([string]$path) {
    $stream = [System.IO.File]::OpenRead($path)
    try {
        $sha = [System.Security.Cryptography.SHA256]::Create()
        try { return ([System.BitConverter]::ToString($sha.ComputeHash($stream)) -replace '-', '').ToLowerInvariant() }
        finally { $sha.Dispose() }
    }
    finally { $stream.Dispose() }
}

function Get-Sha256Text([string]$text) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($text)
        return ([System.BitConverter]::ToString($sha.ComputeHash($bytes)) -replace '-', '').ToLowerInvariant()
    }
    finally { $sha.Dispose() }
}

function Get-FreeLoopbackPort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $listener.Start()
    try { return ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port }
    finally { $listener.Stop() }
}

$nativeSource = @'
using System;
using System.Runtime.InteropServices;

public static class CosCredNative
{
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern bool CredReadW(string target, uint type, uint flags, out IntPtr credential);

    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern bool CredDeleteW(string target, uint type, uint flags);

    [DllImport("advapi32.dll")]
    public static extern void CredFree(IntPtr credential);
}
'@
Add-Type -TypeDefinition $nativeSource

# Three-state read: present / absent / error. Only ERROR_NOT_FOUND (1168) is a real
# absence; every other failure is an unreadable store, never "no entry".
function Get-CredentialState([string]$target) {
    $ptr = [IntPtr]::Zero
    if ([CosCredNative]::CredReadW($target, 1, 0, [ref]$ptr)) {
        [CosCredNative]::CredFree($ptr)
        $script:lastCredentialError = 0
        return 'present'
    }
    $code = [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()
    $script:lastCredentialError = $code
    if ($code -eq 1168) { return 'absent' }
    return 'error'
}

function Remove-CredentialSilently([string]$target) {
    $state = Get-CredentialState $target
    if ($state -eq 'absent') { return 'absent' }
    if ($state -eq 'error') { return 'read-error' }
    $deleted = [CosCredNative]::CredDeleteW($target, 1, 0)
    if ($deleted -and (Get-CredentialState $target) -eq 'absent') { return 'deleted' }
    return 'failed'
}

function Find-Bytes([byte[]]$haystack, [byte[]]$needle) {
    if ($needle.Length -eq 0 -or $haystack.Length -lt $needle.Length) { return -1 }
    $last = $haystack.Length - $needle.Length
    for ($i = 0; $i -le $last; $i++) {
        if ($haystack[$i] -ne $needle[0]) { continue }
        $hit = $true
        for ($j = 1; $j -lt $needle.Length; $j++) {
            if ($haystack[$i + $j] -ne $needle[$j]) { $hit = $false; break }
        }
        if ($hit) { return $i }
    }
    return -1
}

function Scan-Tokens([string[]]$roots, [byte[]]$needle, [string]$labelBase) {
    $state = @{
        files = 0; visited = 0; oversize = 0; unreadable = 0; excluded = 0; capped = $false
        matches = @(); unreadable_paths = @(); oversize_paths = @(); excluded_paths = @()
        scanned_relative = @()
    }
    $reported = @()
    $missingRoots = @()
    foreach ($root in $roots) {
        $reported += $root
        if (-not (Test-Path -LiteralPath $root)) { $missingRoots += $root; continue }
        $stack = [System.Collections.Generic.Stack[object]]::new()
        $stack.Push(@($root, 0))
        while ($stack.Count -gt 0) {
            $item = $stack.Pop()
            $directory = $item[0]
            $depth = $item[1]
            if ($depth -gt $MaxDepth) { $state.capped = $true; continue }
            try { $entries = @(Get-ChildItem -LiteralPath $directory -Force -ErrorAction Stop) }
            catch {
                $state.unreadable++
                if ($state.unreadable_paths.Count -lt 20) { $state.unreadable_paths += $directory }
                continue
            }
            foreach ($entry in $entries) {
                if ($state.files -ge $MaxFiles) { $state.capped = $true; break }
                $state.visited++
                if ($entry.Attributes -band [System.IO.FileAttributes]::ReparsePoint) {
                    $state.excluded++
                    if ($state.excluded_paths.Count -lt 20) { $state.excluded_paths += $entry.Name }
                    continue
                }
                if ($entry.PSIsContainer) { $stack.Push(@($entry.FullName, ($depth + 1))); continue }
                if ($entry.Length -gt $MaxFileBytes) {
                    $state.oversize++
                    if ($state.oversize_paths.Count -lt 20) { $state.oversize_paths += $entry.Name }
                    continue
                }
                try { $bytes = [System.IO.File]::ReadAllBytes($entry.FullName) }
                catch {
                    $state.unreadable++
                    if ($state.unreadable_paths.Count -lt 20) { $state.unreadable_paths += $entry.Name }
                    continue
                }
                $state.files++
                $relative = $entry.FullName
                if ($relative.StartsWith($labelBase, [System.StringComparison]::OrdinalIgnoreCase)) {
                    $relative = $relative.Substring($labelBase.Length).TrimStart('\', '/')
                }
                $relative = $relative -replace '/', '\'
                $state.scanned_relative += $relative
                $index = Find-Bytes $bytes $needle
                if ($index -ge 0) {
                    $state.matches += [ordered]@{ relative_to_work = $relative; offset = $index }
                }
            }
        }
    }
    return [ordered]@{
        roots = $reported
        roots_missing = $missingRoots
        files_scanned = $state.files
        entries_visited = $state.visited
        oversize_skipped = $state.oversize
        oversize_paths = $state.oversize_paths
        unreadable_entries = $state.unreadable
        unreadable_paths = $state.unreadable_paths
        excluded_entries = $state.excluded
        excluded_paths = $state.excluded_paths
        walk_capped = $state.capped
        scanned_relative = $state.scanned_relative
        matches = $state.matches
    }
}

$report = [ordered]@{
    schema = 'cos-desktop/mp-sec2-native-verification/3'
    generated_at_utc = $stamp
    run_id = $runId
    limited_verification = $true
    mp_sec2_complete = $false
    negative_test = $null
    result = 'FAIL'
    exe = $null
    os = $null
    repo = $null
    runs = [ordered]@{}
    areas = [ordered]@{}
    checks = [ordered]@{}
    not_run = @()
    notes = @()
}
$exitCode = 1
$work = $null
$preconditionFailure = $false
$abn = $null
$nrm = $null

try {
    Write-Section 'preconditions'
    if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) {
        $preconditionFailure = $true
        throw "cos-desktop.exe not found: $Exe (build it first: cargo build --locked -p cos-desktop)"
    }
    $existing = @(Get-Process -Name 'cos-desktop' -ErrorAction SilentlyContinue)
    if ($existing.Count -gt 0) {
        $preconditionFailure = $true
        throw "a cos-desktop process is already running (pid $($existing[0].Id)); this script never kills a process it did not start"
    }
    New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null
    $work = Join-Path $EvidenceDir ("work-" + $stamp + "-" + $runId)
    $null = New-Item -ItemType Directory -Path $work -ErrorAction Stop
    $appData = Join-Path $work 'appdata'
    $tempDir = Join-Path $work 'temp'
    $null = New-Item -ItemType Directory -Path $appData, $tempDir -ErrorAction Stop

    $exeInfo = Get-Item -LiteralPath $Exe
    $report.exe = [ordered]@{
        path = $exeInfo.FullName
        sha256 = (Get-Sha256File $exeInfo.FullName)
        size_bytes = [long]$exeInfo.Length
        last_write_utc = $exeInfo.LastWriteTimeUtc.ToString('o')
    }
    $osInfo = Get-CimInstance -ClassName Win32_OperatingSystem
    $report.os = [ordered]@{
        caption = $osInfo.Caption
        version = $osInfo.Version
        build = $osInfo.BuildNumber
        architecture = $osInfo.OSArchitecture
        pwsh = $PSVersionTable.PSVersion.ToString()
    }
    $gitHead = 'unknown'
    $gitDirty = -1
    try {
        $gitHead = (& git -C $repo rev-parse HEAD 2>$null)
        if ($null -eq $gitHead) { $gitHead = 'unknown' }
        $gitDirty = @(& git -C $repo status --porcelain 2>$null).Count
    } catch { $gitHead = 'unknown' }
    $report.repo = [ordered]@{
        git_head = ([string]$gitHead).Trim()
        dirty_entries = $gitDirty
        note = 'the verification reads the working-tree executable; Runtime DB and frozen fixtures are never referenced'
    }

    $port = if ($Port -gt 0) { $Port } else { Get-FreeLoopbackPort }
    $endpoint = "http://127.0.0.1:$port"
    $token = 'synthetic-sec2-' + [guid]::NewGuid().ToString('N')
    $tokenBytes = [System.Text.Encoding]::UTF8.GetBytes($token)
    $tokenSha = Get-Sha256Text $token
    $originHead = (Get-Sha256Text $endpoint).Substring(0, 32)
    $script:credentialTarget = "desktop-observation.cos-desktop/observation/$originHead"
    $report['synthetic_secret'] = [ordered]@{
        token_sha256 = $tokenSha
        note = 'the token itself is never written to any evidence file; only this SHA-256 is recorded'
    }
    $report.negative_test = if ($SimulateMissingArtifact -ne '') { "missing-artifact:$SimulateMissingArtifact" } elseif ($SimulateCredentialLoss) { 'credential-loss' } elseif ($SimulatePreexistingCredential) { 'preexisting-credential' } elseif ($SimulateEvidenceFailure) { 'evidence-failure' } else { $null }

    Write-Section 'credential precondition'
    # The store is read three ways: present / absent / unreadable. Never write or delete
    # before this check, and never launch the Desktop on a target whose entry exists.
    if ($SimulatePreexistingCredential) {
        $state = Get-CredentialState $script:credentialTarget
        if ($state -ne 'absent') {
            $preconditionFailure = $true
            throw ("refusing to simulate a pre-existing entry: the target is already " + $state + " (win32 " + $script:lastCredentialError + ")")
        }
        $null = & cmdkey.exe /generic:"$script:credentialTarget" /user:desktop-observation /pass:synthetic-simulated-entry 2>$null
        if ((Get-CredentialState $script:credentialTarget) -ne 'present') {
            throw 'could not create the simulated pre-existing credential entry'
        }
        $script:simulatedPreexisting = $true
        $report.notes += 'negative test: simulated a pre-existing credential for the synthetic origin'
    }
    $state = Get-CredentialState $script:credentialTarget
    if ($state -eq 'present') {
        $preconditionFailure = $true
        throw 'a credential already exists for the synthetic origin; refusing to overwrite or delete it'
    }
    if ($state -eq 'error') {
        $preconditionFailure = $true
        throw ("the credential store could not be read (win32 " + $script:lastCredentialError + "); the run cannot prove absence")
    }
    $script:credentialPrecheckAbsent = $true
    Write-Host 'credential precondition: the synthetic origin is absent and the store is readable'

    $report.runs['abnormal'] = [ordered]@{
        mode = 'runtime'
        endpoint = $endpoint
        token_sha256 = $tokenSha
        credential_target_sha256 = (Get-Sha256Text $script:credentialTarget)
        pid = $null
        command_line = $null
        environment = $null
        started_at_utc = $null
        ready_observed = $false
        ready_instance_fresh = $false
        credential_state_after_launch = $null
        killed = $false
        exit_code = $null
        alive_ms_before_kill = $null
        stdout_bytes = 0
        stderr_bytes = 0
        stdout_sha256 = $null
        stderr_sha256 = $null
        preferences_absent_after_kill = $null
        credential_cleanup_after_run = $null
    }
    $report.runs['normal'] = [ordered]@{
        mode = 'runtime'
        endpoint = $endpoint
        token_sha256 = $tokenSha
        pid = $null
        command_line = $null
        environment = $null
        started_at_utc = $null
        ready_observed = $false
        ready_instance_fresh = $false
        credential_state_after_launch = $null
        close_requested = $false
        confirmed = $false
        exit_code = $null
        exit_observed_ms = $null
        stdout_bytes = 0
        stderr_bytes = 0
        stdout_sha256 = $null
        stderr_sha256 = $null
        stderr_save_confirmed = $false
        preferences_written_after_start = $false
        preferences_sha256 = $null
        preferences_mtime_utc = $null
    }

    $instancePath = Join-Path $appData 'cos-desktop\desktop.instance'
    $preferencesPath = Join-Path $appData 'cos-desktop\preferences.json'

    function Start-ObservedApp([string]$stdoutPath, [string]$stderrPath) {
        $psi = [System.Diagnostics.ProcessStartInfo]::new()
        $psi.FileName = $Exe
        $psi.UseShellExecute = $false
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $psi.EnvironmentVariables['LOCALAPPDATA'] = $appData
        $psi.EnvironmentVariables['TEMP'] = $tempDir
        $psi.EnvironmentVariables['TMP'] = $tempDir
        $psi.EnvironmentVariables['COS_DESKTOP_RUNTIME_ENDPOINT'] = $endpoint
        $psi.EnvironmentVariables['COS_DESKTOP_RUNTIME_TOKEN'] = $token
        $process = [System.Diagnostics.Process]::new()
        $process.StartInfo = $psi
        $null = $process.Start()
        $script:childProcesses.Add($process)
        return [ordered]@{
            process = $process
            started_utc = [DateTime]::UtcNow
            stdout_path = $stdoutPath
            stderr_path = $stderrPath
            stdout_task = $process.StandardOutput.ReadToEndAsync()
            stderr_task = $process.StandardError.ReadToEndAsync()
        }
    }

    function Wait-Ready([System.Diagnostics.Process]$process, [DateTime]$launchedUtc, [int]$timeoutMs) {
        $deadline = [DateTime]::UtcNow.AddMilliseconds($timeoutMs)
        while ([DateTime]::UtcNow -lt $deadline) {
            if ($process.HasExited) { return $null }
            if (Test-Path -LiteralPath $instancePath) {
                $mtime = (Get-Item -LiteralPath $instancePath).LastWriteTimeUtc
                if ($mtime -ge $launchedUtc.AddSeconds(-1)) {
                    return [ordered]@{ fresh = $true; mtime_utc = $mtime.ToString('o') }
                }
            }
            Start-Sleep -Milliseconds 100
        }
        return $null
    }

    function Stop-Hard([System.Diagnostics.Process]$process) {
        if (-not $process.HasExited) {
            $process.Kill()
            $null = $process.WaitForExit(10000)
        }
    }

    function Complete-Streams([object]$run) {
        $stdout = $run.stdout_task.GetAwaiter().GetResult()
        $stderr = $run.stderr_task.GetAwaiter().GetResult()
        [System.IO.File]::WriteAllText($run.stdout_path, $stdout, [System.Text.Encoding]::UTF8)
        [System.IO.File]::WriteAllText($run.stderr_path, $stderr, [System.Text.Encoding]::UTF8)
        return [ordered]@{
            stdout = $stdout
            stderr = $stderr
            stdout_bytes = [long]([System.Text.Encoding]::UTF8.GetByteCount($stdout))
            stderr_bytes = [long]([System.Text.Encoding]::UTF8.GetByteCount($stderr))
            stdout_sha256 = (Get-Sha256Text $stdout)
            stderr_sha256 = (Get-Sha256Text $stderr)
        }
    }

    Write-Section 'abnormal run (runtime mode, hard kill)'
    Remove-Item -LiteralPath $instancePath -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $preferencesPath -Force -ErrorAction SilentlyContinue
    if ((Test-Path -LiteralPath $instancePath) -or (Test-Path -LiteralPath $preferencesPath)) {
        throw 'a retained instance/preferences file survived the pre-launch cleanup'
    }
    $abn = Start-ObservedApp (Join-Path $work 'abnormal.stdout') (Join-Path $work 'abnormal.stderr')
    $report.runs.abnormal.pid = $abn.process.Id
    $report.runs.abnormal.started_at_utc = $abn.started_utc.ToString('o')
    $report.runs.abnormal.command_line = ('"' + $Exe + '" (no arguments; environment-only configuration)')
    $report.runs.abnormal.environment = [ordered]@{
        LOCALAPPDATA = '<isolated app-data root below the private evidence directory>'
        TEMP = '<isolated temp root below the private evidence directory>'
        TMP = '<isolated temp root below the private evidence directory>'
        COS_DESKTOP_RUNTIME_ENDPOINT = $endpoint
        COS_DESKTOP_RUNTIME_TOKEN = ('sha256:' + $tokenSha)
    }
    $ready = Wait-Ready $abn.process $abn.started_utc $ReadinessTimeoutMs
    if ($null -eq $ready) {
        Stop-Hard $abn.process
        throw "the abnormal-run process never became ready (no fresh desktop.instance within $ReadinessTimeoutMs ms)"
    }
    $report.runs.abnormal.ready_observed = $true
    $report.runs.abnormal.ready_instance_fresh = $ready.fresh
    if ($SimulateCredentialLoss) { $null = Remove-CredentialSilently $script:credentialTarget }
    $state = Get-CredentialState $script:credentialTarget
    $report.runs.abnormal.credential_state_after_launch = $state
    if ($state -eq 'present') { $script:credentialCreatedByTest = $true }
    if ($state -ne 'present') {
        Write-Host ("WARNING: the synthetic credential is " + $state + " after launch (store path not exercised)")
    }
    Start-Sleep -Milliseconds $SettleMs
    if ($abn.process.HasExited) {
        $null = Complete-Streams $abn
        throw "the abnormal-run process exited on its own (exit $($abn.process.ExitCode)); an abnormal kill was not observed"
    }
    $report.runs.abnormal.alive_ms_before_kill = [int]([DateTime]::UtcNow - $abn.started_utc).TotalMilliseconds
    Stop-Hard $abn.process
    $report.runs.abnormal.killed = $abn.process.HasExited
    $report.runs.abnormal.exit_code = $abn.process.ExitCode
    $streams = Complete-Streams $abn
    $report.runs.abnormal.stdout_bytes = $streams.stdout_bytes
    $report.runs.abnormal.stderr_bytes = $streams.stderr_bytes
    $report.runs.abnormal.stdout_sha256 = $streams.stdout_sha256
    $report.runs.abnormal.stderr_sha256 = $streams.stderr_sha256
    if (-not $abn.process.HasExited) { throw 'the abnormal-run process did not exit after a hard kill' }
    $report.runs.abnormal.preferences_absent_after_kill = -not (Test-Path -LiteralPath $preferencesPath)
    $report.runs.abnormal.credential_cleanup_after_run = Remove-CredentialSilently $script:credentialTarget
    if ($report.runs.abnormal.credential_cleanup_after_run -eq 'deleted') { $script:credentialCreatedByTest = $false }
    Write-Host ("abnormal: killed=true exit_code=" + $report.runs.abnormal.exit_code + " alive_ms=" + $report.runs.abnormal.alive_ms_before_kill + " stderr=" + $streams.stderr_bytes + " preferences_absent=" + $report.runs.abnormal.preferences_absent_after_kill + " credential=" + $state + "/" + $report.runs.abnormal.credential_cleanup_after_run)

    Write-Section 'normal-exit control (runtime mode, single required window close)'
    Remove-Item -LiteralPath $instancePath -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $preferencesPath -Force -ErrorAction SilentlyContinue
    if ((Test-Path -LiteralPath $instancePath) -or (Test-Path -LiteralPath $preferencesPath)) {
        throw 'a retained instance/preferences file survived the pre-launch cleanup'
    }
    $nrm = Start-ObservedApp (Join-Path $work 'normal-1.stdout') (Join-Path $work 'normal-1.stderr')
    $report.runs.normal.pid = $nrm.process.Id
    $report.runs.normal.started_at_utc = $nrm.started_utc.ToString('o')
    $report.runs.normal.command_line = $report.runs.abnormal.command_line
    $report.runs.normal.environment = $report.runs.abnormal.environment
    $ready = Wait-Ready $nrm.process $nrm.started_utc $ReadinessTimeoutMs
    if ($null -eq $ready) {
        Stop-Hard $nrm.process
        $null = Complete-Streams $nrm
        throw "the normal-exit control never became ready (no fresh desktop.instance within $ReadinessTimeoutMs ms)"
    }
    $report.runs.normal.ready_observed = $true
    $report.runs.normal.ready_instance_fresh = $ready.fresh
    $state = Get-CredentialState $script:credentialTarget
    $report.runs.normal.credential_state_after_launch = $state
    if ($state -eq 'present') { $script:credentialCreatedByTest = $true }
    Start-Sleep -Milliseconds 1000
    $report.runs.normal.close_requested = $nrm.process.CloseMainWindow()
    if (-not $report.runs.normal.close_requested) {
        Stop-Hard $nrm.process
        throw 'the window-close request was not delivered; the normal-exit control is invalid (no fallback kill)'
    }
    $exited = $nrm.process.WaitForExit($CloseTimeoutMs)
    if (-not $exited) {
        Stop-Hard $nrm.process
        throw "the normal-exit control did not exit within $CloseTimeoutMs ms"
    }
    $report.runs.normal.confirmed = $true
    $report.runs.normal.exit_code = $nrm.process.ExitCode
    $report.runs.normal.exit_observed_ms = [int]([DateTime]::UtcNow - $nrm.started_utc).TotalMilliseconds
    $streams = Complete-Streams $nrm
    $report.runs.normal.stdout_bytes = $streams.stdout_bytes
    $report.runs.normal.stderr_bytes = $streams.stderr_bytes
    $report.runs.normal.stdout_sha256 = $streams.stdout_sha256
    $report.runs.normal.stderr_sha256 = $streams.stderr_sha256
    # The app's own shutdown notice is evidence only; the file below is the ground truth.
    $report.runs.normal.stderr_save_confirmed = $streams.stderr.Contains('preferences save confirmed')
    if (Test-Path -LiteralPath $preferencesPath) {
        $preferences = Get-Item -LiteralPath $preferencesPath
        $report.runs.normal.preferences_written_after_start = $preferences.LastWriteTimeUtc -ge $nrm.started_utc.AddSeconds(-1)
        $report.runs.normal.preferences_mtime_utc = $preferences.LastWriteTimeUtc.ToString('o')
        $report.runs.normal.preferences_sha256 = (Get-Sha256File $preferences.FullName)
    }
    Write-Host ("normal: close_requested=" + $report.runs.normal.close_requested + " exit_code=" + $report.runs.normal.exit_code + " fresh_preferences=" + $report.runs.normal.preferences_written_after_start + " save_notice=" + $report.runs.normal.stderr_save_confirmed + " stderr=" + $streams.stderr_bytes)

    Write-Section 'scan app-managed files'
    if ($SimulateMissingArtifact -ne '') {
        # Closed enum plus containment re-validation: an escaping path is a hard failure.
        $simulated = switch ($SimulateMissingArtifact) {
            'desktop.instance' { $instancePath }
            'preferences.json' { $preferencesPath }
            'normal-1.stderr' { Join-Path $work 'normal-1.stderr' }
        }
        $workingPrefix = $work + '\'
        $resolved = [System.IO.Path]::GetFullPath([string]$simulated)
        if (-not $resolved.StartsWith($workingPrefix, [System.StringComparison]::OrdinalIgnoreCase) -or $resolved.Contains('..')) {
            throw ("negative test path escapes the isolated work directory: " + $resolved)
        }
        if (Test-Path -LiteralPath $resolved) {
            Remove-Item -LiteralPath $resolved -Force -ErrorAction Stop
            $report.notes += ("negative test: removed required artifact " + $SimulateMissingArtifact)
        } else {
            $report.notes += ("negative test: artifact already missing " + $SimulateMissingArtifact)
        }
    }
    $streamFiles = @('abnormal.stdout', 'abnormal.stderr', 'normal-1.stdout', 'normal-1.stderr') | ForEach-Object { Join-Path $work $_ }
    $scanRoots = @($appData, $tempDir) + $streamFiles
    $scan = Scan-Tokens $scanRoots $tokenBytes $work
    $scan['status'] = 'PENDING'
    $scan['pattern'] = ('utf8 synthetic token (sha256:' + $tokenSha + ')')
    $scan['bounds'] = [ordered]@{ max_files = $MaxFiles; max_file_bytes = $MaxFileBytes; max_depth = $MaxDepth }
    $requiredArtifacts = @(
        'appdata\cos-desktop\desktop.instance',
        'appdata\cos-desktop\preferences.json',
        'abnormal.stdout', 'abnormal.stderr', 'normal-1.stdout', 'normal-1.stderr'
    )
    $scan['required_artifacts'] = $requiredArtifacts
    $scan['required_artifacts_missing'] = @($requiredArtifacts | Where-Object { -not ($scan.scanned_relative -contains $_) })
    $report.areas['app_managed_files'] = $scan
    Write-Host ("app-managed files: scanned=" + $scan.files_scanned + " oversize=" + $scan.oversize_skipped + " unreadable=" + $scan.unreadable_entries + " excluded=" + $scan.excluded_entries + " capped=" + $scan.walk_capped + " matches=" + $scan.matches.Count + " missing_required=" + $scan.required_artifacts_missing.Count)
    $report.notes += 'app-managed roots: isolated LOCALAPPDATA tree, isolated TEMP tree, child stdout/stderr capture'

    Write-Section 'support report'
    $report.areas['support_report'] = [ordered]@{
        status = 'NOT_APPLICABLE'
        reason = 'the support report is rendered in memory on demand; the app has no file writer for it, so there is no persisted report to scan'
    }
    Write-Host 'support report: NOT_APPLICABLE (no persistence path; nothing written to disk)'

    Write-Section 'Windows Error Reporting (read-only)'
    function Get-WerLocalDumps([string]$path) {
        if (-not (Test-Path -LiteralPath $path)) { return $null }
        try {
            $key = Get-Item -LiteralPath $path -ErrorAction Stop
            $names = @($key.GetValueNames())
            $type = $null
            $count = $null
            if ($names -contains 'DumpType') { $type = $key.GetValue('DumpType') }
            if ($names -contains 'DumpCount') { $count = $key.GetValue('DumpCount') }
            return [ordered]@{
                exists = $true
                readable = $true
                value_names = $names
                dump_folder_configured = ($names -contains 'DumpFolder')
                dump_type = $type
                dump_count = $count
            }
        } catch {
            return [ordered]@{ exists = $true; readable = $false; read_error = $_.Exception.Message }
        }
    }
    $machineDumps = Get-WerLocalDumps 'HKLM:\SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps'
    $userDumps = Get-WerLocalDumps 'HKCU:\Software\Microsoft\Windows\Windows Error Reporting\LocalDumps'
    $werService = 'unknown'
    try { $werService = (Get-Service -Name WerSvc -ErrorAction Stop).Status.ToString() } catch { $werService = 'unknown' }
    $report.areas['wer_user_dumps'] = [ordered]@{
        status = 'NOT_RUN'
        machine_local_dumps = $machineDumps
        user_local_dumps = $userDumps
        wer_service = $werService
        reason = 'read-only check; no OS setting changed; a hard kill (TerminateProcess) does not invoke WER and no dump was collected'
    }
    $report.not_run += 'WER/user-mode dumps and any OS-managed crash report: configuration only, no dump induced or collected'
    Write-Host ("WER: service=" + $werService + " machine_dumps=" + ($null -ne $machineDumps) + " user_dumps=" + ($null -ne $userDumps) + " status=NOT_RUN")

    $report.not_run += 'OS credential store contents beyond existence/deletion: no plaintext read, no ACL claim'
    $report.not_run += 'other user profiles, process memory, shell history and pagefile'
    $report.not_run += 'DD-22 retention/export and a bounded crash-report writer (not implemented in this slice)'
}
catch {
    $report.notes += ("error: " + $_.Exception.Message)
    Write-Host ("ERROR: " + $_.Exception.Message) -ForegroundColor Red
}
finally {
    foreach ($process in $script:childProcesses) {
        if ($null -ne $process -and -not $process.HasExited) {
            $report.notes += ("finally: stopping our child pid " + $process.Id)
            Stop-Hard $process
        }
    }
    # Remove only an entry this run created, then re-verify absence in three states.
    $credentialAbsent = $true
    $credentialStateAfterCleanup = 'absent'
    $credentialCreated = $script:credentialCreatedByTest
    if ($null -ne $script:credentialTarget) {
        if ($credentialCreated) {
            $script:credentialCleanup = Remove-CredentialSilently $script:credentialTarget
            $credentialStateAfterCleanup = Get-CredentialState $script:credentialTarget
            $credentialAbsent = ($credentialStateAfterCleanup -eq 'absent')
        } elseif ($script:credentialPrecheckAbsent) {
            $stateNow = Get-CredentialState $script:credentialTarget
            if ($stateNow -eq 'present') {
                $credentialCreated = $true
                $script:credentialCleanup = Remove-CredentialSilently $script:credentialTarget
                $credentialStateAfterCleanup = Get-CredentialState $script:credentialTarget
                $credentialAbsent = ($credentialStateAfterCleanup -eq 'absent')
            } elseif ($stateNow -eq 'error') {
                $credentialStateAfterCleanup = 'error'
                $credentialAbsent = $false
                $script:credentialCleanup = 'read-error'
            } else {
                $script:credentialCleanup = 'absent'
            }
        } else {
            $script:credentialCleanup = 'skipped-no-precheck'
        }
    }
    if ($script:simulatedPreexisting) {
        $simulatedCleanup = Remove-CredentialSilently $script:credentialTarget
        $report.notes += ("negative test cleanup: simulated pre-existing credential " + $simulatedCleanup)
        $credentialStateAfterCleanup = Get-CredentialState $script:credentialTarget
        $credentialAbsent = ($credentialStateAfterCleanup -eq 'absent')
        if ($credentialAbsent) { $script:credentialCleanup = 'deleted-simulated' }
    }
    $report.areas['credential_store'] = [ordered]@{
        status = 'PENDING'
        entry_written_by_design = $true
        cleanup = $script:credentialCleanup
        state_after_cleanup = $credentialStateAfterCleanup
        absent_after_cleanup = $credentialAbsent
        note = 'the synthetic token is persisted by the MP-SEC1 path in the OS credential store, which is DPAPI-backed and not a plaintext file; only entries this run created are removed'
    }
    Write-Host ("credential store: final cleanup=" + $script:credentialCleanup + " state=" + $credentialStateAfterCleanup)

    $scan = $null
    if ($report.areas.Contains('app_managed_files')) { $scan = $report.areas['app_managed_files'] }
    $abnRun = $null
    if ($report.runs.Contains('abnormal')) { $abnRun = $report.runs['abnormal'] }
    $nrmRun = $null
    if ($report.runs.Contains('normal')) { $nrmRun = $report.runs['normal'] }
    $fail = @()
    $hold = @()
    if ($preconditionFailure) { $fail += 'precondition' }
    if ($null -ne $scan) {
        if ($scan.matches.Count -gt 0) { $fail += 'token match in an app-managed file' }
        if ($scan.roots_missing.Count -gt 0) { $fail += 'required scan root missing' }
        if ($scan.required_artifacts_missing.Count -gt 0) { $fail += 'required artifact not covered by the scan' }
        if ($scan.unreadable_entries -gt 0) { $hold += 'unreadable files were skipped' }
        if ($scan.oversize_skipped -gt 0) { $hold += 'oversize files were skipped' }
        if ($scan.excluded_entries -gt 0) { $hold += 'excluded (reparse) entries were skipped' }
        if ($scan.walk_capped) { $hold += 'the walk hit its cap' }
    } else {
        $fail += 'no scan was completed'
    }
    $abnormalKilled = ($null -ne $abnRun) -and $abnRun.killed
    $abnormalReady = ($null -ne $abnRun) -and $abnRun.ready_observed
    $normalClose = ($null -ne $nrmRun) -and $nrmRun.close_requested
    $normalConfirmed = ($null -ne $nrmRun) -and $nrmRun.confirmed
    $normalExitZero = ($null -ne $nrmRun) -and ($nrmRun.exit_code -eq 0)
    $normalPreferences = ($null -ne $nrmRun) -and $nrmRun.preferences_written_after_start
    $abnormalCredential = ($null -ne $abnRun) -and ($abnRun.credential_state_after_launch -eq 'present')
    $normalCredential = ($null -ne $nrmRun) -and ($nrmRun.credential_state_after_launch -eq 'present')
    if (-not $abnormalKilled) { $fail += 'the abnormal run was not killed' }
    if (-not $abnormalReady) { $fail += 'the abnormal run never became ready' }
    if (-not $normalClose) { $fail += 'the window-close request was not delivered' }
    if (-not $normalConfirmed) { $fail += 'the normal-exit control was not confirmed' }
    if (-not $normalExitZero) { $fail += 'the normal-exit control did not exit 0' }
    if (-not $normalPreferences) { $fail += 'preferences.json was not written by the normal-exit control' }
    if (-not $abnormalCredential) { $hold += 'the synthetic credential was not observed in the OS store (abnormal run)' }
    if (-not $normalCredential) { $hold += 'the synthetic credential was not observed in the OS store (normal run)' }
    if (-not $credentialAbsent) { $fail += 'the synthetic credential could not be removed' }
    $scanFail = ($null -eq $scan) -or ($scan.matches.Count -gt 0) -or ($scan.roots_missing.Count -gt 0) -or ($scan.required_artifacts_missing.Count -gt 0)
    $scanHold = ($null -ne $scan) -and (($scan.unreadable_entries + $scan.oversize_skipped + $scan.excluded_entries) -gt 0 -or $scan.walk_capped)
    $scanStatus = if ($scanFail) { 'FAIL' } elseif ($scanHold) { 'HOLD' } else { 'PASS' }
    $credentialStatus = if (-not $credentialAbsent) { 'FAIL' } elseif (-not ($abnormalCredential -and $normalCredential)) { 'HOLD' } else { 'PASS' }
    $abnormalStatus = if ($abnormalKilled -and $abnormalReady) { 'PASS' } else { 'FAIL' }
    $normalStatus = if ($normalClose -and $normalConfirmed -and $normalExitZero -and $normalPreferences) { 'PASS' } else { 'FAIL' }
    if ($null -ne $scan) { $report.areas['app_managed_files']['status'] = $scanStatus }
    if ($report.areas.Contains('credential_store')) { $report.areas['credential_store']['status'] = $credentialStatus }
    if ($report.runs.Contains('abnormal')) { $report.runs['abnormal']['status'] = $abnormalStatus }
    if ($report.runs.Contains('normal')) { $report.runs['normal']['status'] = $normalStatus }
    $report.checks = [ordered]@{
        fail = $fail
        hold = $hold
        abnormal_run = $abnormalStatus
        normal_control = $normalStatus
        scan_completeness = $scanStatus
        credential = $credentialStatus
        support_report = 'NOT_APPLICABLE'
        wer_user_dumps = 'NOT_RUN'
    }
    if ($fail.Count -gt 0) {
        $report.result = 'FAIL'
        $exitCode = 1
    } elseif ($hold.Count -gt 0) {
        $report.result = 'HOLD'
        $exitCode = 2
    } else {
        $report.result = 'PASS'
        $exitCode = 0
    }
    if ($preconditionFailure) { $exitCode = 3 }

    $report.finished_at_utc = (Get-Date).ToUniversalTime().ToString('o')
    $jsonPath = Join-Path $EvidenceDir ("mp-sec2-native-" + $stamp + "-" + $runId + ".json")
    $textPath = Join-Path $EvidenceDir ("mp-sec2-native-" + $stamp + "-" + $runId + ".txt")
    $jsonPartial = $jsonPath + '.partial'
    $textPartial = $textPath + '.partial'
    try {
        $report | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $jsonPartial -Encoding UTF8
        if ($SimulateEvidenceFailure) { throw 'negative test: simulated failure after the first evidence partial' }
        $lines = @()
        $lines += ("result: " + $report.result)
        $lines += ("exe: " + ($report.exe | ConvertTo-Json -Compress -Depth 4))
        $lines += ("os: " + ($report.os | ConvertTo-Json -Compress -Depth 4))
        foreach ($runName in $report.runs.Keys) { $lines += ("run.{0}: {1}" -f $runName, ($report.runs[$runName] | ConvertTo-Json -Compress -Depth 4)) }
        foreach ($areaName in $report.areas.Keys) { $lines += ("area.{0}: {1}" -f $areaName, ($report.areas[$areaName] | ConvertTo-Json -Compress -Depth 4)) }
        $lines += ("checks: " + ($report.checks | ConvertTo-Json -Compress -Depth 4))
        foreach ($entry in $report.not_run) { $lines += ("NOT_RUN: " + $entry) }
        $lines | Set-Content -LiteralPath $textPartial -Encoding UTF8
        # Promote both outputs only after every write succeeded; a failure rolls back so a
        # PASS record can never survive a partial write.
        Move-Item -LiteralPath $jsonPartial -Destination $jsonPath -ErrorAction Stop
        Move-Item -LiteralPath $textPartial -Destination $textPath -ErrorAction Stop
        Write-Host ("evidence: " + $jsonPath)
        Write-Host ("evidence: " + $textPath)
    } catch {
        Write-Host ("could not write evidence: " + $_.Exception.Message) -ForegroundColor Red
        foreach ($leftoverFile in @($jsonPartial, $textPartial, $jsonPath, $textPath)) {
            Remove-Item -LiteralPath $leftoverFile -Force -ErrorAction SilentlyContinue
        }
        $report.result = 'FAIL'
        $report.checks.fail += 'evidence files could not be written'
        $exitCode = 1
    }
    if (-not $KeepWork -and $null -ne $work -and (Test-Path -LiteralPath $work)) {
        $removed = $false
        for ($attempt = 0; $attempt -lt 3 -and -not $removed; $attempt++) {
            try { Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction Stop; $removed = $true }
            catch { Start-Sleep -Milliseconds 400 }
        }
        if ($removed) { Write-Host 'isolated work tree removed' } else { Write-Host 'WARNING: isolated work tree could not be removed' }
    }
    Write-Host ("result: " + $report.result + " (exit " + $exitCode + ")")
    exit $exitCode
}
