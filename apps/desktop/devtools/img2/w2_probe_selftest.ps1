#!/usr/bin/env pwsh
#Requires -Version 7.0
[CmdletBinding()]
param()

Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'

$probePath = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..\..\apps\desktop\examples\decode_compare\wic_probe.ps1')).Path
$pwshPath = Join-Path $PSHOME 'pwsh.exe'
if (-not [System.IO.File]::Exists($pwshPath)) {
    throw [System.IO.FileNotFoundException]::new('current PowerShell executable is missing from PSHOME', $pwshPath)
}
$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('w2-probe-selftest-' + [guid]::NewGuid().ToString('N'))
[void] [System.IO.Directory]::CreateDirectory($testRoot)
$script:activeChildren = [System.Collections.Generic.List[System.Diagnostics.Process]]::new()

function Assert-W2Probe {
    param([bool] $Condition, [string] $Message)
    if (-not $Condition) {
        throw [System.InvalidOperationException]::new($Message)
    }
}

function New-W2Bmp {
    param([string] $Path, [int] $Width = 1, [int] $Height = 1)
    $stride = [int] ([Math]::Ceiling(($Width * 3) / 4.0) * 4)
    $pixelBytes = $stride * $Height
    $bytes = [byte[]]::new(54 + $pixelBytes)
    $bytes[0] = 0x42
    $bytes[1] = 0x4D
    [System.Buffer]::BlockCopy([BitConverter]::GetBytes([int] $bytes.Length), 0, $bytes, 2, 4)
    [System.Buffer]::BlockCopy([BitConverter]::GetBytes([int] 54), 0, $bytes, 10, 4)
    [System.Buffer]::BlockCopy([BitConverter]::GetBytes([int] 40), 0, $bytes, 14, 4)
    [System.Buffer]::BlockCopy([BitConverter]::GetBytes($Width), 0, $bytes, 18, 4)
    [System.Buffer]::BlockCopy([BitConverter]::GetBytes($Height), 0, $bytes, 22, 4)
    [System.Buffer]::BlockCopy([BitConverter]::GetBytes([int16] 1), 0, $bytes, 26, 2)
    [System.Buffer]::BlockCopy([BitConverter]::GetBytes([int16] 24), 0, $bytes, 28, 2)
    [System.Buffer]::BlockCopy([BitConverter]::GetBytes([int] $pixelBytes), 0, $bytes, 34, 4)
    [System.Buffer]::BlockCopy([BitConverter]::GetBytes([int] 2835), 0, $bytes, 38, 4)
    [System.Buffer]::BlockCopy([BitConverter]::GetBytes([int] 2835), 0, $bytes, 42, 4)
    for ($y = 0; $y -lt $Height; $y++) {
        for ($x = 0; $x -lt $Width; $x++) {
            $offset = 54 + ($y * $stride) + ($x * 3)
            $bytes[$offset] = 0
            $bytes[$offset + 1] = 0
            $bytes[$offset + 2] = 255
        }
    }
    [System.IO.File]::WriteAllBytes($Path, $bytes)
}

function New-W2ObservationRoot {
    param([string] $Name)
    $path = Join-Path $testRoot $Name
    [void] [System.IO.Directory]::CreateDirectory($path)
    return $path
}

