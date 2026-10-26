#!/usr/bin/env pwsh
#Requires -Version 7.0
<#
.SYNOPSIS
    WIC decode probe: the operator-run Windows decode candidate for the
    exploratory MP-IMG2 Phase 1 comparison harness.

.DESCRIPTION
    Managed WPF imaging (Windows Imaging Component) only. This probe does **not**
    use GDI+ / System.Drawing, and it performs no C# compilation step, no native
    FFmpeg call, no shell, no network and no GPU or presentation object.

    Transport (harness record schema v3; argv is always passed separately, never
    through a command string):

        -Version                            identity probe: one line on stdout, exit 0
        -InputPath <abs> [-OutputPath <abs>] [-MaxPixels N]
                                            measurement: one compact JSON line on
                                            stdout and the matching protocol exit code

    A harness run reaches this script as
        pwsh -NoProfile -File wic_probe.ps1 -InputPath <in> -OutputPath <out> [-MaxPixels N]
    and its identity probe as
        pwsh -NoProfile -File wic_probe.ps1 -Version
    The harness always asks for a `<stem>.<candidate>.wic.png` output, so the encoder
    format never depends on the input suffix.

    Exit codes mirror the harness outcome vocabulary:
        0 decoded       WIC admitted the container and materialized pixels
        2 unsupported   no WIC codec component / unknown image format
        3 corrupt       WIC rejected the header or stream data
        4 oversize      dimensions exceed the operator-declared -MaxPixels bound
        1 error         anything else, including a missing input file

.NOTES
    Observed environment of these facts (measured on the development machine):
    Windows 11, PowerShell 7.6.7, .NET 10.0.13, WIC via WPF PresentationCore.
    The identity line reports the actual runtime versions it sees.

    What counts as a decode here (N1/N3). A header read alone is not evidence:
      1. BitmapDecoder.Create with BitmapCacheOption::OnLoad materializes the whole
         frame, and
      2. a 1x1 CopyPixels through FormatConvertedBitmap(Bgra32) forces real pixel
         access, because a lazy or header-only frame cannot satisfy it.
    With -OutputPath the materialized frame is additionally re-encoded, which is
    what the harness measures as output size.

    Observed HRESULT mapping (measured, mapped by constant and never by message
    text):
        0x88982F50 WINCODEC_ERR_COMPONENTNOTFOUND -> unsupported / win_component_not_found
        0x88982F07 WINCODEC_ERR_UNKNOWNIMAGEFORMAT -> unsupported / win_unknown_image_format
        0x88982F60 WINCODEC_ERR_BADIMAGE           -> corrupt     / win_bad_image
        0x88982F61 WINCODEC_ERR_BADHEADER          -> corrupt     / win_bad_header
        0x88982F72 WINCODEC_ERR_BADSTREAMDATA      -> corrupt     / win_bad_stream_data
    A truncated container therefore surfaces as `unsupported`, because that is what
    WIC reports (UNKNOWNIMAGEFORMAT); the probe does not reinterpret it as
    `corrupt`. `oversize` is emitted only when the operator declares -MaxPixels, and
    `cancelled`/`not_measured` are never emitted as outcomes. Opt-in trace mode records
    exact WIC boundaries and may hold only at explicit operator boundaries; such a
    hold is artificial instrumentation, not cooperative in-decoder cancellation.

    `reason_code` is a closed machine vocabulary the harness whitelists; `reason`
    is human text the harness never reads.

    Path rules (N2): only drive-rooted local paths such as C:\dir\file.png are
    accepted. `C:foo` (drive-relative), `\foo` (root-relative), `\\server\share`
    (UNC), `\\?\C:\...` and `\\.\...` (device) forms are refused.

.PARAMETER InputPath
    Absolute local disk path of one encoded image file. Not echoed in the JSON line.

.PARAMETER OutputPath
    Optional absolute local disk path for the re-encoded decoded frame. Supported
    extensions are .png and .bmp; any other extension is refused as `unsupported`.

.PARAMETER Version
    Print the identity line and exit 0 (the harness identity probe).

.PARAMETER MaxPixels
    Optional operator-declared pixel bound, checked before any pixel decode;
    0 (the default) declares no bound.

.PARAMETER MaxDimension
    Optional decode-budget maximum edge. Supplying any of the four budget limits
    switches on budget admission: all four must then be positive and `-MaxPixels`
    must be positive too. The harness resolves these numbers from the ratified
    `DecodeBudget`; this script holds no budget constant of its own.

.PARAMETER MaxBitDepth
    Decode-budget maximum bits per channel.

.PARAMETER MaxDecodedBytes
    Decode-budget ceiling for the declared source raster
    (`ceil(width * height * bits-per-pixel / 8)`).

.PARAMETER MaxScratchBytes
    Decode-budget ceiling for the required scratch/conversion bytes. This probe
    charges the BGRA32 conversion destination and the explicitly declared scratch
    allowance to it; that mapping is a W1 interpretation, not a measured decoder
    peak.

.PARAMETER DeclaredScratchBytes
    Explicit scratch allowance the adapter declares in addition to the BGRA32
    conversion destination. It is a declared request, never a measurement.

