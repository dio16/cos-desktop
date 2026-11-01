param(
    [string]$Exe = "C:\dev\cos-desktop\target\debug\examples\peak_probe.exe",
    [int]$Runs = 5,
    [int]$Width = 2048,
    [int]$Height = 2048,
    [int]$SampleMs = 10,
    [int]$ReadyTimeoutSeconds = 120,
    [string]$EvidenceDir = "C:\dev\cos-desktop-verification\rel1-2026-10-30",
    [string]$WorkRoot = ""
)

$ErrorActionPreference = 'Stop'
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

$runs_out = @()
for ($i = 1; $i -le $Runs; $i++) {
    $run_dir = Join-Path $WorkRoot "run-$i"
    New-Item -ItemType Directory -Force -Path $run_dir | Out-Null
    $ready = Join-Path $run_dir 'ready'
    $go = Join-Path $run_dir 'go'
    $stdout = Join-Path $run_dir 'stdout.txt'
    $stderr = Join-Path $run_dir 'stderr.txt'

    $started = Get-Date
    $process = Start-Process -FilePath $Exe `
        -ArgumentList @($Width, $Height, $ready, $go) `
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
    $last = $baseline
    while (-not $process.HasExited) {
        $value = Get-WorkingSet $process.Id
        if ($null -ne $value) {
            $samples++
            $last = $value
            if ($value -gt $peak) { $peak = $value }
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
        exit_code = $exit_code
        elapsed_ms = $elapsed_ms
        decode_done = $done_line
        ready = (Test-Path -LiteralPath $ready)
    }
    Remove-Item -LiteralPath $run_dir -Recurse -Force -ErrorAction SilentlyContinue
}

$increments = @($runs_out | ForEach-Object { [int64]$_['increment_bytes'] } | Sort-Object)
$median = $increments[[int][math]::Floor(($increments.Count - 1) / 2)]
$maximum = $increments[-1]
$verdict = if ($maximum -le $ceiling) { 'PASS' } else { 'HOLD' }

$record = [ordered]@{
    class = "static 8-bit RGB PNG ${Width}x${Height} -> RGBA8"
    exe = $Exe
    exe_sha256 = (Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash.ToLowerInvariant()
    ceiling_bytes = $ceiling
    runs = $runs_out
    median_increment_bytes = $median
    max_increment_bytes = $maximum
    verdict = $verdict
    measurement_limit = 'Working Set is process-wide (includes the in-process generated fixture, runtime and mapping pages); it is not an allocation peak. Sampling interval is 10 ms, so a shorter peak can be missed. No OS setting and no credential is touched.'
    sampled_at_utc = $stamp
}
$json_path = Join-Path $EvidenceDir ("decode-peak-$stamp.json")
$txt_path = Join-Path $EvidenceDir ("decode-peak-$stamp.txt")
$record | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $json_path -Encoding UTF8
@(
    "class: $($record.class)",
    "ceiling_bytes: $ceiling",
    "runs: $($runs_out.Count)",
    "median_increment_bytes: $median",
    "max_increment_bytes: $maximum",
    "verdict: $verdict",
    "measurement_limit: $($record.measurement_limit)"
) | Set-Content -LiteralPath $txt_path -Encoding UTF8

Write-Host "record: $json_path"
Write-Host "summary: $txt_path"
foreach ($run in $runs_out) {
    Write-Host ("run {0}: baseline={1} peak={2} increment={3} within_ceiling={4} exit={5} samples={6}" -f `
        $run['run'], $run['baseline_bytes'], $run['peak_bytes'], $run['increment_bytes'], $run['within_ceiling'], $run['exit_code'], $run['samples'])
}
Write-Host "median_increment_bytes=$median max_increment_bytes=$maximum verdict=$verdict"
if ($verdict -ne 'PASS') { exit 1 }