function New-W2Process {
    param(
        [string] $InputPath,
        [string] $OutputPath,
        [string] $ObservationRoot,
        [int] $TraceMaxRecords = 128,
        [int] $TraceMaxBytes = 131072,
        [string[]] $ExtraArguments = @()
    )
    $info = [System.Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $pwshPath
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.CreateNoWindow = $true
    $arguments = @(
        '-NoProfile', '-File', $probePath,
        '-InputPath', $InputPath,
        '-OutputPath', $OutputPath,
        '-MaxPixels', '1',
        '-MaxDimension', '1',
        '-MaxBitDepth', '8',
        '-MaxDecodedBytes', '16',
        '-MaxScratchBytes', '16',
        '-DeclaredScratchBytes', '0',
        '-ObservationRoot', $ObservationRoot,
        '-TraceMaxRecords', [string] $TraceMaxRecords,
        '-TraceMaxBytes', [string] $TraceMaxBytes
    ) + $ExtraArguments
    foreach ($argument in $arguments) {
        [void] $info.ArgumentList.Add([string] $argument)
    }
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $info
    if (-not $process.Start()) {
        throw [System.InvalidOperationException]::new('could not start the direct WIC probe child')
    }
    $script:activeChildren.Add($process)
    return [pscustomobject]@{
        Process = $process
        Stdout = $process.StandardOutput.ReadToEndAsync()
        Stderr = $process.StandardError.ReadToEndAsync()
    }
}

function Wait-W2Process {
    param([pscustomobject] $Run, [int] $TimeoutMs = 30000)
    if (-not $Run.Process.WaitForExit($TimeoutMs)) {
        try { $Run.Process.Kill() } catch { }
        [void] $Run.Process.WaitForExit(5000)
        throw [System.TimeoutException]::new('WIC probe child exceeded the selftest deadline')
    }
    $Run.Process.WaitForExit()
    $response = [pscustomobject]@{
        ExitCode = $Run.Process.ExitCode
        Stdout = $Run.Stdout.Result
        Stderr = $Run.Stderr.Result
    }
    [void] $script:activeChildren.Remove($Run.Process)
    return $response
}

function Read-W2Trace {
    param([string] $ObservationRoot)
    $tracePath = Join-Path $ObservationRoot 'trace.jsonl'
    if (-not [System.IO.File]::Exists($tracePath)) {
        return @()
    }
    $stream = [System.IO.FileStream]::new($tracePath, [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    try {
        if ($stream.Length -gt 131072) {
            throw [System.InvalidOperationException]::new('trace exceeded the selftest byte cap')
        }
        $buffer = [byte[]]::new([int] $stream.Length + 1)
        $count = $stream.Read($buffer, 0, $buffer.Length)
        $text = [System.Text.Encoding]::UTF8.GetString($buffer, 0, $count)
        $lastLine = $text.LastIndexOf("`n")
        if ($lastLine -lt 0) { return @() }
        $prefix = $text.Substring(0, $lastLine)
        return @($prefix.Split("`n") | ForEach-Object { ConvertFrom-Json -InputObject $_ -AsHashtable })
    }
    finally { $stream.Dispose() }
}

function Wait-W2Stage {
    param([pscustomobject] $Run, [string] $ObservationRoot, [string] $Stage, [int] $TimeoutMs = 15000)
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt $TimeoutMs) {
        $rows = @(Read-W2Trace -ObservationRoot $ObservationRoot)
        if (@($rows | Where-Object { $_.stage -ceq $Stage }).Count -gt 0) {
            return $rows
        }
        if ($Run.Process.HasExited) {
            throw [System.InvalidOperationException]::new("probe exited before held boundary '$Stage'")
        }
        Start-Sleep -Milliseconds 20
    }
    throw [System.TimeoutException]::new("probe did not reach held boundary '$Stage'")
}

function Assert-W2SuccessfulTrace {
    param(
        [string] $ObservationRoot,
        [string] $InputFile,
        [string] $OutputPath,
        [pscustomobject] $Response
    )
    Assert-W2Probe ($Response.ExitCode -eq 0) 'traced WIC decode did not return exit 0'
    $result = $Response.Stdout | ConvertFrom-Json -AsHashtable
    Assert-W2Probe ($result.outcome -ceq 'decoded') 'traced stdout did not report decoded'
    Assert-W2Probe ([System.IO.File]::Exists($OutputPath)) 'successful traced decode did not publish output'
    $rows = @(Read-W2Trace -ObservationRoot $ObservationRoot)
    $expected = @(
        'process_observed', 'input_opened', 'metadata_available', 'before_admission',
        'admission_started', 'admitted', 'before_onload', 'onload_entered',
        'onload_returned', 'after_onload', 'conversion_entered', 'conversion_returned',
        'copy_pixels_entered', 'copy_pixels_returned', 'encode_entered', 'encode_returned',
        'before_publish', 'output_published', 'probe_exit'
    )
    $actual = ($rows | ForEach-Object { $_.stage }) -join ','
    Assert-W2Probe ($actual -ceq ($expected -join ',')) 'trace stages did not match the admitted output path order'
    Assert-W2Probe ($rows[-1].complete -eq $true -and $rows[-1].outcome -ceq 'decoded') 'trace terminal row was not complete decoded success'
    Assert-W2Probe ($rows[5].width -eq 1 -and $rows[5].height -eq 1) 'admitted stage omitted image dimensions'
    Assert-W2Probe ($rows[5].input_bytes -gt 0 -and $rows[5].bgra32_bytes_required -eq 4) 'admitted stage accounting fields were absent or incorrect'
    Assert-W2Probe ($rows[5].bits_per_channel -gt 0 -and $rows[5].bits_per_pixel -gt 0 -and
        $rows[5].row_bytes -gt 0 -and $rows[5].decoded_bytes_required -gt 0 -and
        $rows[5].declared_scratch_bytes -eq 0 -and $rows[5].scratch_bytes_required -eq 4) 'admitted stage omitted declared or derived byte accounting'
    $traceText = [System.IO.File]::ReadAllText((Join-Path $ObservationRoot 'trace.jsonl'))
    Assert-W2Probe (-not $traceText.Contains($InputFile) -and -not $traceText.Contains($OutputPath)) 'trace exposed a fixture or output path'
    Assert-W2Probe (@($rows | ForEach-Object { $_.pid } | Select-Object -Unique).Count -eq 1) 'trace process PID changed during one lifetime'
    Assert-W2Probe (@($rows | ForEach-Object { $_.process_start_utc_ticks } | Select-Object -Unique).Count -eq 1) 'trace process lifetime identity changed during one lifetime'
    Assert-W2Probe (@($rows | ForEach-Object { $_.qpc_frequency } | Select-Object -Unique).Count -eq 1) 'trace QPC frequency changed within one process'
    Assert-W2Probe (@($rows | Where-Object { $_.clock_domain -cne 'windows_qpc' -or $_.observer -cne 'wic_probe' }).Count -eq 0) 'trace omitted the QPC clock domain or observer identity'
    Assert-W2Probe ($rows[0].pid -gt 0 -and $rows[0].process_start_utc_ticks -gt 0 -and $rows[0].qpc_frequency -gt 0) 'trace process/QPC lifetime identity was invalid'
    for ($i = 1; $i -lt $rows.Count; $i++) {
        Assert-W2Probe ($rows[$i].qpc_ticks -ge $rows[$i - 1].qpc_ticks) 'QPC timestamps were not monotonic'
    }
    $traceBytes = [System.IO.FileInfo]::new((Join-Path $ObservationRoot 'trace.jsonl')).Length
    Assert-W2Probe ($rows.Count -le 128 -and $traceBytes -le 131072) 'trace exceeded the selftest manifest caps'
    Assert-W2Probe (-not $Response.Stdout.Contains($InputFile)) 'stdout unexpectedly echoed a fixture path'
}

try {
    $inputPath = Join-Path $testRoot 'one-pixel.bmp'
    New-W2Bmp -Path $inputPath

    # Legacy non-opt-in path must keep ordinary decoded JSON and direct output behavior.
    $legacyOutput = Join-Path $testRoot 'legacy.png'
    $legacyInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $legacyInfo.FileName = $pwshPath
    $legacyInfo.UseShellExecute = $false
    $legacyInfo.RedirectStandardOutput = $true
    $legacyInfo.RedirectStandardError = $true
    foreach ($argument in @('-NoProfile', '-File', $probePath, '-InputPath', $inputPath, '-OutputPath', $legacyOutput)) {
        [void] $legacyInfo.ArgumentList.Add($argument)
    }
    $legacyProcess = [System.Diagnostics.Process]::new()
    $legacyProcess.StartInfo = $legacyInfo
    [void] $legacyProcess.Start()
    $script:activeChildren.Add($legacyProcess)
    $legacyRun = [pscustomobject]@{
        Process = $legacyProcess
        Stdout = $legacyProcess.StandardOutput.ReadToEndAsync()
        Stderr = $legacyProcess.StandardError.ReadToEndAsync()
    }
    $legacyResponse = Wait-W2Process $legacyRun
    Assert-W2Probe ($legacyResponse.ExitCode -eq 0 -and [System.IO.File]::Exists($legacyOutput)) 'legacy WIC output path regressed'
    $legacyResult = $legacyResponse.Stdout | ConvertFrom-Json -AsHashtable
    Assert-W2Probe ($legacyResult.outcome -ceq 'decoded' -and -not $legacyResult.Contains('trace')) 'legacy stdout fields changed or gained trace data'
    $legacyProcess.Dispose()

    $successRoot = New-W2ObservationRoot 'trace-success'
    $successOutput = Join-Path $testRoot 'traced.png'
    $successRun = New-W2Process -InputPath $inputPath -OutputPath $successOutput -ObservationRoot $successRoot
    $successResponse = Wait-W2Process $successRun
    Assert-W2SuccessfulTrace -ObservationRoot $successRoot -InputFile $inputPath -OutputPath $successOutput -Response $successResponse
    $successRun.Process.Dispose()

    # Independent WPF pixel read guards the real BMP -> PNG materialization path.
    Add-Type -AssemblyName PresentationCore
    $outputStream = [System.IO.File]::OpenRead($successOutput)
    try {
        $outputDecoder = [System.Windows.Media.Imaging.BitmapDecoder]::Create(
            $outputStream,
            [System.Windows.Media.Imaging.BitmapCreateOptions]::None,
            [System.Windows.Media.Imaging.BitmapCacheOption]::OnLoad)
        $outputFrame = [System.Windows.Media.Imaging.FormatConvertedBitmap]::new(
            $outputDecoder.Frames[0], [System.Windows.Media.PixelFormats]::Bgra32, $null, 0.0)
        $pixel = [byte[]]::new(4)
        $outputFrame.CopyPixels([System.Windows.Int32Rect]::new(0, 0, 1, 1), $pixel, 4, 0)
        Assert-W2Probe ($pixel[0] -eq 0 -and $pixel[1] -eq 0 -and $pixel[2] -eq 255 -and $pixel[3] -eq 255) 'traced PNG output pixel differed from the BMP fixture'
    }
    finally { $outputStream.Dispose() }

    # Every permitted hold is flushed at its exact boundary and resumes only on continue.
    foreach ($stage in @('before_admission', 'before_onload', 'after_onload', 'before_publish')) {
        $runName = 'hold-' + $stage
        $observationRoot = New-W2ObservationRoot $runName
        $output = Join-Path $testRoot "$runName.png"
        $extra = @('-PauseAtStage', $stage, '-PauseTimeoutMs', '10000')
        $run = New-W2Process -InputPath $inputPath -OutputPath $output `
            -ObservationRoot $observationRoot -ExtraArguments $extra
        $heldRows = Wait-W2Stage -Run $run -ObservationRoot $observationRoot -Stage $stage
        $held = @($heldRows | Where-Object { $_.stage -ceq $stage })[0]
        Assert-W2Probe ($held.artificial_instrumentation_hold -eq $true -and $held.pause_timeout_ms -eq 10000) "hold '$stage' was not labelled artificial and bounded"
        Assert-W2Probe (-not $run.Process.HasExited) "probe did not remain held at '$stage'"
        Assert-W2Probe (-not [System.IO.File]::Exists($output)) "output became visible while held at '$stage'"
        if ($stage -ceq 'before_publish') {
            Assert-W2Probe ([System.IO.File]::Exists((Join-Path $observationRoot 'pending.png'))) 'before_publish did not expose only staged pending output'
        }
        $continuePath = Join-Path $observationRoot 'continue'
        $continueStream = [System.IO.FileStream]::new(
            $continuePath, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
        $continueStream.Dispose()
        $response = Wait-W2Process -Run $run -TimeoutMs 20000
        Assert-W2SuccessfulTrace -ObservationRoot $observationRoot -InputFile $inputPath -OutputPath $output -Response $response
        Assert-W2Probe (@($heldRows | Where-Object { $_.stage -ceq $stage }).Count -eq 1) "held stage '$stage' was not recorded exactly once"
        $run.Process.Dispose()
    }

    # A bounded operator timeout completes as an error, not a success or publication.
    $timeoutRoot = New-W2ObservationRoot 'pause-timeout'
    $timeoutOutput = Join-Path $testRoot 'pause-timeout.png'
    $timeoutRun = New-W2Process -InputPath $inputPath -OutputPath $timeoutOutput `
        -ObservationRoot $timeoutRoot -ExtraArguments @('-PauseAtStage', 'before_admission', '-PauseTimeoutMs', '100')
    $timeoutResponse = Wait-W2Process $timeoutRun
    $timeoutResult = $timeoutResponse.Stdout | ConvertFrom-Json -AsHashtable
    $timeoutRows = @(Read-W2Trace $timeoutRoot)
    $timeoutBoundary = @($timeoutRows | Where-Object { $_.stage -ceq 'before_admission' })[0]
    Assert-W2Probe ($timeoutBoundary.artificial_instrumentation_hold -eq $true -and $timeoutBoundary.pause_timeout_ms -eq 100) 'pause timeout boundary did not record the bounded artificial hold'
    Assert-W2Probe ($timeoutResponse.ExitCode -ne 0 -and $timeoutResult.reason_code -ceq 'operator_pause_timeout') 'pause timeout did not fail closed'
    Assert-W2Probe (-not [System.IO.File]::Exists($timeoutOutput)) 'pause timeout published final output'
    Assert-W2Probe ($timeoutRows[-1].stage -ceq 'probe_exit' -and $timeoutRows[-1].complete -eq $true) 'bounded pause timeout did not produce a complete terminal trace'
    Assert-W2Probe ($null -eq $timeoutResult.output_bytes) 'pre-admission timeout reported staged output bytes'
    Assert-W2Probe (-not ($timeoutRows.stage -contains 'refused')) 'operator timeout was misreported as an admission refusal'
    $timeoutRun.Process.Dispose()

    # A post-admission timeout is an execution failure, not a second refusal.
    $publishTimeoutRoot = New-W2ObservationRoot 'publish-pause-timeout'
    $publishTimeoutOutput = Join-Path $testRoot 'publish-pause-timeout.png'
    $publishTimeoutRun = New-W2Process -InputPath $inputPath -OutputPath $publishTimeoutOutput `
        -ObservationRoot $publishTimeoutRoot -ExtraArguments @('-PauseAtStage', 'before_publish', '-PauseTimeoutMs', '100')
    $publishTimeoutResponse = Wait-W2Process $publishTimeoutRun
    $publishTimeoutResult = $publishTimeoutResponse.Stdout | ConvertFrom-Json -AsHashtable
    $publishTimeoutRows = @(Read-W2Trace $publishTimeoutRoot)
    Assert-W2Probe ($publishTimeoutResponse.ExitCode -ne 0 -and $publishTimeoutResult.reason_code -ceq 'operator_pause_timeout') 'post-admission timeout did not fail closed'
    Assert-W2Probe ($null -eq $publishTimeoutResult.output_bytes) 'post-admission timeout reported unpublished output bytes'
    Assert-W2Probe (-not [System.IO.File]::Exists($publishTimeoutOutput) -and -not [System.IO.File]::Exists((Join-Path $publishTimeoutRoot 'pending.png'))) 'post-admission timeout retained staged or published output'
    Assert-W2Probe (-not ($publishTimeoutRows.stage -contains 'refused') -and $publishTimeoutRows[-1].stage -ceq 'probe_exit') 'post-admission timeout emitted a false refusal or omitted terminal trace'
    $publishTimeoutRun.Process.Dispose()

    # Pre-admission refusal emits terminal refusal and never materializes/publishes.
    $oversizedInput = Join-Path $testRoot 'two-pixel.bmp'
    New-W2Bmp -Path $oversizedInput -Width 2
    $refusalRoot = New-W2ObservationRoot 'admission-refusal'
    $refusalOutput = Join-Path $testRoot 'refused.png'
    $refusalResponse = Wait-W2Process (New-W2Process -InputPath $oversizedInput -OutputPath $refusalOutput -ObservationRoot $refusalRoot)
    $refusalResult = $refusalResponse.Stdout | ConvertFrom-Json -AsHashtable
    $refusalRows = @(Read-W2Trace $refusalRoot)
    Assert-W2Probe ($refusalResponse.ExitCode -eq 4 -and $refusalResult.reason_code -ceq 'max_dimension_exceeded') 'oversized admission case did not refuse at the declared bound'
    Assert-W2Probe ($refusalRows[-2].stage -ceq 'refused' -and $refusalRows[-1].stage -ceq 'probe_exit') 'refusal trace lacked terminal stages'
    Assert-W2Probe (-not [System.IO.File]::Exists($refusalOutput) -and -not ($refusalRows.stage -contains 'onload_entered')) 'refusal reached OnLoad or published output'

    # Trace cap failure leaves a readable incomplete prefix, returns error, and never publishes.
    $capRoot = New-W2ObservationRoot 'trace-cap'
    $capOutput = Join-Path $testRoot 'trace-cap.png'
    $capResponse = Wait-W2Process (New-W2Process -InputPath $inputPath -OutputPath $capOutput `
        -ObservationRoot $capRoot -TraceMaxRecords 1)
    $capResult = $capResponse.Stdout | ConvertFrom-Json -AsHashtable
    $capRows = @(Read-W2Trace $capRoot)
    Assert-W2Probe ($capResponse.ExitCode -eq 1 -and $capResult.reason_code -ceq 'trace_failure') 'trace cap failure retained success'
    Assert-W2Probe ($capRows.Count -eq 1 -and $capRows[0].stage -ceq 'process_observed') 'trace cap failure did not retain a readable bounded prefix'
    Assert-W2Probe (-not [System.IO.File]::Exists($capOutput)) 'trace cap failure published final output'

    # An existing owned-root artifact is rejected without being overwritten or deleted.
    $occupiedRoot = New-W2ObservationRoot 'occupied-root'
    $sentinel = Join-Path $occupiedRoot 'owner-sentinel.txt'
    [System.IO.File]::WriteAllText($sentinel, 'caller-owned')
    $occupiedOutput = Join-Path $testRoot 'occupied.png'
    $occupiedResponse = Wait-W2Process (New-W2Process -InputPath $inputPath -OutputPath $occupiedOutput -ObservationRoot $occupiedRoot)
    $occupiedResult = $occupiedResponse.Stdout | ConvertFrom-Json -AsHashtable
    Assert-W2Probe ($occupiedResponse.ExitCode -eq 1 -and $occupiedResult.reason_code -ceq 'trace_setup_failed') 'probe accepted a non-empty observation root'
    Assert-W2Probe ([System.IO.File]::ReadAllText($sentinel) -ceq 'caller-owned' -and -not [System.IO.File]::Exists($occupiedOutput)) 'probe mutated an existing root artifact or published output'

    # Early failures still terminate a trace without inventing metadata stages.
    $missingRoot = New-W2ObservationRoot 'missing-input'
    $missingOutput = Join-Path $testRoot 'missing-input.png'
    $missingInput = Join-Path $testRoot 'absent.bmp'
    $missingResponse = Wait-W2Process (New-W2Process -InputPath $missingInput -OutputPath $missingOutput -ObservationRoot $missingRoot)
    $missingRows = @(Read-W2Trace $missingRoot)
    Assert-W2Probe ($missingResponse.ExitCode -eq 1) 'missing-input case unexpectedly succeeded'
    Assert-W2Probe (($missingRows.stage -join ',') -ceq 'process_observed,refused,probe_exit') 'pre-open refusal invented a later trace stage'
    Assert-W2Probe (-not [System.IO.File]::Exists($missingOutput) -and $missingRows[-1].complete -eq $true) 'missing-input refusal published output or omitted completion'

    # Existing final output is never truncated or replaced by an instrumented run.
    $outputCollisionRoot = New-W2ObservationRoot 'existing-output'
    $outputCollision = Join-Path $testRoot 'caller-owned.png'
    [System.IO.File]::WriteAllText($outputCollision, 'caller-owned-output')
    $collisionResponse = Wait-W2Process (New-W2Process -InputPath $inputPath -OutputPath $outputCollision -ObservationRoot $outputCollisionRoot)
    $collisionResult = $collisionResponse.Stdout | ConvertFrom-Json -AsHashtable
    Assert-W2Probe ($collisionResponse.ExitCode -eq 1 -and $collisionResult.reason_code -ceq 'trace_setup_failed') 'probe accepted an existing output path'
    Assert-W2Probe ([System.IO.File]::ReadAllText($outputCollision) -ceq 'caller-owned-output') 'instrumented probe overwrote caller-owned output'

    # Kill at the pre-publication hold models an external direct-child cancellation.
    $killRoot = New-W2ObservationRoot 'kill-before-publish'
    $killOutput = Join-Path $testRoot 'kill-before-publish.png'
    $killRun = New-W2Process -InputPath $inputPath -OutputPath $killOutput `
        -ObservationRoot $killRoot -ExtraArguments @('-PauseAtStage', 'before_publish', '-PauseTimeoutMs', '10000')
    [void] (Wait-W2Stage -Run $killRun -ObservationRoot $killRoot -Stage 'before_publish')
    Assert-W2Probe ([System.IO.File]::Exists((Join-Path $killRoot 'pending.png'))) 'kill test did not reach staged output'
    Assert-W2Probe (-not [System.IO.File]::Exists($killOutput)) 'final output existed before cancellation at before_publish'
    $killRun.Process.Kill()
    if (-not $killRun.Process.WaitForExit(10000)) {
        throw [System.TimeoutException]::new('direct probe child did not terminate after kill')
    }
    [void] $script:activeChildren.Remove($killRun.Process)
    $partialRows = @(Read-W2Trace $killRoot)
    Assert-W2Probe ($partialRows[-1].stage -ceq 'before_publish' -and -not ($partialRows.stage -contains 'probe_exit')) 'killed trace was not a readable incomplete prefix'
    Assert-W2Probe (-not [System.IO.File]::Exists($killOutput)) 'killed pre-publication worker exposed final output'
    $killRun.Process.Dispose()

    Write-Output 'w2_probe_selftest: PASS'
}
finally {
    foreach ($child in $script:activeChildren.ToArray()) {
        try {
            if (-not $child.HasExited) {
                $child.Kill()
                [void] $child.WaitForExit(10000)
            }
        }
        catch { }
        try { $child.Dispose() } catch { }
    }
    $script:activeChildren.Clear()
    if ([System.IO.Directory]::Exists($testRoot)) {
        [System.IO.Directory]::Delete($testRoot, $true)
    }
}
