param(
    [string]$Exe = "C:\dev\cos-desktop\target\debug\cos-desktop.exe",
    [string]$Endpoint = "http://127.0.0.1:9",
    [int]$Runs = 5,
    [int]$SettleSeconds = 30,
    [int]$WindowSeconds = 60,
    [int]$SampleMs = 500,
    [string]$BuildKind = "debug (cargo build --locked)",
    [string]$EvidenceDir = "C:\dev\cos-desktop-verification\rel1-2026-10-30",
    [string]$WorkRoot = "",
    [int]$SimulateEarlyExitSeconds = 0,
    [switch]$SimulateExeMismatch,
    [switch]$Connected,
    [switch]$SimulateFixture,
    [switch]$RejectProbe
)

# MP-R0B idle baseline, disconnected variant. The approved conditions are mandatory for a
# limited PASS: exactly five fresh-process runs, a 30 s settle and a 60 s observation window
# at 500 ms sampling. Any other value is refused instead of quietly producing a weaker
# measurement. The Simulate* switches exist only to exercise the negative paths.
$ErrorActionPreference = 'Stop'
if ($Runs -ne 5) { [Console]::Error.WriteLine("-Runs must be exactly 5 for the approved baseline (got $Runs)"); exit 2 }
if ($SettleSeconds -ne 30) { [Console]::Error.WriteLine("-SettleSeconds must be exactly 30 for the approved baseline (got $SettleSeconds)"); exit 2 }
if ($WindowSeconds -ne 60) { [Console]::Error.WriteLine("-WindowSeconds must be exactly 60 for the approved baseline (got $WindowSeconds)"); exit 2 }
if ($SampleMs -ne 500) { [Console]::Error.WriteLine("-SampleMs must be exactly 500 for the approved baseline (got $SampleMs)"); exit 2 }
if ($SimulateEarlyExitSeconds -lt 0) { [Console]::Error.WriteLine("-SimulateEarlyExitSeconds must be >= 0"); exit 2 }

$stamp = (Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssZ')
New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null
if ([string]::IsNullOrWhiteSpace($WorkRoot)) { $WorkRoot = Join-Path $env:TEMP "cos-r0b-idle-$stamp" }
New-Item -ItemType Directory -Force -Path $WorkRoot | Out-Null

function Format-Hex([byte[]]$Bytes) {
    if ($null -eq $Bytes) { return $null }
    return ([System.BitConverter]::ToString($Bytes) -replace '-', '').ToLowerInvariant()
}

function Get-FileHashText([string]$Path) {
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $sha = [System.Security.Cryptography.SHA256]::Create()
        try { return Format-Hex $sha.ComputeHash($stream) } finally { $sha.Dispose() }
    } finally { $stream.Dispose() }
}

function Test-EndpointAvailable([string]$Url) {
    try {
        $uri = [Uri]$Url
        $client = [System.Net.Sockets.TcpClient]::new()
        $task = $client.ConnectAsync($uri.Host, $uri.Port)
        $connected = $task.Wait(1000) -and $client.Connected
        $client.Dispose()
        return $connected
    } catch { return $false }
}

function Get-Median([double[]]$Values) {
    if ($null -eq $Values -or $Values.Count -eq 0) { return $null }
    $sorted = @($Values | Sort-Object)
    $count = $sorted.Count
    if ($count % 2 -eq 1) {
        return [double]$sorted[[int][math]::Floor($count / 2)]
    }
    # Even counts: the median is the mean of the two middle values.
    return ([double]$sorted[$count / 2 - 1] + [double]$sorted[$count / 2]) / 2.0
}

function Get-Statistics([double[]]$Values) {
    if ($null -eq $Values -or $Values.Count -eq 0) { return $null }
    $mean = ($Values | Measure-Object -Average).Average
    $sum = 0.0
    foreach ($value in $Values) { $sum += ($value - $mean) * ($value - $mean) }
    $stddev = if ($Values.Count -ge 2) { [math]::Sqrt($sum / ($Values.Count - 1)) } else { 0.0 }
    return [ordered]@{
        count = $Values.Count
        median = Get-Median $Values
        mean = $mean
        stddev = $stddev
        noise_ratio = if ($mean -gt 0) { $stddev / $mean } else { $null }
    }
}

$repo_root = Split-Path -Parent $PSScriptRoot
$git_head = (& git -C $repo_root rev-parse HEAD 2>$null)
$git_status = @(& git -C $repo_root status --porcelain 2>$null)
$git_dirty = $git_status.Count -gt 0

