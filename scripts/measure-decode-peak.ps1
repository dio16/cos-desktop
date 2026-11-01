param(
    [string]$Exe = "C:\dev\cos-desktop\target\debug\examples\peak_probe.exe",
    [int]$Runs = 5,
    [int]$Width = 2048,
    [int]$Height = 2048,
    [int]$SampleMs = 10,
    [int]$MinDecodeSamples = 3,
    [int]$ReadyTimeoutSeconds = 120,
    [string]$EvidenceDir = "C:\dev\cos-desktop-verification\rel1-2026-10-30",
    [string]$WorkRoot = ""
)

$ErrorActionPreference = 'Stop'

# The decode-phase sample condition is part of the measurement validity, not a tunable
# knob: a value below 3 would let a run pass with no observed decode samples.
if ($MinDecodeSamples -lt 3) {
    [Console]::Error.WriteLine("-MinDecodeSamples must be at least 3 (got $MinDecodeSamples); a lower value would disable the decode-phase sample condition")
    exit 2
}

$stamp = (Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssZ')
New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null
if ([string]::IsNullOrWhiteSpace($WorkRoot)) {
    $WorkRoot = Join-Path $env:TEMP "cos-decode-peak-$stamp"
}
New-Item -ItemType Directory -Force -Path $WorkRoot | Out-Null

# Declared admission ceiling for this class: 2 x RGBA8 output + declared slack.
$ceiling = 2 * [int64]$Width * [int64]$Height * 4 + 1048576

function Get-WorkingSet([int]$ProcessId) {
    try {
        $process = Get-Process -Id $ProcessId -ErrorAction Stop
        return [int64]$process.WorkingSet64
    } catch {
        return $null
    }
}

$expected_rgba = [int64]$Width * [int64]$Height * 4
$runs_out = @()
for ($i = 1; $i -le $Runs; $i++) {
    $run_dir = Join-Path $WorkRoot "run-$i"
    New-Item -ItemType Directory -Force -Path $run_dir | Out-Null
    $ready = Join-Path $run_dir 'ready'
    $go = Join-Path $run_dir 'go'
    $stdout = Join-Path $run_dir 'stdout.txt'
    $stderr = Join-Path $run_dir 'stderr.txt'
    $decode_start_marker = Join-Path $run_dir 'decode-start'
    $decode_done_marker = Join-Path $run_dir 'decode-done'

    $started = Get-Date
    $process = Start-Process -FilePath $Exe `
        -ArgumentList @($Width, $Height, $ready, $go, $decode_start_marker, $decode_done_marker) `
        -PassThru -NoNewWindow `
        -RedirectStandardOutput $stdout -RedirectStandardError $stderr

    # Wait for the probe to finish generating the fixture (it then waits for the go file).
    $deadline = (Get-Date).AddSeconds($ReadyTimeoutSeconds)
    while (-not (Test-Path -LiteralPath $ready)) {
        if ((Get-Date) -gt $deadline) { throw "run ${i}: the probe did not become ready within ${ReadyTimeoutSeconds} s" }
        if ($process.HasExited) { throw "run ${i}: the probe exited before it was ready (code $($process.ExitCode))" }
        Start-Sleep -Milliseconds 10
    }

    # Baseline: the pre-decode working set, sampled before the go file releases the decode.
    $baseline_samples = @()
    for ($s = 0; $s -lt 5; $s++) {
        $value = Get-WorkingSet $process.Id
        if ($null -ne $value) { $baseline_samples += $value }
        Start-Sleep -Milliseconds $SampleMs
    }
    if ($baseline_samples.Count -eq 0) { throw "run ${i}: no working-set samples before the decode" }
    $baseline = ($baseline_samples | Sort-Object)[0]  # minimum of the pre-decode samples

    Set-Content -LiteralPath $go -Value 'go'

    $peak = $baseline
    $samples = 0
    $decode_samples = 0
    $last = $baseline
    while (-not $process.HasExited) {
        # The decode interval is open when the start marker exists and the done marker does
        # not. Check it both immediately before and immediately after the sample, so a
        # decode that starts or ends between the two checks cannot be counted.
        $before_in_decode = (Test-Path -LiteralPath $decode_start_marker) -and -not (Test-Path -LiteralPath $decode_done_marker)
        $value = Get-WorkingSet $process.Id
        $after_in_decode = (Test-Path -LiteralPath $decode_start_marker) -and -not (Test-Path -LiteralPath $decode_done_marker)
        if ($null -ne $value) {
            $samples++
            $last = $value
            if ($value -gt $peak) { $peak = $value }
            if ($before_in_decode -and $after_in_decode) {
                $decode_samples++
            }
        }
        Start-Sleep -Milliseconds $SampleMs
    }
    $process.WaitForExit()
    $exit_code = $process.ExitCode
    $elapsed_ms = [int]((Get-Date) - $started).TotalMilliseconds

    $increment = $peak - $baseline
    $out_lines = @()
    if (Test-Path -LiteralPath $stdout) { $out_lines = Get-Content -LiteralPath $stdout }
    $done_line = $out_lines | Where-Object { $_ -like 'decode-done*' } | Select-Object -Last 1
    $rgba_bytes = $null
    if ($null -ne $done_line -and $done_line -match 'rgba-bytes=([0-9]+)') {
        $rgba_bytes = [int64]$Matches[1]
    }
    # A run only counts when the probe exited normally, reported the complete RGBA8
    # output, and was sampled often enough during the decode; otherwise the measured
    # increment is meaningless and the whole verdict is FAIL, never PASS.
    $valid = ($exit_code -eq 0) -and
             ($rgba_bytes -eq $expected_rgba) -and
             ($decode_samples -ge $MinDecodeSamples) -and
             ($null -ne $done_line)

    $runs_out += [ordered]@{
        run = $i
        baseline_bytes = $baseline
        baseline_samples = $baseline_samples
        peak_bytes = $peak
        end_bytes = $last
        increment_bytes = $increment
        ceiling_bytes = $ceiling
        within_ceiling = ($increment -le $ceiling)
        samples = $samples
        decode_samples = $decode_samples
        min_decode_samples = $MinDecodeSamples
        expected_rgba_bytes = $expected_rgba
        rgba_bytes = $rgba_bytes
        valid = $valid
        exit_code = $exit_code
        elapsed_ms = $elapsed_ms
        decode_done = $done_line
        ready = (Test-Path -LiteralPath $ready)
    }
    Remove-Item -LiteralPath $run_dir -Recurse -Force -ErrorAction SilentlyContinue
}

$valid_runs = @($runs_out | Where-Object { $_['valid'] -eq $true })
$invalid_runs = @($runs_out | Where-Object { $_['valid'] -ne $true })
$increments = @($valid_runs | ForEach-Object { [int64]$_['increment_bytes'] } | Sort-Object)
$median = $null
$maximum = $null
if ($increments.Count -gt 0) {
    $median = $increments[[int][math]::Floor(($increments.Count - 1) / 2)]
    $maximum = $increments[-1]
}
# PASS requires every run to be valid (normal exit, complete RGBA8 output, >=3 decode
# samples) and every valid increment to fit the declared ceiling.
$verdict = if ($invalid_runs.Count -eq 0 -and $null -ne $maximum -and $maximum -le $ceiling) {
    'PASS'
} else {
    'FAIL'
}

$record = [ordered]@{
    class = "static 8-bit RGB PNG ${Width}x${Height} -> RGBA8"
    exe = $Exe
    exe_sha256 = (Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash.ToLowerInvariant()
    ceiling_bytes = $ceiling
    runs = $runs_out
    valid_runs = $valid_runs.Count
    invalid_runs = $invalid_runs.Count
    median_increment_bytes = $median
    max_increment_bytes = $maximum
    verdict = $verdict
    measurement_limit = 'Working Set is process-wide (includes the in-process generated fixture, runtime and mapping pages); it is not an allocation peak. Sampling interval is 10 ms, so a shorter peak can be missed. Decode-phase samples are those observed while the decode-start marker exists and the decode-done marker does not; the boundary is coarse at one sampling interval. No OS setting and no credential is touched.'
    sampled_at_utc = $stamp
}
$json_path = Join-Path $EvidenceDir ("decode-peak-$stamp.json")
$txt_path = Join-Path $EvidenceDir ("decode-peak-$stamp.txt")
$record | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $json_path -Encoding UTF8
@(
    "class: $($record.class)",
    "ceiling_bytes: $ceiling",
    "runs: $($runs_out.Count)",
    "valid_runs: $($valid_runs.Count)",
    "invalid_runs: $($invalid_runs.Count)",
    "median_increment_bytes: $median",
    "max_increment_bytes: $maximum",
    "verdict: $verdict",
    "measurement_limit: $($record.measurement_limit)"
) | Set-Content -LiteralPath $txt_path -Encoding UTF8

Write-Host "record: $json_path"
Write-Host "summary: $txt_path"
foreach ($run in $runs_out) {
    Write-Host ("run {0}: baseline={1} peak={2} increment={3} within_ceiling={4} exit={5} samples={6} decode_samples={7} valid={8}" -f `
        $run['run'], $run['baseline_bytes'], $run['peak_bytes'], $run['increment_bytes'], $run['within_ceiling'], $run['exit_code'], $run['samples'], $run['decode_samples'], $run['valid'])
}
Write-Host "median_increment_bytes=$median max_increment_bytes=$maximum verdict=$verdict"
if ($verdict -ne 'PASS') { exit 1 }
