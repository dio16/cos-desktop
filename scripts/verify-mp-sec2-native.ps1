# MP-SEC2 limited native verification: abnormal-exit secret exposure.
#
# Scope (deliberately bounded — this is NOT MP-SEC2 completion):
#   * launches the current cos-desktop.exe in runtime mode with a SYNTHETIC
#     observation token against a refused loopback endpoint,
#   * terminates one run abnormally (hard kill) and one run normally (window close
#     control),
#   * scans the app-managed files it can name (isolated app-data tree, temp tree,
#     child stdout/stderr) for the synthetic token,
#   * records Windows Error Reporting user-dump configuration READ-ONLY and reports
#     it as NOT_RUN (no crash is induced here and no OS setting is changed).
#
# It never uses a real credential, never touches the Runtime DB, the frozen
# fixtures or any OS setting, and never turns on WER. Evidence is written to a
# private directory outside the repository.
#
# Exit codes: 0 = bounded verification completed with zero matches; 1 = a required
# step failed or a match was found; 2 = precondition failure (exe/instance/OS).
[CmdletBinding()]
param(
    [string]$Exe = "",
    [string]$EvidenceDir = "C:\dev\cos-desktop-verification\rel1-2026-10-30",
    [int]$ReadinessTimeoutMs = 30000,
    [int]$SettleMs = 2500,
    [int]$CloseTimeoutMs = 15000,
    [int]$MaxFiles = 5000,
    [int]$MaxFileBytes = 4194304,
    [int]$MaxDepth = 16,
    [switch]$KeepWork
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

$stamp = (Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssZ')
$repo = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($Exe)) { $Exe = Join-Path $repo 'target\debug\cos-desktop.exe' }

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

function Test-CredentialExists([string]$target) {
    $ptr = [IntPtr]::Zero
    $found = [CosCredNative]::CredReadW($target, 1, 0, [ref]$ptr)
    if ($found) { [CosCredNative]::CredFree($ptr) }
    return $found
}

function Remove-CredentialSilently([string]$target) {
    if (-not (Test-CredentialExists $target)) { return 'absent' }
    $deleted = [CosCredNative]::CredDeleteW($target, 1, 0)
    if ($deleted -and -not (Test-CredentialExists $target)) { return 'deleted' }
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
    $state = @{ files = 0; visited = 0; oversize = 0; unreadable = 0; capped = $false; matches = @(); unreadable_paths = @(); oversize_paths = @() }
    $reported = @()
    foreach ($root in $roots) {
        $reported += $root
        if (-not (Test-Path -LiteralPath $root)) { continue }
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
                if ($entry.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { continue }
                if ($entry.PSIsContainer) { $stack.Push(@($entry.FullName, ($depth + 1))); continue }
                if ($entry.Length -gt $MaxFileBytes) {
                    $state.oversize++
                    if ($state.oversize_paths.Count -lt 20) { $state.oversize_paths += $entry.Name }
                    continue
                }
                try { $bytes = [System.IO.File]::ReadAllBytes($entry.FullName) }
                catch {
                $state.unreadable++
                if ($state.unreadable_paths.Count -lt 20) { $state.unreadable_paths += $directory }
                continue
            }
                $state.files++
                $index = Find-Bytes $bytes $needle
                if ($index -ge 0) {
                    $relative = $entry.FullName
                    if ($relative.StartsWith($labelBase, [System.StringComparison]::OrdinalIgnoreCase)) {
                        $relative = $relative.Substring($labelBase.Length).TrimStart('\', '/')
                    }
                    $state.matches += [ordered]@{ relative_to_work = $relative; offset = $index }
                }
            }
        }
    }
    return [ordered]@{
        roots = $reported
        files_scanned = $state.files
        entries_visited = $state.visited
        oversize_skipped = $state.oversize
        oversize_paths = $state.oversize_paths
        unreadable_entries = $state.unreadable
        unreadable_paths = $state.unreadable_paths
        walk_capped = $state.capped
        matches = $state.matches
    }
}

$report = [ordered]@{
    schema = 'cos-desktop/mp-sec2-native-verification/1'
    generated_at_utc = $stamp
    limited_verification = $true
    mp_sec2_complete = $false
    result = 'FAIL'
    exe = $null
    os = $null
    runs = [ordered]@{}
    areas = [ordered]@{}
    not_run = @()
    notes = @()
}
$exitCode = 1
$work = $null

try {
    Write-Section 'preconditions'
    if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) {
        throw "cos-desktop.exe not found: $Exe (build it first: cargo build --locked -p cos-desktop)"
    }
    $existing = @(Get-Process -Name 'cos-desktop' -ErrorAction SilentlyContinue)
    if ($existing.Count -gt 0) {
        throw "a cos-desktop process is already running (pid $($existing[0].Id)); stop it first"
    }
    New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null
    $work = Join-Path $EvidenceDir ("work-" + $stamp)
    $appData = Join-Path $work 'appdata'
    $tempDir = Join-Path $work 'temp'
    New-Item -ItemType Directory -Force -Path $appData, $tempDir | Out-Null

    $exeInfo = Get-Item -LiteralPath $Exe
    $report.exe = [ordered]@{
        path = $exeInfo.FullName
        sha256 = (Get-Sha256File $exeInfo.FullName)
        size_bytes = [long]$exeInfo.Length
        last_write_utc = $exeInfo.LastWriteTimeUtc.ToString('o')
    }
    $osInfo = Get-CimInstance -ClassName Win32_OperatingSystem
    $gitHead = 'unknown'
    $gitDirty = -1
    try {
        $gitHead = (& git -C $repo rev-parse HEAD 2>$null)
        if ($null -eq $gitHead) { $gitHead = 'unknown' }
        $gitDirty = @(& git -C $repo status --porcelain 2>$null).Count
    } catch { $gitHead = 'unknown' }
    $report['repo'] = [ordered]@{
        git_head = ([string]$gitHead).Trim()
        dirty_entries = $gitDirty
        note = 'the verification reads the working-tree executable; Runtime DB and frozen fixtures are never referenced'
    }
    $report.os = [ordered]@{
        caption = $osInfo.Caption
        version = $osInfo.Version
        build = $osInfo.BuildNumber
        architecture = $osInfo.OSArchitecture
        pwsh = $PSVersionTable.PSVersion.ToString()
    }
    Write-Host ("exe sha256=" + $report.exe.sha256 + " size=" + $report.exe.size_bytes)
    Write-Host ("os=" + $osInfo.Caption + " build=" + $osInfo.BuildNumber + " pwsh=" + $PSVersionTable.PSVersion)

    $port = Get-FreeLoopbackPort
    $endpoint = "http://127.0.0.1:$port"
    $token = 'synthetic-sec2-' + [guid]::NewGuid().ToString('N')
    $tokenBytes = [System.Text.Encoding]::UTF8.GetBytes($token)
    $tokenSha = Get-Sha256Text $token
    $origin = $endpoint
    $originHead = (Get-Sha256Text $origin).Substring(0, 32)
    $credentialTarget = "desktop-observation.cos-desktop/observation/$originHead"
    $report['synthetic_secret'] = [ordered]@{
        token_sha256 = $tokenSha
        note = 'the token itself is never written to any evidence file; only this SHA-256 is recorded'
    }
    $report.runs['abnormal'] = [ordered]@{
        mode = 'runtime'
        endpoint = $endpoint
        token_sha256 = $tokenSha
        credential_target_sha256 = (Get-Sha256Text $credentialTarget)
        pid = $null
        command_line = $null
        environment = $null
        ready_observed = $false
        killed = $false
        exit_code = $null
        alive_ms_before_kill = $null
        stdout_bytes = 0
        stderr_bytes = 0
        stdout_sha256 = $null
        stderr_sha256 = $null
    }
    $report.runs['normal'] = [ordered]@{
        mode = 'runtime'
        endpoint = $endpoint
        token_sha256 = $tokenSha
        pid = $null
        command_line = $null
        environment = $null
        ready_observed = $false
        close_requested = $false
        confirmed = $false
        exit_code = $null
        exit_observed_ms = $null
        stdout_bytes = 0
        stderr_bytes = 0
        stdout_sha256 = $null
        stderr_sha256 = $null
    }

    $instancePath = Join-Path $appData 'cos-desktop\desktop.instance'

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
        return [ordered]@{
            process = $process
            stdout_path = $stdoutPath
            stderr_path = $stderrPath
            stdout_task = $process.StandardOutput.ReadToEndAsync()
            stderr_task = $process.StandardError.ReadToEndAsync()
        }
    }

    function Wait-Ready([System.Diagnostics.Process]$process, [int]$timeoutMs) {
        $deadline = [DateTime]::UtcNow.AddMilliseconds($timeoutMs)
        while ([DateTime]::UtcNow -lt $deadline) {
            if ($process.HasExited) { return $false }
            if (Test-Path -LiteralPath $instancePath) { return $true }
            Start-Sleep -Milliseconds 100
        }
        return $false
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
            stdout_bytes = [long]([System.Text.Encoding]::UTF8.GetByteCount($stdout))
            stderr_bytes = [long]([System.Text.Encoding]::UTF8.GetByteCount($stderr))
            stdout_sha256 = (Get-Sha256Text $stdout)
            stderr_sha256 = (Get-Sha256Text $stderr)
        }
    }

    Write-Section 'abnormal run (runtime mode, hard kill)'
    $abn = Start-ObservedApp (Join-Path $work 'abnormal.stdout') (Join-Path $work 'abnormal.stderr')
    $report.runs.abnormal.pid = $abn.process.Id
    $report.runs.abnormal.command_line = ('"' + $Exe + '" (no arguments; environment-only configuration)')
    $report.runs.abnormal.environment = [ordered]@{
        LOCALAPPDATA = '<isolated app-data root below the private evidence directory>'
        TEMP = '<isolated temp root below the private evidence directory>'
        TMP = '<isolated temp root below the private evidence directory>'
        COS_DESKTOP_RUNTIME_ENDPOINT = $endpoint
        COS_DESKTOP_RUNTIME_TOKEN = ('sha256:' + $tokenSha)
    }
    $started = [DateTime]::UtcNow
    $ready = Wait-Ready $abn.process $ReadinessTimeoutMs
    $report.runs.abnormal.ready_observed = $ready
    if (-not $ready) {
        Stop-Hard $abn.process
        $null = Complete-Streams $abn
        throw "the abnormal-run process never became ready (no desktop.instance within $ReadinessTimeoutMs ms)"
    }
    Start-Sleep -Milliseconds $SettleMs
    if ($abn.process.HasExited) {
        $null = Complete-Streams $abn
        throw "the abnormal-run process exited on its own (exit $($abn.process.ExitCode)); an abnormal kill was not observed"
    }
    $report.runs.abnormal.alive_ms_before_kill = [int]([DateTime]::UtcNow - $started).TotalMilliseconds
    Stop-Hard $abn.process
    $report.runs.abnormal.killed = $abn.process.HasExited
    $report.runs.abnormal.exit_code = $abn.process.ExitCode
    $streams = Complete-Streams $abn
    $report.runs.abnormal.stdout_bytes = $streams.stdout_bytes
    $report.runs.abnormal.stderr_bytes = $streams.stderr_bytes
    $report.runs.abnormal.stdout_sha256 = $streams.stdout_sha256
    $report.runs.abnormal.stderr_sha256 = $streams.stderr_sha256
    if (-not $abn.process.HasExited) { throw 'the abnormal-run process did not exit after a hard kill' }
    Write-Host ("abnormal: killed=true exit_code=" + $report.runs.abnormal.exit_code + " alive_ms=" + $report.runs.abnormal.alive_ms_before_kill + " stdout=" + $streams.stdout_bytes + " stderr=" + $streams.stderr_bytes)

    Start-Sleep -Milliseconds 500

    Write-Section 'normal-exit control (runtime mode, window close)'
    $nrm = Start-ObservedApp (Join-Path $work 'normal.stdout') (Join-Path $work 'normal.stderr')
    $report.runs.normal.pid = $nrm.process.Id
    $report.runs.normal.command_line = ('"' + $Exe + '" (no arguments; environment-only configuration)')
    $report.runs.normal.environment = $report.runs.abnormal.environment
    $started = [DateTime]::UtcNow
    $ready = Wait-Ready $nrm.process $ReadinessTimeoutMs
    $report.runs.normal.ready_observed = $ready
    if (-not $ready) {
        Stop-Hard $nrm.process
        $null = Complete-Streams $nrm
        $report.not_run += 'normal-exit control: the process never became ready'
    } else {
        Start-Sleep -Milliseconds 1000
        $report.runs.normal.close_requested = $nrm.process.CloseMainWindow()
        if (-not $report.runs.normal.close_requested) { $null = & taskkill.exe /PID $nrm.process.Id 2>$null }
        $exited = $nrm.process.WaitForExit($CloseTimeoutMs)
        if (-not $exited) {
            Stop-Hard $nrm.process
            $report.not_run += "normal-exit control: the process did not exit within $CloseTimeoutMs ms"
        }
        $report.runs.normal.confirmed = $nrm.process.HasExited
        $report.runs.normal.exit_code = $nrm.process.ExitCode
        $streams = Complete-Streams $nrm
        $report.runs.normal.stdout_bytes = $streams.stdout_bytes
        $report.runs.normal.stderr_bytes = $streams.stderr_bytes
        $report.runs.normal.stdout_sha256 = $streams.stdout_sha256
        $report.runs.normal.stderr_sha256 = $streams.stderr_sha256
        $report.runs.normal.exit_observed_ms = [int]([DateTime]::UtcNow - $started).TotalMilliseconds
        Write-Host ("normal: confirmed=" + $report.runs.normal.confirmed + " exit_code=" + $report.runs.normal.exit_code + " within_ms=" + $report.runs.normal.exit_observed_ms)
    }

    Write-Section 'scan app-managed files'
    $scanRoots = @($appData, $tempDir, (Join-Path $work 'abnormal.stdout'), (Join-Path $work 'abnormal.stderr'), (Join-Path $work 'normal.stdout'), (Join-Path $work 'normal.stderr'))
    $scan = Scan-Tokens $scanRoots $tokenBytes $work
    $scan['status'] = if (($scan.matches.Count -eq 0) -and (-not $scan.walk_capped)) { 'PASS' } else { 'FAIL' }
    $scan['pattern'] = ('utf8 synthetic token (sha256:' + $tokenSha + ')')
    $scan['bounds'] = [ordered]@{ max_files = $MaxFiles; max_file_bytes = $MaxFileBytes; max_depth = $MaxDepth }
    $report.areas['app_managed_files'] = $scan
    Write-Host ("app-managed files: scanned=" + $scan.files_scanned + " oversize=" + $scan.oversize_skipped + " unreadable=" + $scan.unreadable_entries + " capped=" + $scan.walk_capped + " matches=" + $scan.matches.Count + " status=" + $scan.status)
    $instanceRelative = 'cos-desktop\desktop.instance'
    $preferencesRelative = 'cos-desktop\preferences.json'
    $artifacts = @()
    if (Test-Path -LiteralPath (Join-Path $appData $instanceRelative)) { $artifacts += $instanceRelative }
    if (Test-Path -LiteralPath (Join-Path $appData $preferencesRelative)) { $artifacts += $preferencesRelative }
    $scan['app_artifacts_present'] = $artifacts
    $scan['preferences_present'] = $artifacts -contains $preferencesRelative
    $report.notes += 'app-managed roots: isolated LOCALAPPDATA tree, isolated TEMP tree, child stdout/stderr capture'
    if (-not $scan.preferences_present) { $report.not_run += 'preferences.json was not observed after the normal-exit control' }

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

    Write-Section 'credential store (synthetic entry, OS-managed)'
    $credentialState = Remove-CredentialSilently $credentialTarget
    $report.areas['credential_store'] = [ordered]@{
        status = if ($credentialState -eq 'failed') { 'HOLD' } else { 'PASS' }
        entry_written_by_design = $true
        cleanup = $credentialState
        note = 'the synthetic token is persisted by the MP-SEC1 path in the OS credential store, which is DPAPI-backed and not a plaintext file; the entry is removed here'
    }
    $report.not_run += 'OS credential store contents beyond existence/deletion: no plaintext read, no ACL claim'
    Write-Host ("credential store: cleanup=" + $credentialState)

    Start-Sleep -Milliseconds 300
    $leftover = @(Get-Process -Name 'cos-desktop' -ErrorAction SilentlyContinue)
    if ($leftover.Count -gt 0) {
        $report.not_run += ("leftover cos-desktop process(es) killed: " + (($leftover | ForEach-Object { $_.Id }) -join ','))
        foreach ($process in $leftover) { Stop-Hard $process }
    }

    $report.not_run += 'other user profiles, process memory, shell history and pagefile'
    $report.not_run += 'DD-22 retention/export and a bounded crash-report writer (not implemented in this slice)'

    $failed = ($scan.status -ne 'PASS') -or (-not $report.runs.abnormal.killed) -or ($credentialState -eq 'failed')
    $report.result = if ($failed) { 'FAIL' } else { 'PASS' }
    $exitCode = if ($failed) { 1 } else { 0 }
}
catch {
    $report.result = 'FAIL'
    $report.notes += ("error: " + $_.Exception.Message)
    Write-Host ("ERROR: " + $_.Exception.Message) -ForegroundColor Red
    $exitCode = 1
}
finally {
    $report.finished_at_utc = (Get-Date).ToUniversalTime().ToString('o')
    $jsonPath = Join-Path $EvidenceDir ("mp-sec2-native-" + $stamp + ".json")
    $textPath = Join-Path $EvidenceDir ("mp-sec2-native-" + $stamp + ".txt")
    try {
        $report | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $jsonPath -Encoding UTF8
        $lines = @()
        $lines += ("result: " + $report.result)
        $lines += ("exe: " + ($report.exe | ConvertTo-Json -Compress -Depth 4))
        $lines += ("os: " + ($report.os | ConvertTo-Json -Compress -Depth 4))
        foreach ($runName in $report.runs.Keys) {
            $lines += ("run.{0}: {1}" -f $runName, ($report.runs[$runName] | ConvertTo-Json -Compress -Depth 4))
        }
        foreach ($areaName in $report.areas.Keys) {
            $area = $report.areas[$areaName]
            $lines += ("area.{0}: {1}" -f $areaName, ($area | ConvertTo-Json -Compress -Depth 4))
        }
        foreach ($entry in $report.not_run) { $lines += ("NOT_RUN: " + $entry) }
        $lines | Set-Content -LiteralPath $textPath -Encoding UTF8
        Write-Host ("evidence: " + $jsonPath)
        Write-Host ("evidence: " + $textPath)
    } catch {
        Write-Host ("could not write evidence: " + $_.Exception.Message) -ForegroundColor Red
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
    Write-Host ("result: " + $report.result)
    exit $exitCode
}