# B-02: pin the executable and source identity BEFORE the first launch, not afterwards.
$exe_pinned_sha = Get-FileHashText $Exe
$endpoint_available = Test-EndpointAvailable $Endpoint
if ($Connected -and -not $endpoint_available) {
    [Console]::Error.WriteLine("-Connected requires a reachable runtime endpoint (got $Endpoint); refusing to record a disconnected run as connected")
    exit 2
}

if (-not ('R0BWin' -as [type])) {
    Add-Type -Namespace '' -Name 'R0BWin' -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool SetForegroundWindow(System.IntPtr hWnd);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern System.IntPtr GetForegroundWindow();
'@
}

# Positive source check: the Desktop support preview must report Source: runtime.
# This reads only the fixed "Source:" line; it never records a path or report body.
function Get-SupportSource([int]$TargetProcessId) {
    $cond = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ProcessIdProperty, $TargetProcessId)
    $window = [System.Windows.Automation.AutomationElement]::RootElement.FindFirst(
        [System.Windows.Automation.TreeScope]::Children, $cond)
    if ($null -eq $window) { return $null }
    $handle = [IntPtr]$window.Current.NativeWindowHandle
    if ($handle -eq [IntPtr]::Zero) { return $null }
    [void][R0BWin]::SetForegroundWindow($handle)
    Start-Sleep -Milliseconds 200
    if ([R0BWin]::GetForegroundWindow() -ne $handle) { return $null }
    $panel = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::AutomationIdProperty, 'diagnostic-readonly')
    if ($null -eq $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $panel)) {
        [System.Windows.Forms.SendKeys]::SendWait('^d')
        Start-Sleep -Milliseconds 500
    }
    $buttons = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants,
        (New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
            [System.Windows.Automation.ControlType]::Button)))
    $previewButton = $null
    for ($i = 0; $i -lt $buttons.Count; $i++) {
        $candidate = $buttons.Item($i)
        if ($candidate.Current.Name -like '*サポート情報を表示*') { $previewButton = $candidate }
    }
    if ($null -ne $previewButton) {
        $previewButton.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
        Start-Sleep -Milliseconds 1500
    }
    $preview = $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
        (New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::AutomationIdProperty, 'support-preview-readonly')))
    if ($null -eq $preview) { return $null }
    $lines = $preview.Current.Name -split "`n"
    $source = ($lines | Where-Object { $_ -like 'Source:*' } | Select-Object -First 1)
    $connection = ($lines | Where-Object { $_ -like 'Connection:*' } | Select-Object -First 1)
    $stale = ($lines | Where-Object { $_ -like 'Stale projection:*' } | Select-Object -First 1)
    # Return only the minimum closed fields; no path or body is retained.
    return [ordered]@{ source = $source; connection = $connection; stale = $stale }
}

