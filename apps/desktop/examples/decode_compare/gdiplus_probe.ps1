#!/usr/bin/env pwsh
#Requires -Version 7.0
<#
.SYNOPSIS
    Windows GDI+ decode probe: the operator-run managed System.Drawing decode
    candidate for the exploratory MP-IMG2 comparison harness.

.DESCRIPTION
    Managed System.Drawing (Windows GDI+) only. This probe does **not** use
    Windows Imaging Component / WPF, does not use libgdiplus, performs no C#
    compilation step, no P/Invoke of its own, no native FFmpeg call, no shell, no
    network and no GPU or presentation object.

    Transport (harness record schema v9; argv is always passed separately, never
    through a command string):

        -Version                            identity probe: one line on stdout, exit 0
        -InputPath <abs> [-OutputPath <abs>] [-MaxPixels N]
                                            measurement: one compact JSON line on
                                            stdout and the matching protocol exit code

    A harness run reaches this script as
        pwsh -NoProfile -File gdiplus_probe.ps1 -InputPath <in> -OutputPath <out> [-MaxPixels N]
    and its identity probe as
        pwsh -NoProfile -File gdiplus_probe.ps1 -Version
    The harness always asks for a `<stem>.<candidate>.wic.png` output, so the encoder
    format never depends on the input suffix.

    Exit codes mirror the harness outcome vocabulary:
        0 decoded       GDI+ admitted the container and materialized pixels
        2 unsupported   only the *output* extension: no GDI+ encoder bound to it;
                        never used for an input class
        3 corrupt       reserved; this probe never emits it (see below)
        4 oversize      dimensions exceed the operator-declared -MaxPixels bound
        1 error         anything else, including GDI+ input failures and an
                        unavailable System.Drawing (an environment failure must
                        never present as `unsupported`)

