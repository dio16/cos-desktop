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

.EXAMPLE
    pwsh -NoProfile -File wic_probe.ps1 -InputPath C:\tmp\img2-compare\cases\ok.png -OutputPath C:\tmp\img2-compare\wic-out.png
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string] $InputPath = '',

    [string] $OutputPath = '',

    [switch] $Version,

    [long] $MaxPixels = 0
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

if ($Version) {
    $identity = 'wic_probe 1.0 (managed WIC/WPF, PowerShell {0}, {1})' -f `
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
    $result.pixel_format = $declaredFrame.Format.ToString()

    if ($MaxPixels -gt 0 -and ($result.width * $result.height) -gt $MaxPixels) {
        $result.outcome = 'oversize'
        $result.reason_code = 'max_pixels_exceeded'
        $result.reason = 'declared dimensions exceed the operator max-pixels bound'
        Write-ProbeResult -Result $result -ExitCode $exitOversize
    }

    # Admit and materialize the whole frame, then require real pixel access.
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
