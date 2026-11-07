# MP-REC2 external observer. Process/GPU memory gauges are proxies, not atlas residency.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Exe,
    [Parameter(Mandatory)][string]$EvidenceDir,
    [ValidateSet('shared', 'independent', 'distinct', 'windows2')][string]$Variant = 'shared',
    [ValidateRange(1, 10)][int]$Runs = 5,
    [switch]$Recovery
)
$ErrorActionPreference = 'Stop'
$maxCapturedOutputBytes = 1MB
if (-not ('Rec2BoundedOutput' -as [type])) {
    Add-Type -Language CSharp -TypeDefinition @"
using System;
using System.IO;
using System.Threading.Tasks;

public sealed class Rec2CapturedOutput {
    private readonly byte[] buffer;
    public int Length { get; private set; }
    public bool Truncated { get; private set; }
    public Rec2CapturedOutput(byte[] buffer, int length, bool truncated) {
        this.buffer = buffer;
        Length = length;
        Truncated = truncated;
    }
    public void WriteTo(string path) {
        using (var file = new FileStream(path, FileMode.Create, FileAccess.Write, FileShare.Read)) {
            file.Write(buffer, 0, Length);
        }
    }
}

public static class Rec2BoundedOutput {
    public static async Task<Rec2CapturedOutput> DrainAsync(Stream input, int limit) {
        var captured = new byte[limit];
        var scratch = new byte[65536];
        var length = 0;
        var truncated = false;
        int read;
        while ((read = await input.ReadAsync(scratch, 0, scratch.Length).ConfigureAwait(false)) != 0) {
            var keep = Math.Min(read, limit - length);
            if (keep > 0) {
                Buffer.BlockCopy(scratch, 0, captured, length, keep);
                length += keep;
            }
            if (keep < read) truncated = true;
        }
        return new Rec2CapturedOutput(captured, length, truncated);
    }
}
"@ -ErrorAction Stop
}