.PARAMETER ObservationRoot
    Opt-in trace directory. The caller must create a fresh empty absolute local
    directory; trace.jsonl is created exclusively inside it.

.PARAMETER TraceMaxRecords
    Required positive Int32 cap on emitted JSONL records when tracing.

.PARAMETER TraceMaxBytes
    Required positive Int32 cap on UTF-8 trace bytes when tracing.

.PARAMETER PauseAtStage
    Optional boundary hold: before_admission, before_onload, after_onload or
    before_publish. Resume by creating a `continue` file in ObservationRoot.

.PARAMETER PauseTimeoutMs
    Positive finite timeout required with PauseAtStage; the hold is artificial
    instrumentation, not a decoder or cooperative-cancellation duration.

.NOTES
    With budget admission enabled, the script refuses before
    `BitmapDecoder.Create(..., CacheOption::OnLoad)` when the declared width,
    height, format/depth, decoded bytes or scratch requirement exceeds a limit,
    and reports `admission_status`/`materialization` so the refusal point is
    visible. WIC's internal temporaries remain unmeasured: the probe reports
    `wic_internal_scratch = not_measured` and never substitutes zero or a guess.
    A container parse performed by `DelayCreation`/`CacheOption::None` may still
    allocate; no claim of zero allocation is made.

    Trace mode requires explicit positive admission limits and TraceMaxRecords /
    TraceMaxBytes. It writes bounded UTF-8 JSONL to a fresh exclusive trace.jsonl;
    each row is flushed and carries process lifetime UTC ticks plus monotonic QPC
    ticks/frequency (`windows_qpc`, observer `wic_probe`). A killed worker leaves an
    incomplete readable prefix. Optional named holds wait for an operator-created
    `continue` file only at the listed boundaries and are not decoder latency.
    Instrumented output is encoded to fresh pending.png/pending.bmp and moved to an
    absent OutputPath only after before_publish completes.

    Budget refusal codes are `max_dimension_exceeded`, `max_pixels_exceeded`,
    `max_bit_depth_exceeded`, `max_decoded_bytes_exceeded` and
    `max_scratch_bytes_exceeded` (exit 4), `unsupported_pixel_format` (exit 2),
    `invalid_metadata` (exit 3) and `invalid_limits` (exit 1).

.EXAMPLE
    pwsh -NoProfile -File wic_probe.ps1 -InputPath C:\tmp\img2-compare\cases\ok.png -OutputPath C:\tmp\img2-compare\wic-out.png
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string] $InputPath = '',

    [string] $OutputPath = '',

    [switch] $Version,

    [long] $MaxPixels = 0,

    [long] $MaxDimension = 0,

    [long] $MaxBitDepth = 0,

    [long] $MaxDecodedBytes = 0,

    [long] $MaxScratchBytes = 0,

    [long] $DeclaredScratchBytes = 0,

    [string] $ObservationRoot = '',

    [int] $TraceMaxRecords = 0,

    [int] $TraceMaxBytes = 0,

    [string] $PauseAtStage = '',

    [int] $PauseTimeoutMs = 0
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'

$exitDecoded = 0
$exitError = 1
$exitUnsupported = 2
$exitCorrupt = 3
$exitOversize = 4

$script:traceRequested = $false
$script:traceActive = $false
$script:traceFailed = $false
$script:traceStream = $null
$script:tracePath = $null
$script:traceRecordCount = 0
$script:traceByteCount = [long] 0
$script:traceStages = $null
$script:traceProcessStartTicks = [long] 0
$script:pendingPath = $null
$script:publishedOutputPath = $null
$script:publishedOutputByProbe = $false
$script:traceEncoding = $null

function Remove-InstrumentedFiles {
    $removed = $true
    if ($null -ne $script:pendingPath -and [System.IO.File]::Exists($script:pendingPath)) {
        try { [System.IO.File]::Delete($script:pendingPath) } catch { $removed = $false }
    }
    if ($script:publishedOutputByProbe -and
        $null -ne $script:publishedOutputPath -and
        [System.IO.File]::Exists($script:publishedOutputPath)) {
        try { [System.IO.File]::Delete($script:publishedOutputPath) } catch { $removed = $false }
    }
    return $removed
}