$runs_out = @()
$abort_hold = $false
for ($i = 1; $i -le $Runs; $i++) {
    # B-03: a unique run directory created exclusively; nothing pre-existing is reused and
    # only this directory is deleted. The child is reaped in `finally`.
    $run_dir = Join-Path $WorkRoot ("run-$i-" + [guid]::NewGuid().ToString('N'))
    if (Test-Path -LiteralPath $run_dir) { throw "run directory $run_dir already exists" }
    New-Item -ItemType Directory -Path $run_dir | Out-Null
    $stdout = Join-Path $run_dir 'stdout.txt'
    $stderr = Join-Path $run_dir 'stderr.txt'
    $process = $null
    try {
        $pre_run_sha = Get-FileHashText $Exe
        if ($pre_run_sha -ne $exe_pinned_sha) {
            $abort_hold = $true
            $runs_out += [ordered]@{
                run = $i
                hold = $true
                reason = 'exe hash changed before the run started'
                pinned_exe_sha256 = $exe_pinned_sha
                observed_exe_sha256 = $pre_run_sha
            }
            break
        }

        $isolated_local = Join-Path $run_dir 'localappdata'
        New-Item -ItemType Directory -Force -Path $isolated_local | Out-Null
        $previous_local = $env:LOCALAPPDATA
        $env:LOCALAPPDATA = $isolated_local
        $previous_env = $env:COS_DESKTOP_RUNTIME_ENDPOINT
        $env:COS_DESKTOP_RUNTIME_ENDPOINT = $Endpoint
        try {
            $launchArgs = if ($SimulateFixture) {
                @('--fixture', '--fixture-file', (Join-Path $PSScriptRoot '..\..\crates\cos-client\fixtures\task-184.json'))
            } else { @() }
            $process = Start-Process -FilePath $Exe -ArgumentList $launchArgs -PassThru -NoNewWindow `
                -RedirectStandardOutput $stdout -RedirectStandardError $stderr
        } finally {
            $env:COS_DESKTOP_RUNTIME_ENDPOINT = $previous_env
            $env:LOCALAPPDATA = $previous_local
        }

        $deadline = (Get-Date).AddSeconds(120)
        while ($process.MainWindowHandle -eq 0) {
            if ($process.HasExited) { throw "run ${i}: the app exited before a window appeared (code $($process.ExitCode))" }
            if ((Get-Date) -gt $deadline) { throw "run ${i}: no main window within 120 s" }
            Start-Sleep -Milliseconds 100
            $process.Refresh()
        }
        $window_ready_ms = [int](((Get-Date) - $process.StartTime).TotalMilliseconds)
        if ($RejectProbe) {
            # Fast negative-path probe only; never produces a baseline.
            Start-Sleep -Seconds 2
            $probe = Get-SupportSource $process.Id
            $probe_source = if ($null -eq $probe) { $null } else { $probe.source }
            $probe_connection = if ($null -eq $probe) { $null } else { $probe.connection }
            Write-Host "REJECT_PROBE source=$probe_source connection=$probe_connection fixture=$SimulateFixture"
            $closed = $process.CloseMainWindow(); $null = $process.WaitForExit(15000)
            if ($null -eq $probe) { exit 4 }
            if ($probe_source -ne 'Source: runtime' -or $probe_connection -ne 'Connection: observing') { exit 3 }
            exit 0
        }

        # Steady state: visible window plus the approved 30 s settle with no input and no
        # endpoint traffic (the endpoint is unreachable).
        Start-Sleep -Seconds $SettleSeconds
        $source_start = if ($Connected) { Get-SupportSource $process.Id } else { $null }

        $samples = @()
        $window_started = Get-Date
        $cpu_first = $null
        $cpu_previous = $null
        $window_deadline = $window_started.AddSeconds($WindowSeconds)
        $early_exit_at = if ($SimulateEarlyExitSeconds -gt 0) { $window_started.AddSeconds($SimulateEarlyExitSeconds) } else { $null }
        $window_completed = $true
        while ((Get-Date) -lt $window_deadline) {
            $process.Refresh()
            if ($process.HasExited) { $window_completed = $false; break }
            $cpu = $process.TotalProcessorTime.TotalSeconds
            $ws = [int64]$process.WorkingSet64
            $elapsed = ((Get-Date) - $window_started).TotalSeconds
            $samples += [ordered]@{
                at_s = [math]::Round($elapsed, 3)
                cpu_s = $cpu
                cpu_delta_s = if ($null -ne $cpu_previous) { [math]::Round($cpu - $cpu_previous, 6) } else { $null }
                working_set_bytes = $ws
            }
            if ($null -eq $cpu_first) { $cpu_first = $cpu }
            $cpu_previous = $cpu
            if ($null -ne $early_exit_at -and (Get-Date) -ge $early_exit_at) {
                $process.Kill()
                $null = $process.WaitForExit(5000)
                $window_completed = $false
                break
            }
            Start-Sleep -Milliseconds $SampleMs
        }
        $window_elapsed = ((Get-Date) - $window_started).TotalSeconds
        $observed_span = if ($samples.Count -ge 2) { $samples[-1]['at_s'] - $samples[0]['at_s'] } else { 0.0 }
        $process.Refresh()
        $cpu_total = if ($null -ne $cpu_first -and $null -ne $cpu_previous) { $cpu_previous - $cpu_first } else { 0.0 }
        $cpu_fraction = if ($window_elapsed -gt 0) { $cpu_total / $window_elapsed } else { 0.0 }
        # Source must be re-read while the process is still alive; a closed process
        # cannot prove that observation continued through the window.
        $source_end = if ($Connected) { Get-SupportSource $process.Id } else { $null }

        $cpu_deltas = [double[]]@($samples | Where-Object { $null -ne $_['cpu_delta_s'] } | ForEach-Object { [double]$_['cpu_delta_s'] })
        $ws_values = [double[]]@($samples | ForEach-Object { [double]$_['working_set_bytes'] })

        $closed = $process.CloseMainWindow()
        $exited = $process.WaitForExit(15000)
        if (-not $exited) { $process.Kill(); $null = $process.WaitForExit(5000) }
        $exit_code = if ($process.HasExited) { $process.ExitCode } else { $null }
        $post_run_sha = Get-FileHashText $Exe
        $hash_stable = ($post_run_sha -eq $exe_pinned_sha)
        if ($SimulateExeMismatch) { $post_run_sha = 'simulated-mismatch'; $hash_stable = $false }

        $stderr_lines = @()
        if (Test-Path -LiteralPath $stderr) { $stderr_lines = Get-Content -LiteralPath $stderr }
        $disconnected_line = $stderr_lines | Where-Object { $_ -like 'Runtime observation unavailable*' } | Select-Object -Last 1

        # B-01: the approved conditions are part of validity, including the observed span,
        # the sample count and the fact that the process survived the whole window.
        $expected_samples = [math]::Floor($WindowSeconds * 1000 / $SampleMs) - 5
        $source_ok = if ($Connected) {
            ($null -ne $source_start) -and ($null -ne $source_end) -and
            ($source_start.source -eq 'Source: runtime') -and
            ($source_start.connection -eq 'Connection: observing') -and
            ($source_end.source -eq 'Source: runtime') -and
            ($source_end.connection -eq 'Connection: observing') -and
            ($source_start.stale -eq 'Stale projection: no') -and
            ($source_end.stale -eq 'Stale projection: no') -and
            ($null -eq $disconnected_line) -and $endpoint_available
        } else {
            ($null -ne $disconnected_line) -and (-not $endpoint_available)
        }
        $valid = $hash_stable -and
                 ($exit_code -eq 0) -and
                 $window_completed -and
                 ($samples.Count -ge $expected_samples) -and
                 ($observed_span -ge ($WindowSeconds - 1.0)) -and
                 $source_ok

        $runs_out += [ordered]@{
            run = $i
            valid = $valid
            hold = (-not $hash_stable)
            reason = if (-not $hash_stable) { 'exe hash changed during or after the run' } else { $null }
            pinned_exe_sha256 = $exe_pinned_sha
            post_run_exe_sha256 = $post_run_sha
            endpoint = $Endpoint
            endpoint_available = $endpoint_available
            disconnected_line = $disconnected_line
            source_start = $source_start
            source_end = $source_end
            source_ok = $source_ok
            window_ready_ms = $window_ready_ms
            settle_seconds = $SettleSeconds
            window_seconds = $WindowSeconds
            sample_ms = $SampleMs
            samples = $samples
            samples_count = $samples.Count
            expected_samples = $expected_samples
            window_elapsed_s = [math]::Round($window_elapsed, 3)
            observed_span_s = [math]::Round($observed_span, 3)
            window_completed = $window_completed
            cpu_total_s = [math]::Round($cpu_total, 6)
            cpu_fraction_of_one_core = [math]::Round($cpu_fraction, 6)
            cpu_delta_s = Get-Statistics $cpu_deltas
            working_set = Get-Statistics $ws_values
            close_requested = $closed
            exited = $exited
            exit_code = $exit_code
        }
        if (Test-Path -LiteralPath $stderr) { Copy-Item -LiteralPath $stderr -Destination (Join-Path $EvidenceDir ("r0b-idle-$stamp-run$i.stderr.txt")) -Force }
    } finally {
        if ($null -ne $process -and -not $process.HasExited) {
            $process.Kill()
            $null = $process.WaitForExit(5000)
        }
        # Delete only the exclusively created run directory of this iteration.
        if (Test-Path -LiteralPath $run_dir) { Remove-Item -LiteralPath $run_dir -Recurse -Force -ErrorAction SilentlyContinue }
    }
    if ($abort_hold) { break }
}

$valid_runs = @($runs_out | Where-Object { $_['valid'] -eq $true })
$hold = $abort_hold -or (@($runs_out | Where-Object { $_['hold'] -eq $true }).Count -gt 0)
$cpu_metrics = [double[]]@($valid_runs | ForEach-Object { [double]$_['cpu_fraction_of_one_core'] })
$ws_metrics = [double[]]@($valid_runs | ForEach-Object { [double]$_['working_set']['mean'] })
$summary = [ordered]@{
    class = if ($Connected) { 'MP-R0B connected runtime idle baseline' } else { 'MP-R0B idle baseline, runtime mode against an unreachable loopback endpoint (disconnected UI)' }
    exe = $Exe
    exe_sha256_pinned_before_launch = $exe_pinned_sha
    source = [ordered]@{
        git_head = if ($null -eq $git_head) { $null } else { "$git_head".Trim() }
        dirty = $git_dirty
        dirty_entry_count = $git_status.Count
        build_kind = $BuildKind
    }
    endpoint = $Endpoint
    endpoint_available = $endpoint_available
    endpoint_label = if ($endpoint_available) { if ($Connected) { 'available: connected runtime' } else { 'available (not used: disconnected variant)' } } else { 'unavailable (connection refused): disconnected baseline' }
    conditions = "fresh process per run; window visible; ${SettleSeconds} s settle; ${WindowSeconds} s observation; ${SampleMs} ms sampling; no input; no endpoint traffic"
    required = [ordered]@{
        runs = 5
        settle_seconds = 30
        window_seconds = 60
        sample_ms = 500
        min_samples = [math]::Floor($WindowSeconds * 1000 / $SampleMs) - 5
        min_observed_span_s = $WindowSeconds - 1.0
    }
    runs = $runs_out
    valid_runs = $valid_runs.Count
    invalid_runs = $runs_out.Count - $valid_runs.Count
    hold = $hold
    cpu_fraction_of_one_core = Get-Statistics $cpu_metrics
    working_set_mean_bytes = Get-Statistics $ws_metrics
    measurement_limit = if ($Connected) { 'Connected Runtime idle baseline: Source: runtime was required at both ends; CPU/Working Set only, not frame time/SLO/GPU/Present.' } else { 'Disconnected baseline only: the endpoint was unreachable, so nothing here is a connected-endpoint baseline. CPU is TotalProcessorTime delta divided by the observation window (fraction of one core), not a frame time or an SLO. Working Set is process-wide and does not cover GPU/render timing. Every sample series is stored in this record so the aggregates can be recomputed.' }
    sampled_at_utc = $stamp
}

$json_path = Join-Path $EvidenceDir ("r0b-idle-$stamp.json")
$txt_path = Join-Path $EvidenceDir ("r0b-idle-$stamp.txt")
$summary | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $json_path -Encoding UTF8
@(
    "class: $($summary.class)",
    "exe_sha256 (pinned before launch): $exe_pinned_sha",
    "source: HEAD $($summary.source.git_head) dirty=$($summary.source.dirty) entries=$($summary.source.dirty_entry_count) build=$($summary.source.build_kind)",
    "endpoint: $Endpoint ($($summary.endpoint_label))",
    "runs: $($runs_out.Count) valid=$($summary.valid_runs) invalid=$($summary.invalid_runs) hold=$hold",
    "conditions: $($summary.conditions)",
    "cpu: median=$($summary.cpu_fraction_of_one_core.median) mean=$($summary.cpu_fraction_of_one_core.mean) stddev=$($summary.cpu_fraction_of_one_core.stddev) noise=$($summary.cpu_fraction_of_one_core.noise_ratio)",
    "ws: median=$($summary.working_set_mean_bytes.median) mean=$($summary.working_set_mean_bytes.mean) stddev=$($summary.working_set_mean_bytes.stddev) noise=$($summary.working_set_mean_bytes.noise_ratio)"
) | Set-Content -LiteralPath $txt_path -Encoding UTF8

Write-Host "record: $json_path"
foreach ($run in $runs_out) {
    if ($run['hold']) { Write-Host ("run {0}: HOLD {1}" -f $run['run'], $run['reason']); continue }
    Write-Host ("run {0}: valid={1} samples={2} span={3}s cpu_fraction={4} ws_mean={5} ws_noise={6} exit={7}" -f `
        $run['run'], $run['valid'], $run['samples_count'], $run['observed_span_s'], $run['cpu_fraction_of_one_core'], $run['working_set']['mean'], $run['working_set']['noise_ratio'], $run['exit_code'])
}
if ($hold) { exit 3 }
if ($summary.invalid_runs -ne 0 -or $summary.valid_runs -ne 5) { exit 1 }
Write-Host $(if ($Connected) { "PASS_LIMITED (connected runtime baseline)" } else { "PASS_LIMITED (disconnected baseline)" })
exit 0