$root = Split-Path -Parent $PSScriptRoot
$exePath = [IO.Path]::GetFullPath((Resolve-Path -LiteralPath $Exe).Path)
$evidencePath = [IO.Path]::GetFullPath($EvidenceDir)
if (Test-Path -LiteralPath $evidencePath) { throw 'EvidenceDir must be a new directory; previous observations are never overwritten' }
$source = (& git -C $root rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve source commit' }
$tracked = @(& git -C $root status --porcelain --untracked-files=no)
if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect tracked source state' }
if ($tracked.Count -ne 0) { throw 'Cannot sample a tracked-dirty source candidate' }
New-Item -ItemType Directory -Path $evidencePath -ErrorAction Stop | Out-Null
$binaryHash = (Get-FileHash -LiteralPath $exePath -Algorithm SHA256).Hash.ToLowerInvariant()
$driverHash = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash.ToLowerInvariant()
$cargo = Get-Content -LiteralPath (Join-Path $root 'Cargo.toml') -Raw
$gpuiPin = [regex]::Match($cargo, 'gpui\s*=.*?rev\s*=\s*"([0-9a-f]{40})"').Groups[1].Value
if ($gpuiPin.Length -ne 40) { throw 'Cannot resolve fixed GPUI revision' }
$clockFrequency = [Diagnostics.Stopwatch]::Frequency
$runRecords = [Collections.Generic.List[object]]::new()
$operatingSystem = Get-CimInstance Win32_OperatingSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object Name, NumberOfLogicalProcessors
$gpu = Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion, DriverDate
$power = (& powercfg /getactivescheme) -join "`n"

function Get-AdapterMemorySnapshot {
    try {
        $samples = @((Get-Counter -Counter '\GPU Adapter Memory(*)\Dedicated Usage', '\GPU Adapter Memory(*)\Shared Usage', '\GPU Adapter Memory(*)\Total Committed' -MaxSamples 1 -ErrorAction Stop).CounterSamples)
        $required = @('Dedicated Usage', 'Shared Usage', 'Total Committed')
        $seen = @{}
        $valid = $samples.Count -gt 0
        $sampleRecords = [Collections.Generic.List[object]]::new()
        foreach ($sample in $samples) {
            $counterName = $sample.Path.Substring($sample.Path.LastIndexOf('\') + 1)
            $seen[$counterName] = $true
            $value = [double]$sample.CookedValue
            $valueValid = $sample.Status -eq 0 -and [double]::IsFinite($value) -and $value -ge 0 -and $value -lt 9223372036854775808.0
            if (-not $valueValid) { $valid = $false }
            $sampleRecords.Add(@{
                instance = $sample.InstanceName
                counter = $counterName
                bytes = if ($valueValid) { [long]$value } else { $null }
                cooked_value = if ([double]::IsFinite($value)) { $value } else { [string]$value }
                path = $sample.Path
                status = $sample.Status
            })
        }
        foreach ($counterName in $required) {
            if (-not $seen.ContainsKey($counterName)) { $valid = $false }
        }
        return @{
            status = if ($valid) { 'OBSERVED' } else { 'NOT_MEASURED' }
            reason = if ($valid) { $null } else { 'PDH system adapter counters missing or invalid' }
            utc = [DateTime]::UtcNow.ToString('o')
            samples = @($sampleRecords.ToArray())
            meaning = 'System-wide per-adapter gauges, including other processes and driver/DWM allocations'
        }
    } catch {
        return @{ status = 'NOT_MEASURED'; reason = 'PDH system adapter counters unavailable' }
    }
}

$metadata = [ordered]@{
    manifest_version = 1
    scenario_id = 'rec2-native-lifetime-v1'
    scenario_version = 1
    task = 'MP-REC2'
    contract = 'DC-MP-REC2'
    source_commit = $source
    source_commit_before = $source
    tracked_dirty = ($tracked.Count -ne 0)
    tracked_changes = $tracked
    tracked_changes_after = $null
    gpui_revision = $gpuiPin
    executable_path = $exePath
    executable_sha256 = $binaryHash
    executable_sha256_after = $null
    driver_sha256 = $driverHash
    driver_sha256_after = $null
    campaign_status = 'IN_PROGRESS'
    acceptance = 'HOLD'
    build = @{ profile = 'caller-supplied; verify build record'; features = @('gpu-recovery-test') }
    os = @{ name = $operatingSystem.Caption; version = $operatingSystem.Version; build = $operatingSystem.BuildNumber }
    cpu = @($cpu)
    ram_bytes = [long]$operatingSystem.TotalVisibleMemorySize * 1024
    gpu = @($gpu)
    adapter_memory_before = Get-AdapterMemorySnapshot
    power_scheme = $power
    displays = @{ status = 'NOT_MEASURED'; reason = 'Attach native display/window observations to the final manifest' }
    thermal_frequency_background_controls = 'Uncontrolled; record observed contention separately'
    variant = $Variant
    recovery = [bool]$Recovery
    independent_processes = $Runs
    requested_sample_ms = 1000
    process_clock = 'Stopwatch/QPC with UTC bounds per sample'
    qpc_frequency = $clockFrequency
    true_cold = @{ status = 'NOT_MEASURED'; reason = 'Fresh process only; no OS file cache/driver reset' }
    plateau_method = 'Report per-run stress thirds, final-minus-first and least-squares bytes/second; no new ratified numeric threshold. Require repeated matched workloads and explain owner changes. Bounded refusal is not release or throughput success.'
    percentiles = @{ p95 = $null; p99 = $null; reason = 'Correlated gauge samples; no tail-latency certification' }
    endpoints = @{
        process_memory = 'Diagnostics.Process PrivateMemorySize64/WorkingSet64; process-wide, sampled, not allocator attribution'
        gpu_memory = 'PDH GPU Process Memory Dedicated Usage/Shared Usage/Total Committed summed across matching PID adapter instances; attribution-limited gauge'
        atlas_allocations = @{ status = 'NOT_MEASURED'; reason = 'No supported public per-atlas allocation/removal/completion endpoint' }
        gpu_completion = @{ status = 'NOT_MEASURED'; reason = 'Backend Ready and paint/drop calls are not GPU completion' }
        encoded_and_decode_scratch = @{ status = 'NOT_MEASURED'; reason = 'Source callback buffers and decoder scratch are transient; retained service bytes do not measure their peaks' }
        consumer_sets = 'Sampled service count is a lower bound whenever consumer_unknown_frames is nonzero'
        generation_resources = @{ status = 'NOT_MEASURED'; reason = 'Native generation and service pool/wrapper counts are not physical GPU resource counts' }
    }
}
$metadata | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $evidencePath 'manifest.json') -Encoding utf8NoBOM
$campaignFailures = [Collections.Generic.List[string]]::new()
foreach ($run in 1..$Runs) {
    $currentSource = (& git -C $root rev-parse HEAD 2>$null)
    if ($LASTEXITCODE -ne 0) { $campaignFailures.Add('Cannot re-resolve source commit before launch'); break }
    if (($currentSource -join '').Trim() -ne $source) { $campaignFailures.Add('Source commit changed before launch'); break }
    $currentTracked = @(& git -C $root status --porcelain --untracked-files=no 2>$null)
    if ($LASTEXITCODE -ne 0) { $campaignFailures.Add('Cannot recheck tracked source state before launch'); break }
    if ($currentTracked.Count -ne 0) { $campaignFailures.Add('Source became tracked-dirty before launch'); break }
    if ([string]::Join("`n", $currentTracked) -cne [string]::Join("`n", $tracked)) { $campaignFailures.Add('Tracked source state changed before launch'); break }
    try {
        $currentBinaryHash = (Get-FileHash -LiteralPath $exePath -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
    } catch {
        $campaignFailures.Add("Cannot hash executable before launch: $($_.Exception.Message)")
        break
    }
    if ($currentBinaryHash -ne $binaryHash) { $campaignFailures.Add('Executable changed before launch'); break }
    $runDir = Join-Path $evidencePath "run-$run"
    New-Item -ItemType Directory -Path $runDir -ErrorAction Stop | Out-Null
    $stdout = Join-Path $runDir 'native.stdout'
    $stderr = Join-Path $runDir 'native.stderr'
    [IO.File]::WriteAllBytes($stdout, [byte[]]@())
    [IO.File]::WriteAllBytes($stderr, [byte[]]@())
    $arguments = @('--fixture', '--lifetime')
    if ($Variant -eq 'windows2') { $arguments += '--windows2' } else { $arguments += "--$Variant" }
    if ($Recovery) { $arguments += '--lifetime-recovery' }
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $exePath
    $startInfo.WorkingDirectory = $runDir
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $arguments) { $startInfo.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    $rows = [Collections.Generic.List[object]]::new()
    $failures = [Collections.Generic.List[string]]::new()
    $timedOut = $false
    $exitCode = $null
    $startedUtc = [DateTime]::UtcNow
    $startedQpc = [Diagnostics.Stopwatch]::GetTimestamp()
    $lifetimeTicks = $null
    $processStarted = $false
    $stdoutTask = $null
    $stderrTask = $null
    $stdoutTruncated = $false
    $stderrTruncated = $false
    $previousSampleEndQpc = $startedQpc
    $processExecutablePath = $null
    $pathMatchesRequested = $null
    try {
        if (-not $process.Start()) { throw 'Native experiment did not start' }
        $processStarted = $true
        # Concurrent capped drains preserve pipe liveness while discarding excess output.
        $stdoutTask = [Rec2BoundedOutput]::DrainAsync($process.StandardOutput.BaseStream, [int]$maxCapturedOutputBytes)
        $stderrTask = [Rec2BoundedOutput]::DrainAsync($process.StandardError.BaseStream, [int]$maxCapturedOutputBytes)
        $process.Refresh()
        $lifetimeTicks = $process.StartTime.ToUniversalTime().Ticks
        $counterNames = @('Dedicated Usage', 'Shared Usage', 'Total Committed')
        $counterPaths = @($counterNames | ForEach-Object { "\GPU Process Memory(pid_$($process.Id)_*)\$_" })
        $deadlineQpc = $startedQpc + ([long]150 * $clockFrequency)
        $nextSample = $startedQpc
        while (-not $process.HasExited -and [Diagnostics.Stopwatch]::GetTimestamp() -lt $deadlineQpc -and $rows.Count -lt 151) {
            $remainingMs = ([math]::Min($nextSample, $deadlineQpc) - [Diagnostics.Stopwatch]::GetTimestamp()) * 1000 / $clockFrequency
            if ($remainingMs -gt 0) { Start-Sleep -Milliseconds ([int][math]::Ceiling($remainingMs)) }
            if ([Diagnostics.Stopwatch]::GetTimestamp() -ge $deadlineQpc) { break }
            $process.Refresh()
            if ($process.HasExited) { break }
            $before = $process.StartTime.ToUniversalTime().Ticks
            # A module read is a secondary executable check: Windows can transiently refuse or
            # return an empty name while the process is being inspected, and the reported path can
            # differ from the caller's string by junction or short-name resolution. PID plus start
            # time bind the sampled lifetime, and the file hash binds the executable, so the first
            # readable path is the reference for later samples rather than the caller's spelling.
            $observedPath = $null
            for ($attempt = 0; $attempt -lt 3 -and -not $observedPath; $attempt++) {
                try {
                    $candidatePath = [string]$process.MainModule.FileName
                    if (-not [string]::IsNullOrEmpty($candidatePath)) {
                        $observedPath = [IO.Path]::GetFullPath($candidatePath)
                    }
                } catch {
                    $observedPath = $null
                }
                if (-not $observedPath) { Start-Sleep -Milliseconds 100 }
            }
            if ($null -eq $processExecutablePath -and $observedPath) {
                $processExecutablePath = $observedPath
                $pathMatchesRequested = [StringComparer]::OrdinalIgnoreCase.Equals($observedPath, $exePath)
            }
            # One PID with a fixed start time cannot change its image, so a differently spelled
            # module path is a resolution artifact (junction, short name), not a new executable.
            # The recorded binary hash is the executable identity; the path stays informational.
            if ($before -ne $lifetimeTicks) { throw 'Process lifetime changed before sampling' }
            $observedTitle = [string]$process.MainWindowTitle
            $sampleQpcStart = [Diagnostics.Stopwatch]::GetTimestamp()
            if ($sampleQpcStart -lt $previousSampleEndQpc) { throw 'Process sample QPC intervals are not monotonic' }
            $sample = [ordered]@{
                order = $rows.Count
                utc_start = [DateTime]::UtcNow.ToString('o')
                qpc_start = $sampleQpcStart
                pid = $process.Id
                executable_path = $observedPath
                main_window_title = $observedTitle
                start_utc_ticks = $before
                identity_after_ticks = $null
                working_set_bytes = [long]$process.WorkingSet64
                private_bytes = [long]$process.PrivateMemorySize64
                lifetime_peak_working_set_bytes = [long]$process.PeakWorkingSet64
                gpu = [ordered]@{ status = 'NOT_MEASURED'; reason = 'PDH process instance unavailable or query failed'; instances = @(); dedicated_bytes = $null; shared_bytes = $null; committed_bytes = $null }
                alive_after = $null
                process_exit_censored = $null
                qpc_end = $null
                utc_end = $null
            }
            try {
                $counterSet = Get-Counter -Counter $counterPaths -MaxSamples 1 -ErrorAction Stop
                $processInstances = @($counterSet.CounterSamples | Where-Object { $_.InstanceName -like "pid_$($process.Id)_*" })
                $sample.gpu.instances = @($processInstances | ForEach-Object {
                    $path = [string]$_.Path
                    $value = [double]$_.CookedValue
                    $valueValid = $_.Status -eq 0 -and [double]::IsFinite($value) -and $value -ge 0 -and $value -lt 9223372036854775808.0
                    @{
                        instance = $_.InstanceName
                        counter = $path.Substring($path.LastIndexOf('\') + 1)
                        bytes = if ($valueValid) { [long]$value } else { $null }
                        cooked_value = if ([double]::IsFinite($value)) { $value } else { [string]$value }
                        path = $path
                        status = $_.Status
                    }
                })
                $counterSums = @{}
                $countersValid = $processInstances.Count -gt 0
                foreach ($counterName in $counterNames) {
                    $counterSamples = @($processInstances | Where-Object { $_.Path.EndsWith("\$counterName", [StringComparison]::OrdinalIgnoreCase) })
                    if ($counterSamples.Count -eq 0) {
                        $countersValid = $false
                        continue
                    }
                    $sum = 0.0
                    $counterValuesValid = $true
                    foreach ($counterSample in $counterSamples) {
                        $value = [double]$counterSample.CookedValue
                        if ($counterSample.Status -ne 0 -or -not [double]::IsFinite($value) -or $value -lt 0 -or $value -ge 9223372036854775808.0) {
                            $counterValuesValid = $false
                            break
                        }
                        $sum += $value
                        if ($sum -ge 9223372036854775808.0) {
                            $counterValuesValid = $false
                            break
                        }
                    }
                    if (-not $counterValuesValid) {
                        $countersValid = $false
                    } else {
                        $counterSums[$counterName] = [long]$sum
                    }
                }
                if ($countersValid -and $counterSums.Count -eq $counterNames.Count) {
                    $sample.gpu.status = 'OBSERVED'
                    $sample.gpu.reason = $null
                    $sample.gpu.dedicated_bytes = $counterSums['Dedicated Usage']
                    $sample.gpu.shared_bytes = $counterSums['Shared Usage']
                    $sample.gpu.committed_bytes = $counterSums['Total Committed']
                } else {
                    $sample.gpu.reason = 'PDH process instances missing or invalid'
                }
            } catch {
                # Missing GPU counters stay NOT_MEASURED; zero is only a valid sampled value.
                $sample.gpu.reason = 'PDH process instance unavailable or query failed'
            }
            $process.Refresh()
            $sample.alive_after = -not $process.HasExited
            if ($sample.alive_after) {
                try {
                    $sample.identity_after_ticks = $process.StartTime.ToUniversalTime().Ticks
                } catch {
                    $process.Refresh()
                    if (-not $process.HasExited) { throw }
                    $sample.alive_after = $false
                }
            }
            $sample.process_exit_censored = -not $sample.alive_after
            $sample.qpc_end = [Diagnostics.Stopwatch]::GetTimestamp()
            $sample.utc_end = [DateTime]::UtcNow.ToString('o')
            if ($sample.qpc_end -le $sample.qpc_start) { $failures.Add('Process sample QPC interval is not positive') }
            $previousSampleEndQpc = $sample.qpc_end
            $rows.Add($sample)
            if (-not $sample.alive_after) { break }
            if ($sample.identity_after_ticks -ne $lifetimeTicks) { throw 'Process identity changed after sampling' }
            $nextSample = [math]::Max($nextSample + $clockFrequency, [Diagnostics.Stopwatch]::GetTimestamp())
        }
        if (-not $process.HasExited) {
            $timedOut = $true
            $failures.Add('Native experiment exceeded the 150-second/151-sample bound')
            $process.Kill($true)
        }
        $process.WaitForExit()
        $exitCode = $process.ExitCode
        if ($exitCode -ne 0) { $failures.Add("Native exit code $exitCode") }
    } catch {
        $failures.Add($_.Exception.Message)
    } finally {
        if ($processStarted) {
            try {
                if (-not $process.HasExited) { $process.Kill($true) }
                $process.WaitForExit()
                if ($null -eq $exitCode) { $exitCode = $process.ExitCode }
            } catch {
                $failures.Add("Native process cleanup failed: $($_.Exception.Message)")
            }
        }
        if ($null -ne $stdoutTask) {
            try {
                $stdoutCapture = $stdoutTask.GetAwaiter().GetResult()
                $stdoutCapture.WriteTo($stdout)
                $stdoutTruncated = $stdoutCapture.Truncated
            } catch {
                $failures.Add("Native stdout collection failed: $($_.Exception.Message)")
            }
        }
        if ($null -ne $stderrTask) {
            try {
                $stderrCapture = $stderrTask.GetAwaiter().GetResult()
                $stderrCapture.WriteTo($stderr)
                $stderrTruncated = $stderrCapture.Truncated
            } catch {
                $failures.Add("Native stderr collection failed: $($_.Exception.Message)")
            }
        }
        if ($stdoutTruncated) { $failures.Add("Native stdout exceeded the $maxCapturedOutputBytes-byte capture bound") }
        if ($stderrTruncated) { $failures.Add("Native stderr exceeded the $maxCapturedOutputBytes-byte capture bound") }

        $record = [ordered]@{
            run_id = "rec2-$Variant-$run"
            pair_id = "rec2-$Variant"
            scenario_id = 'rec2-native-lifetime-v1'
            variant = $Variant
            status = if ($failures.Count) { 'FAILED' } else { 'OBSERVED' }
            reason = 'Gauge observations only; run check_cache_lifetime.py for validity, trends and endpoint HOLD disposition'
            executable_path = $exePath
            observed_executable_path = $processExecutablePath
            observed_path_matches_requested = $pathMatchesRequested
            process_started = [bool]$processStarted
            pid = if ($processStarted) { $process.Id } else { $null }
            start_utc_ticks = $lifetimeTicks
            launch_utc = $startedUtc.ToString('o')
            launch_qpc = $startedQpc
            ended_utc = [DateTime]::UtcNow.ToString('o')
            qpc_frequency = $clockFrequency
            arguments = $arguments
            timed_out = $timedOut
            exit_code = $exitCode
            failures = @($failures.ToArray())
            samples = @($rows.ToArray())
            capture_limit_bytes = $maxCapturedOutputBytes
            native_stdout_truncated = [bool]$stdoutTruncated
            native_stderr_truncated = [bool]$stderrTruncated
            native_stdout_sha256 = if (Test-Path -LiteralPath $stdout) { (Get-FileHash -LiteralPath $stdout -Algorithm SHA256).Hash.ToLowerInvariant() } else { $null }
            native_stderr_sha256 = if (Test-Path -LiteralPath $stderr) { (Get-FileHash -LiteralPath $stderr -Algorithm SHA256).Hash.ToLowerInvariant() } else { $null }
        }
        $record | ConvertTo-Json -Depth 15 | Set-Content -LiteralPath (Join-Path $runDir 'observations.json') -Encoding utf8NoBOM
        $runRecords.Add(@{ run_id = $record.run_id; status = $record.status; observations = "run-$run/observations.json"; stdout = "run-$run/native.stdout"; stderr = "run-$run/native.stderr" })
        $process.Dispose()
    }
    Write-Output ("REC2 run {0}/{1}: {2}, samples={3}" -f $run, $Runs, $record.status, $rows.Count)
    if ($failures.Count) { break }
}
$metadata['runs'] = @($runRecords.ToArray())
$executableHashAfter = $null
try {
    $executableHashAfter = (Get-FileHash -LiteralPath $exePath -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
} catch {
    $campaignFailures.Add("Cannot hash executable after campaign: $($_.Exception.Message)")
}
$metadata['executable_sha256_after'] = $executableHashAfter
$sourceAfterResult = (& git -C $root rev-parse HEAD 2>$null)
$sourceAfterOk = $LASTEXITCODE -eq 0
$metadata['source_commit_after'] = if ($sourceAfterOk) { ($sourceAfterResult -join '').Trim() } else { $null }
if (-not $sourceAfterOk) { $campaignFailures.Add('Cannot resolve source commit after campaign') }
$trackedAfter = @(& git -C $root status --porcelain --untracked-files=no 2>$null)
$trackedAfterOk = $LASTEXITCODE -eq 0
$metadata['tracked_changes_after'] = $trackedAfter
$metadata['tracked_dirty_after'] = -not $trackedAfterOk -or $trackedAfter.Count -ne 0
if (-not $trackedAfterOk) { $campaignFailures.Add('Cannot inspect tracked source state after campaign') }
if ($metadata['source_commit_after'] -ne $source) { $campaignFailures.Add('Source commit changed during the campaign') }
if ($metadata['tracked_dirty_after']) { $campaignFailures.Add('Source was tracked-dirty after the campaign') }
if ([string]::Join("`n", $trackedAfter) -cne [string]::Join("`n", $tracked)) { $campaignFailures.Add('Tracked source state changed during the campaign') }
if ($executableHashAfter -ne $binaryHash) { $campaignFailures.Add('Executable changed during the campaign') }
$driverHashAfter = $null
try {
    $driverHashAfter = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
} catch {
    $campaignFailures.Add("Cannot hash observer after campaign: $($_.Exception.Message)")
}
$metadata['driver_sha256_after'] = $driverHashAfter
if ($driverHashAfter -ne $driverHash) { $campaignFailures.Add('Observer script changed during the campaign') }
$metadata['integrity_failures'] = @($campaignFailures.ToArray())
$metadata['adapter_memory_after'] = Get-AdapterMemorySnapshot
$runFailed = @($runRecords | Where-Object status -eq 'FAILED').Count -gt 0
$metadata['campaign_status'] = if ($campaignFailures.Count -eq 0 -and $runRecords.Count -eq $Runs -and -not $runFailed) { 'OBSERVED' } else { 'FAILED' }
$metadata | ConvertTo-Json -Depth 15 | Set-Content -LiteralPath (Join-Path $evidencePath 'manifest.json') -Encoding utf8NoBOM
if ($metadata['campaign_status'] -eq 'FAILED') { exit 1 }
Write-Output 'REC2 observer completed; acceptance remains HOLD and no full residency is inferred'