function Write-ProbeResult {
    param(
        [Parameter(Mandatory = $true)]
        [System.Collections.Specialized.OrderedDictionary] $Result,

        [Parameter(Mandatory = $true)]
        [int] $ExitCode
    )
    if ($script:traceActive -and $Result.outcome -ne 'decoded') {
        $Result.output_bytes = $null
    }
    if ($script:traceActive -and -not $script:traceFailed) {
        try {
            $preAdmissionTerminal = -not $script:traceStages.ContainsKey('admitted') -and
                $Result.reason_code -ne 'operator_pause_timeout'
            if ($Result.outcome -ne 'decoded' -and
                -not $script:traceStages.ContainsKey('refused') -and
                $preAdmissionTerminal) {
                Write-ProbeTraceStage -Stage 'refused'
            }
            Write-ProbeTraceStage -Stage 'probe_exit' -Fields ([ordered]@{
                outcome = [string] $Result.outcome
                reason_code = $Result.reason_code
                admission_status = [string] $Result.admission_status
                materialization = [string] $Result.materialization
                complete = $true
            })
            $script:traceStream.Flush($true)
            $script:traceStream.Dispose()
            $script:traceStream = $null
        }
        catch {
            $script:traceFailed = $true
        }
    }
    $cleanupSucceeded = $true
    if ($script:traceFailed) {
        $Result.outcome = 'error'
        $Result.reason_code = 'trace_failure'
        $Result.reason = 'bounded stage trace could not be completed'
        $Result.output_bytes = $null
        $ExitCode = $exitError
        $cleanupSucceeded = Remove-InstrumentedFiles
    }
    elseif ($Result.outcome -ne 'decoded') {
        $cleanupSucceeded = Remove-InstrumentedFiles
    }
    if (-not $cleanupSucceeded) {
        $Result.outcome = 'error'
        $Result.reason_code = 'output_cleanup_failed'
        $Result.reason = 'instrumented output cleanup could not be confirmed'
        $Result.output_bytes = $null
        $ExitCode = $exitError
    }
    if ($null -ne $script:traceStream) {
        try { $script:traceStream.Dispose() } catch { }
        $script:traceStream = $null
    }
    [Console]::Out.WriteLine(($Result | ConvertTo-Json -Compress -Depth 2))
    [Console]::Out.Flush()
    exit $ExitCode
}

function Test-NoReparseComponents {
    param([Parameter(Mandatory = $true)][string] $Path)

    $full = [System.IO.Path]::GetFullPath($Path)
    $root = [System.IO.Path]::GetPathRoot($full)
    $current = $root
    $relative = $full.Substring($root.Length)
    foreach ($part in ($relative -split '[\\/]' | Where-Object { $_.Length -gt 0 })) {
        $current = [System.IO.Path]::Combine($current, $part)
        $item = Get-Item -LiteralPath $current -Force -ErrorAction SilentlyContinue
        if ($null -ne $item) {
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw [System.IO.IOException]::new('reparse points are not permitted in instrumented paths')
            }
        }
    }
}

function Initialize-ProbeTrace {
    if (-not $script:traceOptionsDeclared) {
        return
    }
    if (-not $script:traceRequested -or -not (Test-LocalDiskPath $ObservationRoot)) {
        throw [System.ArgumentException]::new('observation root must be an absolute local path')
    }
    if (-not $script:traceCapsDeclared -or $TraceMaxRecords -le 0 -or $TraceMaxBytes -le 0) {
        throw [System.ArgumentException]::new('trace maximum records and bytes must be supplied as positive values')
    }
    if ($script:pauseOptionsDeclared -and
        (-not $script:pauseOptionsComplete -or [string]::IsNullOrWhiteSpace($PauseAtStage) -or $PauseTimeoutMs -le 0)) {
        throw [System.ArgumentException]::new('pause stage and positive pause timeout must be supplied together')
    }
    $allowedPauses = @('before_admission', 'before_onload', 'after_onload', 'before_publish')
    if ($PauseAtStage.Length -gt 0 -and $PauseAtStage -cnotin $allowedPauses) {
        throw [System.ArgumentException]::new('pause stage is not an explicit instrumented boundary')
    }
    if (-not $budgetDeclared -or $MaxPixels -le 0 -or $MaxDimension -le 0 -or
        $MaxBitDepth -le 0 -or $MaxDecodedBytes -le 0 -or $MaxScratchBytes -le 0) {
        throw [System.ArgumentException]::new('instrumented WIC requires all positive decode admission limits')
    }
    if (-not [System.IO.Directory]::Exists($ObservationRoot)) {
        throw [System.IO.DirectoryNotFoundException]::new('observation root must be a pre-created owned directory')
    }
    Test-NoReparseComponents -Path $ObservationRoot
    $rootPath = [System.IO.Path]::GetFullPath($ObservationRoot)
    if ([System.IO.Directory]::GetFileSystemEntries($rootPath).Length -ne 0) {
        throw [System.IO.IOException]::new('observation root must be empty')
    }
    if (-not [string]::IsNullOrWhiteSpace($OutputPath)) {
        if (-not (Test-LocalDiskPath $OutputPath)) {
            throw [System.ArgumentException]::new('instrumented output must be an absolute local path')
        }
        $output = [System.IO.Path]::GetFullPath($OutputPath)
        $inputFullPath = if ([string]::IsNullOrWhiteSpace($InputPath)) { '' } else { [System.IO.Path]::GetFullPath($InputPath) }
        if ($inputFullPath.Length -gt 0 -and [System.StringComparer]::OrdinalIgnoreCase.Equals($inputFullPath, $output)) {
            throw [System.ArgumentException]::new('instrumented output must not replace the input')
        }
        $outputParent = [System.IO.Path]::GetDirectoryName($output)
        if (-not [System.IO.Directory]::Exists($outputParent)) {
            throw [System.IO.DirectoryNotFoundException]::new('instrumented output directory must already exist')
        }
        $rootCanonical = [System.IO.Path]::TrimEndingDirectorySeparator($rootPath)
        $outputParentCanonical = [System.IO.Path]::TrimEndingDirectorySeparator($outputParent)
        $reservedOutputNames = @('trace.jsonl', 'continue', 'pending.png', 'pending.bmp')
        if ([System.StringComparer]::OrdinalIgnoreCase.Equals($rootCanonical, $outputParentCanonical) -and
            [System.IO.Path]::GetFileName($output) -in $reservedOutputNames) {
            throw [System.ArgumentException]::new('instrumented output collides with a trace or control artifact')
        }
        Test-NoReparseComponents -Path $outputParent
        Test-NoReparseComponents -Path $output
        if ([System.IO.File]::Exists($output) -or [System.IO.Directory]::Exists($output)) {
            throw [System.IO.IOException]::new('instrumented output path must be absent')
        }
    }
    $script:traceStages = @{}
    $script:traceEncoding = [System.Text.UTF8Encoding]::new($false)
    $script:tracePath = [System.IO.Path]::Combine($rootPath, 'trace.jsonl')
    $traceFile = [System.IO.FileStream]::new(
        $script:tracePath,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::Read,
        4096,
        [System.IO.FileOptions]::SequentialScan)
    $script:traceStream = $traceFile
    $process = [System.Diagnostics.Process]::GetCurrentProcess()
    try {
        $script:traceProcessStartTicks = $process.StartTime.ToUniversalTime().Ticks
    }
    finally {
        $process.Dispose()
    }
    $script:traceActive = $true
}

