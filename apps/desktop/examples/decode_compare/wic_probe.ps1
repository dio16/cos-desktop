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
    `cancelled`/`not_measured` are never emitted: this probe exposes no
    cancellation granularity and no declared scratch budget, so those classes stay
    NOT_MEASURED rather than being guessed.

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

.NOTES
    With budget admission enabled, the script refuses before
    `BitmapDecoder.Create(..., CacheOption::OnLoad)` when the declared width,
    height, format/depth, decoded bytes or scratch requirement exceeds a limit,
    and reports `admission_status`/`materialization` so the refusal point is
    visible. WIC's internal temporaries remain unmeasured: the probe reports
    `wic_internal_scratch = not_measured` and never substitutes zero or a guess.
    A container parse performed by `DelayCreation`/`CacheOption::None` may still
    allocate; no claim of zero allocation is made.

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

    [long] $DeclaredScratchBytes = 0
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'

$exitDecoded = 0
$exitError = 1
$exitUnsupported = 2
$exitCorrupt = 3
$exitOversize = 4

function Write-ProbeResult {
    param(
        [Parameter(Mandatory = $true)]
        [System.Collections.Specialized.OrderedDictionary] $Result,

        [Parameter(Mandatory = $true)]
        [int] $ExitCode
    )
    [Console]::Out.WriteLine(($Result | ConvertTo-Json -Compress -Depth 2))
    [Console]::Out.Flush()
    exit $ExitCode
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

# Declared format support is a closed mapping, not a guess. An unmodelled WIC format is
# refused before materialization rather than assumed to be 8-bit RGBA. Depth is the
# bits-per-channel used for the bit-depth ceiling; BitsPerPixel is WIC's own declared
# storage width and drives the decoded-byte requirement.
function Get-DeclaredFormatSpec {
    param([string] $Name)

    switch ($Name) {
        'Bgra32' { return @{ Depth = 8; BitsPerPixel = 32 } }
        'Pbgra32' { return @{ Depth = 8; BitsPerPixel = 32 } }
        'Prgba64' { return @{ Depth = 16; BitsPerPixel = 64 } }
        'Rgba64' { return @{ Depth = 16; BitsPerPixel = 64 } }
        'Bgr32' { return @{ Depth = 8; BitsPerPixel = 32 } }
        'Bgr24' { return @{ Depth = 8; BitsPerPixel = 24 } }
        'Rgb24' { return @{ Depth = 8; BitsPerPixel = 24 } }
        'Rgb48' { return @{ Depth = 16; BitsPerPixel = 48 } }
        'Gray8' { return @{ Depth = 8; BitsPerPixel = 8 } }
        'Gray16' { return @{ Depth = 16; BitsPerPixel = 16 } }
        'Bgr565' { return @{ Depth = 5; BitsPerPixel = 16 } }
        'Bgr555' { return @{ Depth = 5; BitsPerPixel = 16 } }
        'Bgr101010' { return @{ Depth = 10; BitsPerPixel = 32 } }
        'BlackWhite' { return @{ Depth = 1; BitsPerPixel = 1 } }
        'Indexed1' { return @{ Depth = 1; BitsPerPixel = 1 } }
        'Indexed2' { return @{ Depth = 2; BitsPerPixel = 2 } }
        'Indexed4' { return @{ Depth = 4; BitsPerPixel = 4 } }
        'Indexed8' { return @{ Depth = 8; BitsPerPixel = 8 } }
        default { return $null }
    }
}

# Exact required-byte arithmetic. BigInteger is used only as an exact integer, so a
# hostile declared dimension cannot wrap a 64-bit value into an admitting one.
function Get-AdmissionArithmetic {
    param(
        [long] $Width,
        [long] $Height,
        [int] $BitsPerPixel,
        [long] $DeclaredScratch
    )

    $area = [System.Numerics.BigInteger]::Multiply(
        [System.Numerics.BigInteger] $Width,
        [System.Numerics.BigInteger] $Height)
    $bits = [System.Numerics.BigInteger]::Multiply(
        $area,
        [System.Numerics.BigInteger] $BitsPerPixel)
    $decoded = ([System.Numerics.BigInteger]::Add($bits, 7)) / 8
    $bgra32 = [System.Numerics.BigInteger]::Multiply($area, 4)
    $scratch = [System.Numerics.BigInteger]::Add($bgra32, [System.Numerics.BigInteger] $DeclaredScratch)
    return @{
        Area = $area
        Decoded = $decoded
        Bgra32 = $bgra32
        Scratch = $scratch
    }
}

function ConvertTo-ReportedLong {
    param([System.Numerics.BigInteger] $Value)

    if ($Value -gt [System.Numerics.BigInteger]::Parse([long]::MaxValue.ToString())) {
        return $null
    }
    return [long] $Value
}

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
    decoded_bytes_required    = $null
    bgra32_bytes_required     = $null
    declared_scratch_bytes    = $DeclaredScratchBytes
    scratch_bytes_required    = $null
    wic_internal_scratch      = 'not_measured'
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
$budgetDeclared = ($MaxDimension -ne 0 -or $MaxBitDepth -ne 0 -or
    $MaxDecodedBytes -ne 0 -or $MaxScratchBytes -ne 0)
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
    $stream = [System.IO.File]::Open(
        $fullPath,
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        [System.IO.FileShare]::Read)

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
    $result.decoded_bytes_required = ConvertTo-ReportedLong $math.Decoded
    $result.bgra32_bytes_required = ConvertTo-ReportedLong $math.Bgra32
    $result.scratch_bytes_required = ConvertTo-ReportedLong $math.Scratch
    $result.admission_status = if ($budgetDeclared) { 'admitted' } else { 'not_enforced' }

    # Admit and materialize the whole frame, then require real pixel access.
    $result.materialization = 'requested'
    $stream.Position = 0
    $decoded = [System.Windows.Media.Imaging.BitmapDecoder]::Create(
        $stream,
        [System.Windows.Media.Imaging.BitmapCreateOptions]::None,
        [System.Windows.Media.Imaging.BitmapCacheOption]::OnLoad)
    $converted = [System.Windows.Media.Imaging.FormatConvertedBitmap]::new(
        $decoded.Frames[0],
        [System.Windows.Media.PixelFormats]::Bgra32,
        $null,
        0.0)
    $sample = [byte[]]::new(4)
    $converted.CopyPixels([System.Windows.Int32Rect]::new(0, 0, 1, 1), $sample, 4, 0)

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
        $encoded = [System.IO.File]::Create($OutputPath)
        try {
            $encoder.Save($encoded)
        }
        finally {
            $encoded.Dispose()
        }
        $result.output_bytes = ([System.IO.FileInfo]::new($OutputPath)).Length
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
