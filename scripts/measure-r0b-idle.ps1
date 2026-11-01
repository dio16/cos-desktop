param(
    [string]$Exe = "C:\dev\cos-desktop\target\debug\cos-desktop.exe",
    [int]$Runs = 5,
    [string]$Endpoint = "http://127.0.0.1:9",
    [int]$SettleSeconds = 10,
    [int]$WindowSeconds = 30,
    [int]$SampleMs = 500,
    [int]$WindowTimeoutSeconds = 120,
    [string]$EvidenceDir = "C:\dev\cos-desktop-verification\rel1-2026-10-30",
    [string]$WorkRoot = ""
)

# MP-R0B idle baseline, disconnected variant: the app runs in runtime mode against an
# unreachable loopback endpoint, so every sample describes the **disconnected** UI. This
# is not a connected-endpoint baseline and does not close MP-R0B.
$ErrorActionPreference = 'Stop'
if ($SettleSeconds -lt 0) { [Console]::Error.WriteLine("-SettleSeconds must be >= 0"); exit 2 }
if ($WindowSeconds -lt 1) { [Console]::Error.WriteLine("-WindowSeconds must be >= 1"); exit 2 }
if ($SampleMs -lt 50) { [Console]::Error.WriteLine("-SampleMs must be >= 50"); exit 2 }