function Write-ProbeTraceStage {
    param(
        [Parameter(Mandatory = $true)][string] $Stage,
        [System.Collections.IDictionary] $Fields = $null
    )
    if (-not $script:traceActive) {
        return
    }
    if ($script:traceFailed) {
        throw [System.IO.IOException]::new('stage trace is already incomplete')
    }
    if ($script:traceRecordCount -ge $TraceMaxRecords) {
        $script:traceFailed = $true
        throw [System.IO.IOException]::new('trace record cap reached')
    }
    $row = [ordered]@{
        schema_version = 1
        seq = $script:traceRecordCount + 1
        stage = $Stage
        pid = $PID
        process_start_utc_ticks = $script:traceProcessStartTicks
        qpc_ticks = [System.Diagnostics.Stopwatch]::GetTimestamp()
        qpc_frequency = [System.Diagnostics.Stopwatch]::Frequency
        clock_domain = 'windows_qpc'
        observer = 'wic_probe'
    }
    if ($null -ne $Fields) {
        foreach ($key in $Fields.Keys) {
            $row[$key] = $Fields[$key]
        }
    }
    if ($PauseAtStage -ceq $Stage) {
        $row.artificial_instrumentation_hold = $true
        $row.pause_timeout_ms = $PauseTimeoutMs
    }
    try {
        $line = ($row | ConvertTo-Json -Compress -Depth 4) + "`n"
        $bytes = $script:traceEncoding.GetBytes($line)
        if ($script:traceByteCount + [long] $bytes.Length -gt [long] $TraceMaxBytes) {
            throw [System.IO.IOException]::new('trace byte cap reached')
        }
        $script:traceStream.Write($bytes, 0, $bytes.Length)
        $script:traceStream.Flush($true)
        $script:traceRecordCount += 1
        $script:traceByteCount += [long] $bytes.Length
        $script:traceStages[$Stage] = $true
    }
    catch {
        $script:traceFailed = $true
        throw
    }
    if ($PauseAtStage -ceq $Stage) {
        $pauseWatch = [System.Diagnostics.Stopwatch]::StartNew()
        $continuePath = [System.IO.Path]::Combine([System.IO.Path]::GetDirectoryName($script:tracePath), 'continue')
        while ($pauseWatch.ElapsedMilliseconds -lt $PauseTimeoutMs) {
            if ([System.IO.Directory]::Exists($continuePath)) {
                throw [System.IO.IOException]::new('continue control must be an operator-created file')
            }
            if ([System.IO.File]::Exists($continuePath)) {
                Test-NoReparseComponents -Path $continuePath
                return
            }
            $remaining = $PauseTimeoutMs - [int] $pauseWatch.ElapsedMilliseconds
            Start-Sleep -Milliseconds ([Math]::Min(25, $remaining))
        }
        throw [System.TimeoutException]::new('operator boundary hold timed out')
    }
}