.NOTES
    Observed environment of these facts (measured on the development machine):
    Windows 11, PowerShell 7.6.7, .NET 10.0.13, System.Drawing.Common 10.0.0.0,
    gdiplus.dll 10.0.26100.9711. The identity line reports the actual runtime,
    assembly and loaded gdiplus.dll file versions it sees.

    What counts as a decode here (N1/N3). A header read alone is not evidence, and
    GDI+ has no header-only read to begin with:
      1. Image.FromStream on a FileShare.Read stream loads the container and
         allocates the decoded surface for its declared geometry, and
      2. a `Bitmap` construction from that image materializes the whole frame a
         second time (converted to 32bpp ARGB), and a 1x1 `GetPixel` forces real
         pixel access, because a lazy or header-only image cannot satisfy it.
    With -OutputPath the materialized frame is additionally re-encoded, which is
    what the harness measures as output size.

    GDI+ capability limits this probe records honestly instead of papering over:

      1. **No unsupported-versus-invalid distinction.** GDI+ exposes exactly one
         generic input failure per entry point, not a codec-classified one. The load
         path this probe actually uses, `Image.FromStream` on a `FileShare.Read`
         stream, reports an unrecognized container (for example a text file) as
         `System.ArgumentException` with `E_INVALIDARG`, HRESULT `0x80070057`, which
         is the same shape of failure a damaged stream produces. (GDI+'s other entry
         point, `Image.FromFile`, reports the identical condition as
         `System.Runtime.InteropServices.ExternalException` with `E_UNEXPECTED`
         0x8000FFFF; this probe does not use it, so 0x80070057 is the HRESULT this
         candidate records.) This probe therefore emits `error` and never claims
         `unsupported` or `corrupt` for input: either claim would invent a
         distinction GDI+ does not make. The R4 admission classes for this candidate
         are consequently coarser than for the WIC/WPF candidate, and that difference
         is the measurement.
      2. **No header-only load: the declared geometry costs the full frame.** GDI+
         has no delayed-creation / header-only decode. The only way to read the
         declared width and height is `Image.FromStream`, and GDI+ allocates the
         decoded surface at load time, before the caller can look at the geometry.
         Measured on the development host with a 20000x20000 claim (65-byte file):
         the process working set went from 91.3 MiB to 1243.2 MiB at the
         `FromStream` call itself, before any pixel access; the second copy made by
         the `Bitmap` materialization would add another ~1.6 GiB. The
         `-MaxPixels` bound below therefore prevents the second allocation and the
         encode, and **cannot** prevent the first: for this candidate an oversize
         refusal is a *post-allocation* refusal, not an admission gate.
      3. **A corrupt zlib stream is not rejected at load.** A PNG whose `IDAT` is
         not a zlib stream at all can still be loaded and materialized; GDI+
         returns zeroed pixels (an encode then succeeds, so the probe's own verdict
         is `decoded`). The probe does not recompute chunk CRCs, does not verify the
         zlib Adler-32 and cannot detect a wrong-but-decodable pixel stream: only
         the harness's independent pixel oracle can, when the operator declares
         `pixels:<sha256>`, and that is exactly what happened on the record this
         probe was added for.
      4. **No declared scratch budget and no cancellation granularity.** The probe
         exposes neither, so it never emits `cancelled` or `not_measured`; those
         classes stay NOT_MEASURED (the harness records its own deadline kill as
         `cancelled`, which is not the candidate's claim).

    `reason_code` is a closed machine vocabulary the harness whitelists; `reason`
    is human text the harness never reads. This probe emits only tokens that already
    exist in that vocabulary (`probe_error`, `input_missing`, `input_not_local`,
    `output_not_local`, `max_pixels_exceeded`, `imaging_unavailable`,
    `unsupported_output_extension`, `argument_missing`); a GDI+-specific token
    vocabulary would change the record's value domain, which is not this probe's
    decision to make.

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
    Optional operator-declared pixel bound, checked on the declared geometry as soon
    as it can be read. GDI+ allocates that geometry at load time, so the bound limits
    the second (Bitmap) materialization and the encode, not the first allocation
    (capability note 2 above). 0 (the default) declares no bound.

.EXAMPLE
    pwsh -NoProfile -File gdiplus_probe.ps1 -InputPath C:\tmp\img2-gdiplus\cases\ok.png -OutputPath C:\tmp\img2-gdiplus\gdiplus-out.png
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

# The real codec implementation version, as observed from the module GDI+ loaded.
# A 1x1 bitmap forces gdiplus.dll into this process; without it the module is absent.
function Get-GdiplusVersion {
    try {
        # A 1x1 bitmap loads gdiplus.dll into this process; without it the module is
        # absent from the module list and its file version cannot be observed.
        $probe = [System.Drawing.Bitmap]::new(1, 1)
        try {
            $module = (Get-Process -Id $PID).Modules |
                Where-Object { $_.ModuleName -ieq 'gdiplus.dll' } |
                Select-Object -First 1
        }
        finally {
            $probe.Dispose()
        }
        if ($null -ne $module) {
            return [System.Diagnostics.FileVersionInfo]::GetVersionInfo($module.FileName).FileVersion
        }
    }
    catch {
        return 'unknown'
    }
    return 'unknown'
}

# The managed imaging assembly is resolved once, so the identity probe can report an
# unavailable candidate instead of failing to register at all.
$imagingFailure = $null
try {
    Add-Type -AssemblyName System.Drawing
}
catch {
    $imagingFailure = $_.Exception.GetType().Name
}

if ($Version) {
    if ($null -eq $imagingFailure) {
        $assembly = [System.Drawing.Bitmap].Assembly.GetName().Version
        $gdiplus = Get-GdiplusVersion
    }
    else {
        $assembly = "unavailable ($imagingFailure)"
        $gdiplus = 'unavailable'
    }
    $identity = 'gdiplus_probe 1.0 (managed System.Drawing/GDI+, gdiplus.dll ' + $gdiplus +
        ', System.Drawing.Common ' + $assembly + ', ' +
        [System.Runtime.InteropServices.RuntimeInformation]::FrameworkDescription +
        ', PowerShell ' + $PSVersionTable.PSVersion + ')'
    [Console]::Out.WriteLine($identity)
    [Console]::Out.Flush()
    exit $exitDecoded
}

$result = [ordered]@{
    outcome      = 'error'
    probe        = 'system-drawing-gdiplus-v1'
    input_bytes  = $null
    suffix       = $null
    width        = $null
    height       = $null
    pixel_format = $null
    decoder      = $null
    raw_format   = $null
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

if ($null -ne $imagingFailure) {
    # An unavailable environment is `error`, never `unsupported`: exit 2 would be
    # recorded as a codec-capability claim, which this observation cannot support.
    $result.reason_code = 'imaging_unavailable'
    $result.reason = 'managed System.Drawing is unavailable on this platform'
    Write-ProbeResult -Result $result -ExitCode $exitError
}

$stream = $null
$image = $null
$frame = $null
try {
    # FileShare.Read keeps the input readable by the other candidates and by the
    # harness while this process runs; GDI+ requires the stream to stay open.
    $stream = [System.IO.File]::Open(
        $fullPath,
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        [System.IO.FileShare]::Read)

    # Container and declared geometry. GDI+ has no header-only load, so this call
    # allocates the decoded surface for the declared geometry; the -MaxPixels check
    # below can only bound the *second* (Bitmap) allocation and the encode.
    $image = [System.Drawing.Image]::FromStream($stream)
    $result.width = [long] $image.Width
    $result.height = [long] $image.Height
    $result.pixel_format = $image.PixelFormat.ToString()
    $result.raw_format = $image.RawFormat.Guid.ToString()
    $codec = [System.Drawing.Imaging.ImageCodecInfo]::GetImageDecoders() |
        Where-Object { $_.FormatID -eq $image.RawFormat.Guid } |
        Select-Object -First 1
    if ($null -ne $codec) {
        $result.decoder = [string] $codec.CodecName
    }
    else {
        $result.decoder = 'gdiplus-unknown-codec'
    }

    if ($MaxPixels -gt 0 -and ($result.width * $result.height) -gt $MaxPixels) {
        $result.outcome = 'oversize'
        $result.reason_code = 'max_pixels_exceeded'
        $result.reason = 'declared dimensions exceed the operator max-pixels bound'
        Write-ProbeResult -Result $result -ExitCode $exitOversize
    }

    # Materialize the whole frame, then require real pixel access.
    $frame = [System.Drawing.Bitmap]::new($image)
    $null = $frame.GetPixel(0, 0)

    if (-not [string]::IsNullOrWhiteSpace($OutputPath)) {
        $encoder = switch ([System.IO.Path]::GetExtension($OutputPath).ToLowerInvariant()) {
            '.png' { [System.Drawing.Imaging.ImageFormat]::Png }
            '.bmp' { [System.Drawing.Imaging.ImageFormat]::Bmp }
            default { $null }
        }
        if ($null -eq $encoder) {
            $result.outcome = 'unsupported'
            $result.reason_code = 'unsupported_output_extension'
            $result.reason = 'unsupported output extension (.png and .bmp only)'
            Write-ProbeResult -Result $result -ExitCode $exitUnsupported
        }
        $frame.Save($OutputPath, $encoder)
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
    # GDI+ reports one generic failure for every rejected input, so this probe keeps
    # `error` and records the raw HRESULT instead of inventing `corrupt`/`unsupported`.
    $result.outcome = 'error'
    $result.reason_code = 'probe_error'
    $result.reason = $failure.GetType().Name
    $result.hresult = '0x{0:X8}' -f ($failure.HResult -band 0xFFFFFFFF)
    Write-ProbeResult -Result $result -ExitCode $exitError
}
finally {
    if ($null -ne $frame) { $frame.Dispose() }
    if ($null -ne $image) { $image.Dispose() }
    if ($null -ne $stream) { $stream.Dispose() }
}