$stamp = (Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssZ')
New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null
if ([string]::IsNullOrWhiteSpace($WorkRoot)) { $WorkRoot = Join-Path $env:TEMP "cos-r0b-idle-$stamp" }
New-Item -ItemType Directory -Force -Path $WorkRoot | Out-Null

function Test-EndpointAvailable([string]$Url) {
    try {
        $uri = [Uri]$Url
        $client = [System.Net.Sockets.TcpClient]::new()
        $task = $client.ConnectAsync($uri.Host, $uri.Port)
        $connected = $task.Wait(1000) -and $client.Connected
        $client.Dispose()
        return $connected
    } catch {
        return $false
    }
}

function Get-Median([double[]]$Values) {
    if ($Values.Count -eq 0) { return $null }
    $sorted = $Values | Sort-Object
    return $sorted[[int][math]::Floor(($sorted.Count - 1) / 2)]
}

function Get-StdDev([double[]]$Values) {
    if ($Values.Count -lt 2) { return 0.0 }
    $mean = ($Values | Measure-Object -Average).Average
    $sum = 0.0
    foreach ($value in $Values) { $sum += ($value - $mean) * ($value - $mean) }
    return [math]::Sqrt($sum / ($Values.Count - 1))
}

$endpoint_available = Test-EndpointAvailable $Endpoint
$runs_out = @()
for ($i = 1; $i -le $Runs; $i++) {
    $run_dir = Join-Path $WorkRoot "run-$i"
    New-Item -ItemType Directory -Force -Path $run_dir | Out-Null
    $stdout = Join-Path $run_dir 'stdout.txt'
    $stderr = Join-Path $run_dir 'stderr.txt'

    $previous_env = $env:COS_DESKTOP_RUNTIME_ENDPOINT
    $env:COS_DESKTOP_RUNTIME_ENDPOINT = $Endpoint
    try {
        $process = Start-Process -FilePath $Exe -PassThru -NoNewWindow `
            -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    } finally {
        $env:COS_DESKTOP_RUNTIME_ENDPOINT = $previous_env
    }

    $deadline = (Get-Date).AddSeconds($WindowTimeoutSeconds)
    while ($process.MainWindowHandle -eq 0) {
        if ($process.HasExited) { throw "run ${i}: the app exited before a window appeared (code $($process.ExitCode))" }
        if ((Get-Date) -gt $deadline) { throw "run ${i}: no main window within ${WindowTimeoutSeconds} s" }
        Start-Sleep -Milliseconds 100
        $process.Refresh()
    }
    $window_ready_ms = [int](((Get-Date) - $process.StartTime).TotalMilliseconds)

    # Steady state: the window exists and the configured settle interval has passed with no
    # input and no endpoint traffic (the endpoint is unreachable). Only then does the
    # observation window open.
    Start-Sleep -Seconds $SettleSeconds

    $samples = @()
    $window_started = Get-Date
    $cpu_first = $null
    $cpu_previous = $null
    $window_deadline = $window_started.AddSeconds($WindowSeconds)
    while ((Get-Date) -lt $window_deadline) {
        $process.Refresh()
        if ($process.HasExited) { break }
        $cpu = $process.TotalProcessorTime.TotalSeconds
        $ws = [int64]$process.WorkingSet64
        $elapsed = ((Get-Date) - $window_started).TotalSeconds
        $samples += [ordered]@{
            at_s = [math]::Round($elapsed, 3)
            cpu_s = $cpu
            cpu_delta_s = if ($null -ne $cpu_previous) { $cpu - $cpu_previous } else { $null }
            working_set_bytes = $ws
        }
        if ($null -eq $cpu_first) { $cpu_first = $cpu }
        $cpu_previous = $cpu
        Start-Sleep -Milliseconds $SampleMs
    }
    $window_elapsed = ((Get-Date) - $window_started).TotalSeconds
    $process.Refresh()
    $cpu_total = if ($null -ne $cpu_first) { $cpu_previous - $cpu_first } else { 0.0 }
    $cpu_fraction = if ($window_elapsed -gt 0) { $cpu_total / $window_elapsed } else { 0.0 }

    $cpu_deltas = @($samples | Where-Object { $null -ne $_['cpu_delta_s'] } | ForEach-Object { [double]$_['cpu_delta_s'] })
    $ws_values = [double[]]@($samples | ForEach-Object { [double]$_['working_set_bytes'] })

    $closed = $process.CloseMainWindow()
    $exited = $process.WaitForExit(15000)
    if (-not $exited) { $process.Kill(); $null = $process.WaitForExit(5000) }
    $exit_code = if ($process.HasExited) { $process.ExitCode } else { $null }
    $stderr_lines = @()
    if (Test-Path -LiteralPath $stderr) { $stderr_lines = Get-Content -LiteralPath $stderr }
    $disconnected_line = $stderr_lines | Where-Object { $_ -like 'Runtime observation unavailable*' } | Select-Object -Last 1

    $ws_mean = if ($ws_values.Count -gt 0) { ($ws_values | Measure-Object -Average).Average } else { $null }
    $ws_median = Get-Median $ws_values
    $ws_stddev = Get-StdDev $ws_values
    $ws_noise = if ($ws_mean -and $ws_mean -gt 0) { $ws_stddev / $ws_mean } else { $null }
    $cpu_delta_mean = if ($cpu_deltas.Count -gt 0) { ($cpu_deltas | Measure-Object -Average).Average } else { $null }
    $cpu_delta_stddev = Get-StdDev $cpu_deltas
    $valid = ($exit_code -eq 0) -and ($samples.Count -ge 45) -and ($null -ne $ws_mean) -and
             (-not [string]::IsNullOrWhiteSpace($disconnected_line)) -and (-not $endpoint_available)

    $runs_out += [ordered]@{
        run = $i
        valid = $valid
        endpoint = $Endpoint
        endpoint_available = $endpoint_available
        disconnected_line = $disconnected_line
        window_ready_ms = $window_ready_ms
        settle_seconds = $SettleSeconds
        window_seconds = $WindowSeconds
        sample_ms = $SampleMs
        samples = $samples.Count
        window_elapsed_s = [math]::Round($window_elapsed, 3)
        cpu_total_s = [math]::Round($cpu_total, 6)
        cpu_fraction_of_one_core = [math]::Round($cpu_fraction, 6)
        cpu_delta_mean_s = $cpu_delta_mean
        cpu_delta_stddev_s = $cpu_delta_stddev
        working_set_mean_bytes = $ws_mean
        working_set_median_bytes = $ws_median
        working_set_stddev_bytes = $ws_stddev
        working_set_noise_ratio = $ws_noise
        close_requested = $closed
        exited = $exited
        exit_code = $exit_code
    }
    # Keep the run's stderr/stdout next to the evidence, then drop the scratch directory.
    if (Test-Path -LiteralPath $stderr) { Copy-Item -LiteralPath $stderr -Destination (Join-Path $EvidenceDir ("r0b-idle-$stamp-run$i.stderr.txt")) -Force }
    Remove-Item -LiteralPath $run_dir -Recurse -Force -ErrorAction SilentlyContinue
}

$valid_runs = @($runs_out | Where-Object { $_['valid'] -eq $true })
$cpu_metrics = [double[]]@($valid_runs | ForEach-Object { [double]$_['cpu_fraction_of_one_core'] })
$ws_metrics = [double[]]@($valid_runs | ForEach-Object { [double]$_['working_set_mean_bytes'] })
$summary = [ordered]@{
    class = 'MP-R0B idle baseline, runtime mode against an unreachable loopback endpoint (disconnected UI)'
    exe = $Exe
    exe_sha256 = (Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash.ToLowerInvariant()
    endpoint = $Endpoint
    endpoint_available = $endpoint_available
    endpoint_label = if ($endpoint_available) { 'available' } else { 'unavailable (connection refused): disconnected baseline' }
    conditions = "fresh process per run; window visible; ${SettleSeconds} s settle; ${WindowSeconds} s observation window; ${SampleMs} ms sampling; no input; no endpoint traffic"
    runs = $runs_out
    valid_runs = $valid_runs.Count
    invalid_runs = $runs_out.Count - $valid_runs.Count
    cpu_fraction_of_one_core = [ordered]@{
        median = Get-Median $cpu_metrics
        mean = if ($cpu_metrics.Count -gt 0) { ($cpu_metrics | Measure-Object -Average).Average } else { $null }
        stddev = Get-StdDev $cpu_metrics
        noise_ratio = if ($cpu_metrics.Count -gt 0 -and (($cpu_metrics | Measure-Object -Average).Average) -gt 0) { (Get-StdDev $cpu_metrics) / (($cpu_metrics | Measure-Object -Average).Average) } else { $null }
        count = $cpu_metrics.Count
    }
    working_set_mean_bytes = [ordered]@{
        median = Get-Median $ws_metrics
        mean = if ($ws_metrics.Count -gt 0) { ($ws_metrics | Measure-Object -Average).Average } else { $null }
        stddev = Get-StdDev $ws_metrics
        noise_ratio = if ($ws_metrics.Count -gt 0 -and (($ws_metrics | Measure-Object -Average).Average) -gt 0) { (Get-StdDev $ws_metrics) / (($ws_metrics | Measure-Object -Average).Average) } else { $null }
        count = $ws_metrics.Count
    }
    measurement_limit = 'This is the disconnected baseline only: the endpoint was unreachable, so nothing here is a connected-endpoint baseline. CPU is TotalProcessorTime delta divided by the observation window (fraction of one core), not a frame time or an SLO. Working Set is process-wide sampled at the configured interval and does not cover GPU/render timing.'
    sampled_at_utc = $stamp
}

$json_path = Join-Path $EvidenceDir ("r0b-idle-$stamp.json")
$txt_path = Join-Path $EvidenceDir ("r0b-idle-$stamp.txt")
$summary | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $json_path -Encoding UTF8
@(
    "class: $($summary.class)",
    "exe_sha256: $($summary.exe_sha256)",
    "endpoint: $Endpoint ($($summary.endpoint_label))",
    "runs: $($runs_out.Count) (valid $($summary.valid_runs) / invalid $($summary.invalid_runs))",
    "conditions: $($summary.conditions)",
    "cpu_fraction_of_one_core: median=$($summary.cpu_fraction_of_one_core.median) mean=$($summary.cpu_fraction_of_one_core.mean) stddev=$($summary.cpu_fraction_of_one_core.stddev) noise=$($summary.cpu_fraction_of_one_core.noise_ratio)",
    "working_set_mean_bytes: median=$($summary.working_set_mean_bytes.median) mean=$($summary.working_set_mean_bytes.mean) stddev=$($summary.working_set_mean_bytes.stddev) noise=$($summary.working_set_mean_bytes.noise_ratio)"
) | Set-Content -LiteralPath $txt_path -Encoding UTF8

Write-Host "record: $json_path"
foreach ($run in $runs_out) {
    Write-Host ("run {0}: valid={1} samples={2} cpu_fraction={3} cpu_total_s={4} ws_mean={5} ws_noise={6} exit={7} window_ready_ms={8}" -f `
        $run['run'], $run['valid'], $run['samples'], $run['cpu_fraction_of_one_core'], $run['cpu_total_s'], $run['working_set_mean_bytes'], $run['working_set_noise_ratio'], $run['exit_code'], $run['window_ready_ms'])
}
Write-Host ("cpu: median={0} mean={1} stddev={2} noise={3}" -f $summary.cpu_fraction_of_one_core.median, $summary.cpu_fraction_of_one_core.mean, $summary.cpu_fraction_of_one_core.stddev, $summary.cpu_fraction_of_one_core.noise_ratio)
Write-Host ("ws: median={0} mean={1} stddev={2} noise={3}" -f $summary.working_set_mean_bytes.median, $summary.working_set_mean_bytes.mean, $summary.working_set_mean_bytes.stddev, $summary.working_set_mean_bytes.noise_ratio)
if ($summary.invalid_runs -ne 0) { exit 1 }