function Assert-ProbeTracePublishCapacity {
    if (-not $script:traceActive) {
        return
    }
    try {
        if ([long] $script:traceRecordCount + 2 -gt [long] $TraceMaxRecords) {
            throw [System.IO.IOException]::new('trace cap cannot hold publication and terminal records')
        }
        $base = [ordered]@{
            schema_version = 1
            seq = [int]::MaxValue
            stage = ''
            pid = [int]::MaxValue
            process_start_utc_ticks = [long]::MaxValue
            qpc_ticks = [long]::MaxValue
            qpc_frequency = [long]::MaxValue
            clock_domain = 'windows_qpc'
            observer = 'wic_probe'
        }
        $outputRow = [ordered]@{}
        foreach ($key in $base.Keys) { $outputRow[$key] = $base[$key] }
        $outputRow.seq = $script:traceRecordCount + 1
        $outputRow.stage = 'output_published'
        $exitRow = [ordered]@{}
        foreach ($key in $base.Keys) { $exitRow[$key] = $base[$key] }
        $exitRow.seq = $script:traceRecordCount + 2
        $exitRow.stage = 'probe_exit'
        $exitRow.outcome = 'decoded'
        $exitRow.reason_code = $null
        $exitRow.admission_status = 'admitted'
        $exitRow.materialization = 'completed'
        $exitRow.complete = $true
        $needed = [long] $script:traceEncoding.GetByteCount(
            (($outputRow | ConvertTo-Json -Compress -Depth 4) + "`n"))
        $needed += [long] $script:traceEncoding.GetByteCount(
            (($exitRow | ConvertTo-Json -Compress -Depth 4) + "`n"))
        if ($script:traceByteCount + $needed -gt [long] $TraceMaxBytes) {
            throw [System.IO.IOException]::new('trace byte cap cannot hold publication and terminal records')
        }
    }
    catch {
        $script:traceFailed = $true
        throw
    }
}

# Only drive-rooted local paths: no UNC, no device namespace, no drive-relative and
# no root-relative spelling, and no parent traversal.
function Test-LocalDiskPath {
    param([string] $Path)

    if ([string]::IsNullOrWhiteSpace($Path)) { return $false }
    if (-not [System.IO.Path]::IsPathRooted($Path)) { return $false }
    if ($Path -notmatch '^[A-Za-z]:[\\/]') { return $false }
    foreach ($segment in ($Path -split '[\\/]')) {
        if ($segment -eq '..') { return $false }
    }
    return $true
}

# Admission arithmetic and the closed format table live in a dot-sourced helper so
# they are unit-testable without WIC and stay free of budget literals.
. (Join-Path $PSScriptRoot 'wic_admission.ps1')

if ($Version) {
    $identity = 'wic_probe 1.1 (managed WIC/WPF, pre-materialization decode budget, PowerShell {0}, {1})' -f `
        $PSVersionTable.PSVersion, `
        [System.Runtime.InteropServices.RuntimeInformation]::FrameworkDescription
    [Console]::Out.WriteLine($identity)
    [Console]::Out.Flush()
    exit $exitDecoded
}

$result = [ordered]@{
    outcome      = 'error'
    probe        = 'wic-gdiplus-v1'
    input_bytes  = $null
    suffix       = $null
    width        = $null
    height       = $null
    pixel_format = $null
    decoder      = $null
    output_bytes = $null
    hresult      = $null
    reason_code  = 'probe_error'
    reason       = $null
    admission_status          = 'not_enforced'
    materialization           = 'not_requested'
    limits_source             = 'none'
    bits_per_channel          = $null
    bits_per_pixel            = $null
    row_bytes                 = $null
    decoded_bytes_required    = $null
    bgra32_bytes_required     = $null
    declared_scratch_bytes    = $DeclaredScratchBytes
    scratch_bytes_required    = $null
    wic_internal_scratch      = 'not_measured'
}

$script:traceRequested = $PSBoundParameters.ContainsKey('ObservationRoot')
$script:traceCapsDeclared = $PSBoundParameters.ContainsKey('TraceMaxRecords') -and
    $PSBoundParameters.ContainsKey('TraceMaxBytes')
$script:pauseOptionsDeclared = $PSBoundParameters.ContainsKey('PauseAtStage') -or
    $PSBoundParameters.ContainsKey('PauseTimeoutMs')
$script:pauseOptionsComplete = $PSBoundParameters.ContainsKey('PauseAtStage') -and
    $PSBoundParameters.ContainsKey('PauseTimeoutMs')
$script:traceOptionsDeclared = $script:traceRequested -or
    $PSBoundParameters.ContainsKey('TraceMaxRecords') -or
    $PSBoundParameters.ContainsKey('TraceMaxBytes') -or
    $script:pauseOptionsDeclared
$budgetDeclared = Get-BudgetPresence -BoundNames $PSBoundParameters.Keys
if ($script:traceOptionsDeclared) {
    try {
        Initialize-ProbeTrace
        Write-ProbeTraceStage -Stage 'process_observed'
    }
    catch {
        $result.outcome = 'error'
        $result.reason_code = if ($script:traceFailed) { 'trace_failure' } else { 'trace_setup_failed' }
        $result.reason = if ($script:traceFailed) {
            'bounded stage trace could not be started'
        }
        else {
            'instrumented probe arguments or owned paths are invalid'
        }
        Write-ProbeResult -Result $result -ExitCode $exitError
    }
}

if ([string]::IsNullOrWhiteSpace($InputPath)) {
    $result.outcome = 'not_run'
    $result.reason_code = 'argument_missing'
    $result.reason = 'one input path argument is required'
    Write-ProbeResult -Result $result -ExitCode $exitError
}
if (-not (Test-LocalDiskPath $InputPath)) {
    $result.reason_code = 'input_not_local'
    $result.reason = 'input path must be an absolute local disk path'
    Write-ProbeResult -Result $result -ExitCode $exitError
}
if (-not [string]::IsNullOrWhiteSpace($OutputPath) -and -not (Test-LocalDiskPath $OutputPath)) {
    $result.reason_code = 'output_not_local'
    $result.reason = 'output path must be an absolute local disk path'
    Write-ProbeResult -Result $result -ExitCode $exitError
}

$fullPath = [System.IO.Path]::GetFullPath($InputPath)
$file = [System.IO.FileInfo]::new($fullPath)
if (-not $file.Exists) {
    $result.reason_code = 'input_missing'
    $result.reason = 'input file does not exist'
    Write-ProbeResult -Result $result -ExitCode $exitError
}
$result.input_bytes = $file.Length
$result.suffix = $file.Extension.TrimStart('.')

# Budget admission is opt-in through the four limit parameters. When any of them is
# present the whole bundle must be usable: a partial, negative or zero budget is a
# closed refusal, never an implicit default or a clamp. The numbers themselves come
# from the harness's ratified DecodeBudget; no budget constant lives in this script.
$budgetDeclared = Get-BudgetPresence -BoundNames $PSBoundParameters.Keys
if ($PSBoundParameters.ContainsKey('DeclaredScratchBytes') -and -not $budgetDeclared) {
    $result.outcome = 'error'
    $result.reason_code = 'invalid_limits'
    $result.reason = 'a declared scratch allowance requires the four budget limits'
    Write-ProbeResult -Result $result -ExitCode $exitError
}
if ($DeclaredScratchBytes -lt 0 -or $MaxPixels -lt 0 -or $MaxDimension -lt 0 -or
    $MaxBitDepth -lt 0 -or $MaxDecodedBytes -lt 0 -or $MaxScratchBytes -lt 0) {
    $result.outcome = 'error'
    $result.reason_code = 'invalid_limits'
    $result.reason = 'decode budget values must not be negative'
    Write-ProbeResult -Result $result -ExitCode $exitError
}
if ($budgetDeclared -and ($MaxPixels -le 0 -or $MaxDimension -le 0 -or $MaxBitDepth -le 0 -or
        $MaxDecodedBytes -le 0 -or $MaxScratchBytes -le 0)) {
    $result.outcome = 'error'
    $result.reason_code = 'invalid_limits'
    $result.reason = 'budget admission requires every limit to be positive'
    Write-ProbeResult -Result $result -ExitCode $exitError
}

try {
    Add-Type -AssemblyName PresentationCore
}
catch {
    $result.reason_code = 'imaging_unavailable'
    $result.reason = 'managed WPF imaging assembly is unavailable on this platform'
    Write-ProbeResult -Result $result -ExitCode $exitUnsupported
}

$stream = $null
try {
    if ($script:traceActive) {
        Test-NoReparseComponents -Path $fullPath
    }
    $stream = [System.IO.File]::Open(
        $fullPath,
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        [System.IO.FileShare]::Read)
    Write-ProbeTraceStage -Stage 'input_opened'
    # Read declared dimensions before any pixel allocation.
    $declared = [System.Windows.Media.Imaging.BitmapDecoder]::Create(
        $stream,
        [System.Windows.Media.Imaging.BitmapCreateOptions]::DelayCreation,
        [System.Windows.Media.Imaging.BitmapCacheOption]::None)
    $declaredFrame = $declared.Frames[0]
    $result.decoder = $declared.GetType().Name
    $result.width = [long] $declaredFrame.PixelWidth
    $result.height = [long] $declaredFrame.PixelHeight
    $formatName = $declaredFrame.Format.ToString()
    $result.pixel_format = $formatName
    Write-ProbeTraceStage -Stage 'metadata_available'
    Write-ProbeTraceStage -Stage 'before_admission'
    Write-ProbeTraceStage -Stage 'admission_started'

    # Admission happens here, strictly before the OnLoad materialization below. The
    # declared metadata must be usable and the closed format mapping must know the
    # declared depth; neither is inferred from the file suffix or a fallback.
    if ($result.width -lt 1 -or $result.height -lt 1 -or
        $result.width -gt [int]::MaxValue -or $result.height -gt [int]::MaxValue) {
        $result.outcome = 'corrupt'
        $result.reason_code = 'invalid_metadata'
        $result.reason = 'declared dimensions are empty or outside the WIC integer range'
        $result.admission_status = 'refused'
        Write-ProbeResult -Result $result -ExitCode $exitCorrupt
    }
    $formatSpec = Get-DeclaredFormatSpec $formatName
    if ($null -eq $formatSpec) {
        $result.outcome = 'unsupported'
        $result.reason_code = 'unsupported_pixel_format'
        $result.reason = 'declared pixel format is outside the probed format mapping'
        $result.admission_status = 'refused'
        Write-ProbeResult -Result $result -ExitCode $exitUnsupported
    }
    $result.bits_per_channel = $formatSpec.Depth
    $result.bits_per_pixel = $formatSpec.BitsPerPixel
    $math = Get-AdmissionArithmetic -Width $result.width -Height $result.height `
        -BitsPerPixel $formatSpec.BitsPerPixel -DeclaredScratch $DeclaredScratchBytes

    if ($budgetDeclared) {
        $result.limits_source = 'argv'
    }
    if ($budgetDeclared -and ($result.width -gt $MaxDimension -or $result.height -gt $MaxDimension)) {
        $result.outcome = 'oversize'
        $result.reason_code = 'max_dimension_exceeded'
        $result.reason = 'declared edge exceeds the decode-budget maximum dimension'
        $result.admission_status = 'refused'
        Write-ProbeResult -Result $result -ExitCode $exitOversize
    }
    if ($MaxPixels -gt 0 -and $math.Area -gt [System.Numerics.BigInteger] $MaxPixels) {
        $result.outcome = 'oversize'
        $result.reason_code = 'max_pixels_exceeded'
        $result.reason = 'declared dimensions exceed the declared max-pixels bound'
        $result.admission_status = 'refused'
        Write-ProbeResult -Result $result -ExitCode $exitOversize
    }
    if ($budgetDeclared -and $formatSpec.Depth -gt $MaxBitDepth) {
        $result.outcome = 'oversize'
        $result.reason_code = 'max_bit_depth_exceeded'
        $result.reason = 'declared bit depth exceeds the decode-budget maximum'
        $result.admission_status = 'refused'
        Write-ProbeResult -Result $result -ExitCode $exitOversize
    }
    if ($budgetDeclared -and $math.Decoded -gt [System.Numerics.BigInteger] $MaxDecodedBytes) {
        $result.row_bytes = ConvertTo-ReportedLong $math.RowBytes
        $result.decoded_bytes_required = ConvertTo-ReportedLong $math.Decoded
        $result.outcome = 'oversize'
        $result.reason_code = 'max_decoded_bytes_exceeded'
        $result.reason = 'declared source raster exceeds the decode-budget decoded bytes'
        $result.admission_status = 'refused'
        Write-ProbeResult -Result $result -ExitCode $exitOversize
    }
    if ($budgetDeclared -and $math.Scratch -gt [System.Numerics.BigInteger] $MaxScratchBytes) {
        $result.decoded_bytes_required = ConvertTo-ReportedLong $math.Decoded
        $result.bgra32_bytes_required = ConvertTo-ReportedLong $math.Bgra32
        $result.scratch_bytes_required = ConvertTo-ReportedLong $math.Scratch
        $result.outcome = 'oversize'
        $result.reason_code = 'max_scratch_bytes_exceeded'
        $result.reason = 'required conversion plus declared scratch exceeds the decode budget'
        $result.admission_status = 'refused'
        Write-ProbeResult -Result $result -ExitCode $exitOversize
    }
    $result.row_bytes = ConvertTo-ReportedLong $math.RowBytes
    $result.decoded_bytes_required = ConvertTo-ReportedLong $math.Decoded
    $result.bgra32_bytes_required = ConvertTo-ReportedLong $math.Bgra32
    $result.scratch_bytes_required = ConvertTo-ReportedLong $math.Scratch
    $result.admission_status = if ($budgetDeclared) { 'admitted' } else { 'not_enforced' }
    if ($script:traceActive) {
        Write-ProbeTraceStage -Stage 'admitted' -Fields ([ordered]@{
            width = [long] $result.width
            height = [long] $result.height
            bits_per_channel = [int] $result.bits_per_channel
            bits_per_pixel = [int] $result.bits_per_pixel
            row_bytes = [long] $result.row_bytes
            decoded_bytes_required = [long] $result.decoded_bytes_required
            bgra32_bytes_required = [long] $result.bgra32_bytes_required
            declared_scratch_bytes = [long] $result.declared_scratch_bytes
            scratch_bytes_required = [long] $result.scratch_bytes_required
            input_bytes = [long] $result.input_bytes
        })
    }

    # Admit and materialize the whole frame, then require real pixel access.
    $result.materialization = 'requested'
    $stream.Position = 0
    Write-ProbeTraceStage -Stage 'before_onload'
    Write-ProbeTraceStage -Stage 'onload_entered'
    $decoded = [System.Windows.Media.Imaging.BitmapDecoder]::Create(
        $stream,
        [System.Windows.Media.Imaging.BitmapCreateOptions]::None,
        [System.Windows.Media.Imaging.BitmapCacheOption]::OnLoad)
    Write-ProbeTraceStage -Stage 'onload_returned'
    Write-ProbeTraceStage -Stage 'after_onload'
    Write-ProbeTraceStage -Stage 'conversion_entered'
    $converted = [System.Windows.Media.Imaging.FormatConvertedBitmap]::new(
        $decoded.Frames[0],
        [System.Windows.Media.PixelFormats]::Bgra32,
        $null,
        0.0)
    Write-ProbeTraceStage -Stage 'conversion_returned'
    $sample = [byte[]]::new(4)
    Write-ProbeTraceStage -Stage 'copy_pixels_entered'
    $converted.CopyPixels([System.Windows.Int32Rect]::new(0, 0, 1, 1), $sample, 4, 0)
    Write-ProbeTraceStage -Stage 'copy_pixels_returned'

    if (-not [string]::IsNullOrWhiteSpace($OutputPath)) {
        $encoder = switch ([System.IO.Path]::GetExtension($OutputPath).ToLowerInvariant()) {
            '.png' { [System.Windows.Media.Imaging.PngBitmapEncoder]::new() }
            '.bmp' { [System.Windows.Media.Imaging.BmpBitmapEncoder]::new() }
            default { $null }
        }
        if ($null -eq $encoder) {
            $result.outcome = 'unsupported'
            $result.reason_code = 'unsupported_output_extension'
            $result.reason = 'unsupported output extension (.png and .bmp only)'
            Write-ProbeResult -Result $result -ExitCode $exitUnsupported
        }
        $encoder.Frames.Add([System.Windows.Media.Imaging.BitmapFrame]::Create($converted))
        if ($script:traceActive) {
            $pendingExtension = [System.IO.Path]::GetExtension($OutputPath).ToLowerInvariant()
            $script:pendingPath = [System.IO.Path]::Combine(
                [System.IO.Path]::GetDirectoryName($script:tracePath),
                "pending$pendingExtension")
            $encoded = [System.IO.FileStream]::new(
                $script:pendingPath,
                [System.IO.FileMode]::CreateNew,
                [System.IO.FileAccess]::Write,
                [System.IO.FileShare]::None)
        }
        else {
            $encoded = [System.IO.File]::Create($OutputPath)
        }
        try {
            Write-ProbeTraceStage -Stage 'encode_entered'
            $encoder.Save($encoded)
            if ($script:traceActive) {
                $encoded.Flush($true)
            }
        }
        finally {
            $encoded.Dispose()
        }
        $encodedPath = if ($script:traceActive) { $script:pendingPath } else { $OutputPath }
        $result.output_bytes = ([System.IO.FileInfo]::new($encodedPath)).Length
        Write-ProbeTraceStage -Stage 'encode_returned'
        if ($script:traceActive) {
            Write-ProbeTraceStage -Stage 'before_publish'
            Assert-ProbeTracePublishCapacity
            $finalPath = [System.IO.Path]::GetFullPath($OutputPath)
            [System.IO.File]::Move($script:pendingPath, $finalPath)
            $script:publishedOutputPath = $finalPath
            $script:publishedOutputByProbe = $true
            $script:pendingPath = $null
            Write-ProbeTraceStage -Stage 'output_published'
        }
    }

    $result.outcome = 'decoded'
    $result.reason_code = $null
    $result.materialization = 'completed'
    Write-ProbeResult -Result $result -ExitCode $exitDecoded
}
catch {
    $failure = $_.Exception
    while ($null -ne $failure.InnerException) {
        $failure = $failure.InnerException
    }
    if ($script:traceFailed) {
        $result.outcome = 'error'
        $result.reason_code = 'trace_failure'
        $result.reason = 'bounded stage trace could not be written'
        Write-ProbeResult -Result $result -ExitCode $exitError
    }
    if ($script:traceActive -and $failure -is [System.TimeoutException]) {
        $result.outcome = 'error'
        $result.reason_code = 'operator_pause_timeout'
        $result.reason = 'operator boundary hold timed out'
        Write-ProbeResult -Result $result -ExitCode $exitError
    }
    $result.outcome = 'error'
    $result.reason_code = 'probe_error'
    $result.reason = $failure.GetType().Name
    if ($failure -is [System.Runtime.InteropServices.COMException]) {
        $code = '{0:X8}' -f ($failure.HResult -band 0xFFFFFFFF)
        $result.hresult = "0x$code"
        switch ($code) {
            '88982F50' {
                $result.outcome = 'unsupported'
                $result.reason_code = 'win_component_not_found'
                $result.reason = 'WINCODEC_ERR_COMPONENTNOTFOUND'
                $exitCode = $exitUnsupported
            }
            '88982F07' {
                $result.outcome = 'unsupported'
                $result.reason_code = 'win_unknown_image_format'
                $result.reason = 'WINCODEC_ERR_UNKNOWNIMAGEFORMAT'
                $exitCode = $exitUnsupported
            }
            '88982F60' {
                $result.outcome = 'corrupt'
                $result.reason_code = 'win_bad_image'
                $result.reason = 'WINCODEC_ERR_BADIMAGE'
                $exitCode = $exitCorrupt
            }
            '88982F61' {
                $result.outcome = 'corrupt'
                $result.reason_code = 'win_bad_header'
                $result.reason = 'WINCODEC_ERR_BADHEADER'
                $exitCode = $exitCorrupt
            }
            '88982F72' {
                $result.outcome = 'corrupt'
                $result.reason_code = 'win_bad_stream_data'
                $result.reason = 'WINCODEC_ERR_BADSTREAMDATA'
                $exitCode = $exitCorrupt
            }
            default {
                $result.reason_code = 'win_codec_error'
                $exitCode = $exitError
            }
        }
        Write-ProbeResult -Result $result -ExitCode $exitCode
    }
    Write-ProbeResult -Result $result -ExitCode $exitError
}
finally {
    if ($null -ne $stream) {
        $stream.Dispose()
    }
}
