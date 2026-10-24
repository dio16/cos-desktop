# MP-VID1 video capability/interop probe (declared, operator-run; probe only).
#
# Emits exactly one bounded JSON document on stdout. Every endpoint degrades independently
# to NOT_MEASURED with a closed reason code; an endpoint the host refuses never becomes a
# zero, a guess or an app-reported stand-in. Capture is bounded while reading: the probe
# stops reading at -MaxOutputBytes, kills the child and records PROCESS_OUTPUT_CAPPED, so
# the cap is a memory bound rather than a post-hoc size check.
#
# What it observes:
#   * Media Foundation platform startup (MFStartup HRESULT), the MF component versions and
#     the registered transform counts per MFT category. The category GUIDs come from the
#     Windows SDK `mfapi.h` on the probe host, and every count carries the MFTEnumFlag it
#     was produced with: 0x1 SYNCMFT (software V1), 0x2 ASYNCMFT (software V2), 0x4
#     HARDWARE, 0x3F ALL. A count without its flag is meaningless, so the flag values are
#     recorded in the document.
#   * D3D11 device creation (hardware and WARP) with the resulting feature level, plus the
#     display-adapter identity (identity only, never a decode-capability claim).
#   * Optional interop experiments through an operator-supplied FFmpeg (absolute path):
#     software decode, seek, raw NV12/P010 output, D3D11VA/DXVA2 decode and an explicit
#     hardware-frame download. Hardware decoding is OBSERVED only when the decoder log shows
#     a hardware `pix_fmt` and at least one frame decoded; a reported DXVA GUID list is
#     recorded as information but is never treated as hardware evidence, and a silent
#     software fallback is UNSUPPORTED/HWACCEL_SOFTWARE_FALLBACK.
#   * A bounded, read-only scan of the pinned product sources for a surface-presentation call
#     site, which grounds the route-limitation statement MP-VID1 must produce (either a
#     working presentation route, or the limitation of the examined route with grounds).
#
# Safety: only absolute operator-supplied program paths are used (no PATH search, which is
# empty under a Refine Gate); argv is passed as a separate argument vector (no command
# string is built or interpolated); every child has a timeout, both captured streams are
# bounded and the child is killed on overflow; the scratch directory is created by this
# probe and only that directory is deleted unless -KeepScratch was requested.
param(
    [string]$FfmpegPath = '',
    [string]$ScratchRoot = '',
    [string]$SourceRoot = '',
    [string]$GitPath = '',
    [switch]$KeepScratch,
    [int]$TimeoutMs = 15000,
    [int]$MaxOutputBytes = 262144,
    [int]$MaxFixtureBytes = 67108864
)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8

$ProbeVersion = 3
$MaxExperiments = 24
$MaxPixFmts = 8
$MaxDxvaGuids = 16
$MaxSourceFiles = 3000
$MaxSourceFileBytes = 1048576
$MaxSourceMatches = 20

# MFTEnumFlag values, copied from the Windows SDK header (mfapi.h 10.0.19041.0):
#   0x1 MFT_ENUM_FLAG_SYNCMFT, 0x2 MFT_ENUM_FLAG_ASYNCMFT (software V2),
#   0x4 MFT_ENUM_FLAG_HARDWARE (V2 hardware), 0x3F MFT_ENUM_FLAG_ALL.
$MftFlagSync = 0x1
$MftFlagAsync = 0x2
$MftFlagHardware = 0x4
$MftFlagAll = 0x3F

# Decoder pixel formats that can only come from a hardware decoder surface.
$HardwarePixFmts = @('d3d11', 'd3d11va', 'dxva2_vld', 'cuda', 'qsv', 'vaapi', 'vulkan')

# Tokens of a surface-presentation route into the pinned UI path. A match means the route
# may exist; no match means this checkout contains no such call site.
$SurfaceTokens = @('draw_surfaces', 'IDXGISurface', 'ID3D11Texture2D', 'ID3D11VideoDevice', 'SurfaceHandle', 'import_surface')

function Emit($Value) {
    $Value | ConvertTo-Json -Depth 8 -Compress
}

function Fail([string]$Code) {
    Emit @{ ok = $false; probe_version = $ProbeVersion; error = $Code }
    exit 0
}

if ($TimeoutMs -lt 1000 -or $TimeoutMs -gt 120000) { Fail 'CAP_RANGE' }
if ($MaxOutputBytes -lt 1024 -or $MaxOutputBytes -gt 1048576) { Fail 'CAP_RANGE' }
if ($MaxFixtureBytes -lt 1048576 -or $MaxFixtureBytes -gt 134217728) { Fail 'CAP_RANGE' }

function New-Section([string]$Status, [string]$Reason) {
    [ordered]@{ status = $Status; reason = $Reason }
}

function Get-Hex([int]$HResult) {
    '0x{0:X8}' -f ($HResult -band 0xFFFFFFFF)
}

# ---------------------------------------------------------------------------
# Media Foundation / D3D11 capability (P/Invoke; no shell, no COM vtable calls)
# ---------------------------------------------------------------------------
$signature = @'
using System;
using System.Runtime.InteropServices;
public static class Vid1Native
{
    [DllImport("mfplat.dll", ExactSpelling = true)]
    public static extern int MFStartup(int version, uint flags);
    [DllImport("mfplat.dll", ExactSpelling = true)]
    public static extern int MFShutdown();
    [DllImport("mfplat.dll", ExactSpelling = true)]
    public static extern int MFTEnumEx(Guid category, uint flags, IntPtr input, IntPtr output,
                                      out IntPtr activates, out uint count);
    [DllImport("d3d11.dll", ExactSpelling = true)]
    public static extern int D3D11CreateDevice(IntPtr adapter, int driverType, IntPtr software,
        uint flags, IntPtr featureLevels, uint featureLevelCount, uint sdkVersion,
        out IntPtr device, out int featureLevel, out IntPtr context);
    [DllImport("ole32.dll", ExactSpelling = true)]
    public static extern void CoTaskMemFree(IntPtr pointer);
}
'@

$mediaFoundation = New-Section 'NOT_MEASURED' 'MF_COMPONENT_MISSING'
$d3d11 = [ordered]@{
    status = 'NOT_MEASURED'; reason = 'D3D11_DEVICE_UNAVAILABLE'
    hardware_hresult = ''; hardware_feature_level = ''; warp_hresult = ''; warp_feature_level = ''
    adapters = @()
}
$mfComponents = @()
$mftRows = @()
$adapters = @()
$mftGuidSource = 'Windows SDK 10.0.19041.0 um/mfapi.h'

function Get-MftCount([Guid]$Category, [uint32]$Flag) {
    $activates = [IntPtr]::Zero
    $count = 0
    $hr = [Vid1Native]::MFTEnumEx($Category, $Flag, [IntPtr]::Zero, [IntPtr]::Zero, [ref]$activates, [ref]$count)
    if ($activates -ne [IntPtr]::Zero -and $count -gt 0) {
        for ($i = 0; $i -lt $count; $i++) {
            $pointer = [System.Runtime.InteropServices.Marshal]::ReadIntPtr($activates, $i * [IntPtr]::Size)
            if ($pointer -ne [IntPtr]::Zero) { [void][System.Runtime.InteropServices.Marshal]::Release($pointer) }
        }
        [Vid1Native]::CoTaskMemFree($activates)
    }
    return @{ hresult = $hr; count = $(if ($hr -eq 0) { $count } else { $null }) }
}

try {
    foreach ($component in @('mfplat.dll', 'mf.dll', 'mfreadwrite.dll', 'd3d11.dll', 'dxva2.dll')) {
        $path = [System.IO.Path]::Combine($env:WINDIR, 'System32', $component)
        if (Test-Path -LiteralPath $path) {
            $version = ''
            try { $version = (Get-Item -LiteralPath $path).VersionInfo.FileVersion } catch { $version = '' }
            $mfComponents += [ordered]@{ name = $component; status = 'OBSERVED'; reason = ''; version = $version }
        }
        else {
            $mfComponents += [ordered]@{ name = $component; status = 'NOT_MEASURED'; reason = 'MF_COMPONENT_MISSING'; version = '' }
        }
    }

    Add-Type -TypeDefinition $signature -ErrorAction Stop

    $startup = [Vid1Native]::MFStartup(0x00020070, 0)
    if ($startup -eq 0) {
        $mediaFoundation = New-Section 'OBSERVED' ''
        $categoryGuids = [ordered]@{
            'video_decoder' = 'd6c02d4b-6833-45b4-971a-05a4b04bab91'
            'video_encoder' = 'f79eac7d-e545-4387-bdee-d647d7bde42a'
            'video_processor' = '302ea3fc-aa5f-47f9-9f7a-c2188bb16302'
            'audio_decoder' = '9ea73fb4-ef7a-4559-8d5d-719d8f0426c7'
        }
        foreach ($name in $categoryGuids.Keys) {
            $guid = [Guid]$categoryGuids[$name]
            $all = Get-MftCount $guid ([uint32]$MftFlagAll)
            $sync = Get-MftCount $guid ([uint32]$MftFlagSync)
            $async = Get-MftCount $guid ([uint32]$MftFlagAsync)
            $hardware = Get-MftCount $guid ([uint32]$MftFlagHardware)
            $row = [ordered]@{
                category = $name
                guid = $guid.ToString()
                hresult = (Get-Hex ([int]$all.hresult))
                all = $all.count; sync = $sync.count; async = $async.count; hardware = $hardware.count
                sync_status = $(if ($null -ne $sync.count) { 'OBSERVED' } else { 'NOT_MEASURED' })
                async_status = $(if ($null -ne $async.count) { 'OBSERVED' } else { 'NOT_MEASURED' })
                hardware_status = $(if ($null -ne $hardware.count) { 'OBSERVED' } else { 'NOT_MEASURED' })
                reason = $(if ($null -eq $all.count) { 'MFT_ENUMERATION_FAILED' } else { '' })
            }
            $mftRows += $row
        }
        [void][Vid1Native]::MFShutdown()
    }
    else {
        $mediaFoundation = New-Section 'NOT_MEASURED' 'MF_STARTUP_FAILED'
    }
}
catch {
    if ($mediaFoundation.status -eq 'OBSERVED') { $mediaFoundation = New-Section 'NOT_MEASURED' 'MF_STARTUP_FAILED' }
}

try {
    if (-not ('Vid1Native' -as [type])) { Add-Type -TypeDefinition $signature -ErrorAction Stop }
    $device = [IntPtr]::Zero; $context = [IntPtr]::Zero; $level = 0
    $hardwareHr = [Vid1Native]::D3D11CreateDevice([IntPtr]::Zero, 1, [IntPtr]::Zero, 0, [IntPtr]::Zero, 0, 7, [ref]$device, [ref]$level, [ref]$context)
    $hardwareLevel = if ($hardwareHr -eq 0) { '0x{0:X}' -f $level } else { '' }
    if ($context -ne [IntPtr]::Zero) { [void][System.Runtime.InteropServices.Marshal]::Release($context) }
    if ($device -ne [IntPtr]::Zero) { [void][System.Runtime.InteropServices.Marshal]::Release($device) }
    $device = [IntPtr]::Zero; $context = [IntPtr]::Zero; $level = 0
    $warpHr = [Vid1Native]::D3D11CreateDevice([IntPtr]::Zero, 5, [IntPtr]::Zero, 0, [IntPtr]::Zero, 0, 7, [ref]$device, [ref]$level, [ref]$context)
    $warpLevel = if ($warpHr -eq 0) { '0x{0:X}' -f $level } else { '' }
    if ($context -ne [IntPtr]::Zero) { [void][System.Runtime.InteropServices.Marshal]::Release($context) }
    if ($device -ne [IntPtr]::Zero) { [void][System.Runtime.InteropServices.Marshal]::Release($device) }
    $d3d11 = [ordered]@{
        status = 'OBSERVED'; reason = ''
        hardware_hresult = (Get-Hex $hardwareHr); hardware_feature_level = $hardwareLevel
        warp_hresult = (Get-Hex $warpHr); warp_feature_level = $warpLevel
        adapters = @()
    }
}
catch {
    $d3d11 = [ordered]@{
        status = 'NOT_MEASURED'; reason = 'D3D11_DEVICE_UNAVAILABLE'
        hardware_hresult = ''; hardware_feature_level = ''; warp_hresult = ''; warp_feature_level = ''
        adapters = @()
    }
}

try {
    foreach ($controller in (Get-CimInstance Win32_VideoController -ErrorAction Stop)) {
        $adapters += [ordered]@{
            name = [string]$controller.Name
            driver_version = [string]$controller.DriverVersion
        }
    }
}
catch {
    $adapters = @()
}
$d3d11.adapters = @($adapters)

# ---------------------------------------------------------------------------
# Absolute-path helpers, bounded child capture
# ---------------------------------------------------------------------------
function Resolve-AbsoluteLocal([string]$Path, [string]$Reason, [string]$Code = '', [switch]$Directory) {
    $failure = @{ ok = $false; reason = $Reason; code = $Code }
    if ([string]::IsNullOrWhiteSpace($Path)) { return $failure }
    if (-not [System.IO.Path]::IsPathRooted($Path)) { return $failure }
    if ($Path -notmatch '^[A-Za-z]:[\\/]') { return $failure }
    foreach ($segment in ($Path -split '[\\/]')) { if ($segment -eq '..') { return $failure } }
    if ($Directory) {
        if (-not (Test-Path -LiteralPath $Path -PathType Container)) { return $failure }
    }
    elseif (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return @{ ok = $false; reason = 'TOOL_NOT_FOUND'; code = 'TOOL_NOT_FOUND' }
    }
    return @{ ok = $true; path = [System.IO.Path]::GetFullPath($Path); reason = ''; code = '' }
}

function Invoke-Tool([string[]]$Argv) {
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $Argv[0]
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    for ($index = 1; $index -lt $Argv.Count; $index++) { $psi.ArgumentList.Add($Argv[$index]) }
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        $process = [System.Diagnostics.Process]::Start($psi)
    }
    catch {
        return @{ status = 'NOT_MEASURED'; reason = 'PROCESS_SPAWN_FAILED'; exit_code = $null; wall_ms = $null
            stdout = ''; stderr = ''; capped = $false; timed_out = $false }
    }
    $exitCode = $null
    $capped = $false
    $timedOut = $false
    $stdout = ''
    $stderr = ''
    $outStream = $process.StandardOutput.BaseStream
    $errStream = $process.StandardError.BaseStream
    $outBuffer = [byte[]]::new(4096)
    $errBuffer = [byte[]]::new(4096)
    $out = [System.IO.MemoryStream]::new()
    $err = [System.IO.MemoryStream]::new()
    $outTask = $outStream.ReadAsync($outBuffer, 0, $outBuffer.Length)
    $errTask = $errStream.ReadAsync($errBuffer, 0, $errBuffer.Length)
    try {
        while ($null -ne $outTask -or $null -ne $errTask) {
            if ($watch.ElapsedMilliseconds -gt $TimeoutMs) { $timedOut = $true; break }
            $pending = [System.Collections.Generic.List[System.Threading.Tasks.Task]]::new()
            if ($null -ne $outTask) { $pending.Add($outTask) }
            if ($null -ne $errTask) { $pending.Add($errTask) }
            [void][System.Threading.Tasks.Task]::WaitAny($pending.ToArray(), 100)
            if ($null -ne $outTask -and $outTask.IsCompleted) {
                $read = $outTask.Result
                if ($read -le 0) { $outTask = $null }
                elseif ($out.Length + $read -gt $MaxOutputBytes) { $capped = $true; break }
                else {
                    $out.Write($outBuffer, 0, $read)
                    $outTask = $outStream.ReadAsync($outBuffer, 0, $outBuffer.Length)
                }
            }
            if ($null -ne $errTask -and $errTask.IsCompleted) {
                $read = $errTask.Result
                if ($read -le 0) { $errTask = $null }
                elseif ($err.Length + $read -gt $MaxOutputBytes) { $capped = $true; break }
                else {
                    $err.Write($errBuffer, 0, $read)
                    $errTask = $errStream.ReadAsync($errBuffer, 0, $errBuffer.Length)
                }
            }
        }
        if (-not $capped -and -not $timedOut) {
            if (-not $process.WaitForExit($TimeoutMs)) { $timedOut = $true }
            else { $exitCode = $process.ExitCode }
        }
        if ($capped -or $timedOut) {
            try { $process.Kill($true) } catch { }
            try { [void]$process.WaitForExit(2000) } catch { }
        }
        if ($out.Length -gt 0) { $stdout = [System.Text.Encoding]::UTF8.GetString($out.ToArray()) }
        if ($err.Length -gt 0) { $stderr = [System.Text.Encoding]::UTF8.GetString($err.ToArray()) }
    }
    finally {
        $watch.Stop()
        $out.Dispose(); $err.Dispose()
        try { $process.Dispose() } catch { }
    }
    return @{
        status = 'OBSERVED'; reason = ''; exit_code = $exitCode; wall_ms = $watch.ElapsedMilliseconds
        stdout = $stdout; stderr = $stderr; capped = $capped; timed_out = $timedOut
    }
}

function Get-FramesDecoded([string]$Text) {
    $summary = [regex]::Matches($Text, '(\d+) frames decoded')
    if ($summary.Count -gt 0) { return [int]$summary[$summary.Count - 1].Groups[1].Value }
    $progress = [regex]::Matches($Text, 'frame=\s*(\d+)')
    if ($progress.Count -gt 0) { return [int]$progress[$progress.Count - 1].Groups[1].Value }
    return $null
}

function Get-PixFmts([string]$Text) {
    $values = @()
    foreach ($match in [regex]::Matches($Text, 'pix_?fmt:\s*([a-z0-9_]+)')) {
        $value = $match.Groups[1].Value
        if ($values -notcontains $value -and $values.Count -lt $MaxPixFmts) { $values += $value }
    }
    return $values
}

function Get-DxvaGuids([string]$Text) {
    $guids = @()
    $enabled = $false
    foreach ($line in ($Text -split "`n")) {
        if ($line -match 'Decoder GUIDs reported as supported') { $enabled = $true; continue }
        if (-not $enabled) { continue }
        $match = [regex]::Match($line, '\{[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\}')
        if ($match.Success -and $guids.Count -lt $MaxDxvaGuids) { $guids += $match.Value.ToLowerInvariant() }
        elseif ($line -notmatch '^\s*\[' -and $guids.Count -gt 0) { $enabled = $false }
    }
    return $guids
}

function Get-HardwarePixFmt([string[]]$PixFmts) {
    foreach ($format in $PixFmts) {
        if ($HardwarePixFmts -contains $format) { return $format }
    }
    return ''
}

# ---------------------------------------------------------------------------
# Interop experiments
# ---------------------------------------------------------------------------
function New-ToolSection([string]$Status, [string]$Reason, [string]$Path, [string]$Sha256, [string[]]$Hwaccels) {
    [ordered]@{ status = $Status; reason = $Reason; path = $Path; sha256 = $Sha256; hwaccels = @($Hwaccels) }
}

function New-ObservedToolSection([string]$Path) {
    $hash = ''
    try { $hash = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() } catch { $hash = '' }
    $section = New-ToolSection 'OBSERVED' '' $Path $hash @()
    $run = Invoke-Tool @($Path, '-hide_banner', '-hwaccels')
    if ($run.status -eq 'OBSERVED' -and $run.exit_code -eq 0) {
        $section.hwaccels = @(($run.stdout -split "`n") | ForEach-Object { $_.Trim() } |
            Where-Object { $_ -match '^[a-z0-9]+$' } | Select-Object -First 16)
    }
    return $section
}

$interop = @()
$fixtures = @()
$toolSection = New-ToolSection 'NOT_MEASURED' 'TOOL_NOT_SUPPLIED' '' '' @()
$scratch = $null
$scratchRetained = $false
$expected = [ordered]@{
    h264 = @{ width = 640; height = 360; bytes_nv12 = 640 * 360 * 3 / 2; bytes_p010 = 640 * 360 * 3 }
    hevc10 = @{ width = 320; height = 180; bytes_nv12 = 320 * 180 * 3 / 2; bytes_p010 = 320 * 180 * 3 }
    vp8 = @{ width = 160; height = 90; bytes_nv12 = 160 * 90 * 3 / 2; bytes_p010 = 160 * 90 * 3 }
}
$fixtureSpecs = @(
    @{ id = 'h264'; container = 'mp4'; file = 'fixture_h264_8bit.mp4'; codec = 'h264'; pixel = 'yuv420p'; rate = 30; seconds = 2; width = 640; height = 360; encoder = @('-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-preset', 'ultrafast') }
    @{ id = 'hevc10'; container = 'mp4'; file = 'fixture_hevc_10bit.mp4'; codec = 'hevc'; pixel = 'yuv420p10le'; rate = 30; seconds = 1; width = 320; height = 180; encoder = @('-c:v', 'libx265', '-pix_fmt', 'yuv420p10le', '-preset', 'ultrafast', '-x265-params', 'log-level=none') }
    @{ id = 'vp8'; container = 'webm'; file = 'fixture_vp8.webm'; codec = 'vp8'; pixel = 'yuv420p'; rate = 30; seconds = 1; width = 160; height = 90; encoder = @('-c:v', 'libvpx', '-pix_fmt', 'yuv420p') }
)

function New-InteropRow([string]$Id, [string]$Kind, [string]$Container, [string]$Codec,
                        [string]$PixelFormat, [string]$Hwaccel) {
    [ordered]@{
        id = $Id; kind = $Kind; container = $Container; codec = $Codec
        pixel_format = $PixelFormat; hwaccel = $Hwaccel
        argv = @(); exit_code = $null; wall_ms = $null
        status = 'NOT_MEASURED'; reason = ''; stdout_bytes = 0; stderr_bytes = 0; capped = $false
        frames_decoded = $null; decoder_pix_fmts = @(); hardware_pix_fmt = ''
        hardware_frame_evidence = 'NONE'; dxva_profile_guids = @()
        output_bytes = $null; expected_output_bytes = $null
    }
}

function Set-RowFailure($Row, [string]$Reason) {
    $Row.status = 'NOT_MEASURED'; $Row.reason = $Reason
}

function New-DisabledInterop([string]$Reason) {
    @([ordered]@{ id = 'interop_disabled'; kind = 'decode'; container = ''; codec = ''; pixel_format = ''; hwaccel = ''
        argv = @(); exit_code = $null; wall_ms = $null; status = 'NOT_MEASURED'; reason = $Reason
        stdout_bytes = 0; stderr_bytes = 0; capped = $false; frames_decoded = $null
        decoder_pix_fmts = @(); hardware_pix_fmt = ''; hardware_frame_evidence = 'NONE'
        dxva_profile_guids = @(); output_bytes = $null; expected_output_bytes = $null })
}

if ([string]::IsNullOrWhiteSpace($FfmpegPath)) {
    $toolSection = New-ToolSection 'NOT_MEASURED' 'TOOL_NOT_SUPPLIED' '' '' @()
    $interop = New-DisabledInterop 'INTEROP_DISABLED'
}
elseif ([string]::IsNullOrWhiteSpace($ScratchRoot)) {
    $toolSection = New-ToolSection 'NOT_MEASURED' 'SCRATCH_NOT_SUPPLIED' '' '' @()
    $interop = New-DisabledInterop 'INTEROP_DISABLED'
}
else {
    $tool = Resolve-AbsoluteLocal $FfmpegPath 'TOOL_PATH_INVALID'
    $scratchCheck = Resolve-AbsoluteLocal $ScratchRoot 'SCRATCH_UNAVAILABLE' -Directory
    if (-not $tool.ok) {
        $toolSection = New-ToolSection 'NOT_MEASURED' $tool.reason '' '' @()
        $interop = New-DisabledInterop 'INTEROP_DISABLED'
    }
    elseif (-not $scratchCheck.ok) {
        # The tool stays observable (hash + -hwaccels need no scratch), but no experiment
        # runs in an undeclared directory: the declared scratch root is never created here.
        $toolSection = New-ObservedToolSection $tool.path
        $interop = New-DisabledInterop 'SCRATCH_UNAVAILABLE'
    }
    else {
        $toolSection = New-ObservedToolSection $tool.path
        try {
            $token = 'vid1-probe-{0}-{1}' -f $PID, ([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds())
            $scratch = [System.IO.Path]::Combine($scratchCheck.path, $token)
            [System.IO.Directory]::CreateDirectory($scratch) | Out-Null
            $scratchRetained = [bool]$KeepScratch

            $fixturePath = @{}
            foreach ($spec in $fixtureSpecs) {
                $path = [System.IO.Path]::Combine($scratch, $spec.file)
                # `+bitexact` both ways keeps the generated bytes reproducible across runs, so a
                # reader can regenerate the exact fixture recorded by SHA-256.
                $argv = @($tool.path, '-hide_banner', '-loglevel', 'error', '-y', '-fflags', '+bitexact',
                    '-f', 'lavfi', '-i', ("testsrc2=size={0}x{1}:rate={2}" -f $spec.width, $spec.height, $spec.rate),
                    '-t', [string]$spec.seconds) + $spec.encoder +
                    @('-flags:v', '+bitexact', '-fflags', '+bitexact', $path)
                $generated = Invoke-Tool $argv
                $row = [ordered]@{
                    id = $spec.id; container = $spec.container; codec = $spec.codec; pixel_format = $spec.pixel
                    status = 'NOT_MEASURED'; reason = 'FIXTURE_GENERATION_FAILED'
                    bytes = $null; sha256 = ''; width = $spec.width; height = $spec.height
                    seconds = $spec.seconds; generator_argv = @($argv)
                }
                if ($generated.capped) { $row.reason = 'PROCESS_OUTPUT_CAPPED' }
                elseif ($generated.timed_out) { $row.reason = 'PROCESS_TIMEOUT' }
                elseif ($generated.status -ne 'OBSERVED' -or $generated.exit_code -ne 0) { $row.reason = 'FIXTURE_GENERATION_FAILED' }
                elseif (-not (Test-Path -LiteralPath $path)) { $row.reason = 'FIXTURE_GENERATION_FAILED' }
                else {
                    $bytes = (Get-Item -LiteralPath $path).Length
                    if ($bytes -gt $MaxFixtureBytes) { $row.reason = 'FIXTURE_TOO_LARGE' }
                    else {
                        $digest = ''
                        try { $digest = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() } catch { $digest = '' }
                        if ($digest -match '^[0-9a-f]{64}$') {
                            $row.status = 'OBSERVED'; $row.reason = ''
                            $row.bytes = $bytes; $row.sha256 = $digest
                            $fixturePath[$spec.id] = $path
                        }
                        else {
                            $row.reason = 'FIXTURE_GENERATION_FAILED'
                        }
                    }
                }
                $fixtures += $row
            }

            function Add-Experiment([string]$Id, [string]$Kind, [string]$FixtureId,
                                    [string]$Hwaccel, [string[]]$PreInputArgv, [string[]]$PostInputArgv,
                                    [string]$ExpectedKey, [string]$LogLevel, [int]$Frames = 2) {
                if ($interop.Count -ge $MaxExperiments) { return }
                $spec = $fixtureSpecs | Where-Object { $_.id -eq $FixtureId } | Select-Object -First 1
                $row = New-InteropRow $Id $Kind $spec.container $spec.codec $spec.pixel $Hwaccel
                if (-not $fixturePath.ContainsKey($FixtureId)) {
                    $missing = $fixtures | Where-Object { $_.id -eq $FixtureId } | Select-Object -First 1
                    $row.reason = if ($missing) { $missing.reason } else { 'FIXTURE_GENERATION_FAILED' }
                    $script:interop += $row
                    return
                }
                if ($ExpectedKey) {
                    $outputPath = [System.IO.Path]::Combine($scratch, ("{0}.raw" -f $Id))
                    $argv = @($tool.path, '-hide_banner', '-loglevel', $LogLevel, '-stats', '-y') + $PreInputArgv +
                        @('-i', $fixturePath[$FixtureId], '-frames:v', '1') + $PostInputArgv +
                        @('-f', 'rawvideo', $outputPath)
                }
                else {
                    $limit = if ($Frames -gt 0) { @('-frames:v', [string]$Frames) } else { @() }
                    $argv = @($tool.path, '-hide_banner', '-loglevel', $LogLevel, '-stats', '-y') + $PreInputArgv +
                        @('-i', $fixturePath[$FixtureId]) + $limit + $PostInputArgv + @('-f', 'null', '-')
                }
                $row.argv = @($argv)
                $run = Invoke-Tool $argv
                $row.stdout_bytes = $run.stdout.Length
                $row.stderr_bytes = $run.stderr.Length
                $row.capped = $run.capped
                $row.wall_ms = $run.wall_ms
                $row.exit_code = $run.exit_code
                if ($run.timed_out) { Set-RowFailure $row 'PROCESS_TIMEOUT' }
                elseif ($run.capped) { Set-RowFailure $row 'PROCESS_OUTPUT_CAPPED' }
                elseif ($run.exit_code -ne 0) {
                    Set-RowFailure $row $(if ($Kind -eq 'hw_download' -or $Kind -eq 'hw_decode') { 'HWACCEL_REQUEST_REJECTED' } else { 'PROCESS_FAILED' })
                }
                else {
                    $row.frames_decoded = Get-FramesDecoded $run.stderr
                    $row.decoder_pix_fmts = @(Get-PixFmts $run.stderr)
                    $row.dxva_profile_guids = @(Get-DxvaGuids $run.stderr)
                    $row.hardware_pix_fmt = Get-HardwarePixFmt $row.decoder_pix_fmts
                    if ($row.hardware_pix_fmt -ne '') { $row.hardware_frame_evidence = 'PIX_FMT_OBSERVED' }
                    $row.status = 'OBSERVED'
                    if ($Kind -eq 'hw_decode' -or $Kind -eq 'hw_download') {
                        if ($row.hardware_frame_evidence -eq 'PIX_FMT_OBSERVED' -and $null -ne $row.frames_decoded -and $row.frames_decoded -ge 1) {
                            $row.status = 'OBSERVED'; $row.reason = ''
                        }
                        else {
                            $row.status = 'UNSUPPORTED'; $row.reason = 'HWACCEL_SOFTWARE_FALLBACK'
                        }
                    }
                    if ($ExpectedKey) {
                        $outputPath = [System.IO.Path]::Combine($scratch, ("{0}.raw" -f $Id))
                        $specForBytes = $expected[$FixtureId]
                        $row.expected_output_bytes = if ($ExpectedKey -eq 'nv12') { $specForBytes.bytes_nv12 } else { $specForBytes.bytes_p010 }
                        if (-not (Test-Path -LiteralPath $outputPath)) {
                            $row.status = 'NOT_MEASURED'; $row.reason = 'RAW_OUTPUT_SIZE_UNEXPECTED'
                        }
                        else {
                            $row.output_bytes = (Get-Item -LiteralPath $outputPath).Length
                            if ($row.output_bytes -ne $row.expected_output_bytes) {
                                $row.status = 'NOT_MEASURED'; $row.reason = 'RAW_OUTPUT_SIZE_UNEXPECTED'
                            }
                        }
                    }
                }
                $script:interop += $row
            }

            Add-Experiment 'sw_h264_first_frame' 'decode' 'h264' 'none' @() @() '' 'error'
            Add-Experiment 'sw_h264_decode_all' 'decode' 'h264' 'none' @() @() '' 'error' 0
            Add-Experiment 'sw_h264_seek_1s' 'seek' 'h264' 'none' @() @('-ss', '1.0') '' 'error'
            Add-Experiment 'sw_h264_nv12' 'raw_output' 'h264' 'none' @() @('-pix_fmt', 'nv12') 'nv12' 'error'
            Add-Experiment 'hw_d3d11va_h264_decode' 'hw_decode' 'h264' 'd3d11va' @('-hwaccel', 'd3d11va', '-hwaccel_output_format', 'd3d11') @() '' 'verbose'
            Add-Experiment 'hw_d3d11va_h264_download' 'hw_download' 'h264' 'd3d11va' @('-hwaccel', 'd3d11va', '-hwaccel_output_format', 'd3d11') @('-vf', 'hwdownload,format=nv12') 'nv12' 'verbose'
            Add-Experiment 'hw_dxva2_h264_decode' 'hw_decode' 'h264' 'dxva2' @('-hwaccel', 'dxva2') @() '' 'verbose'
            Add-Experiment 'sw_hevc10_first_frame' 'decode' 'hevc10' 'none' @() @() '' 'error'
            Add-Experiment 'sw_hevc10_p010' 'raw_output' 'hevc10' 'none' @() @('-pix_fmt', 'p010le') 'p010' 'error'
            Add-Experiment 'hw_d3d11va_hevc10_decode' 'hw_decode' 'hevc10' 'd3d11va' @('-hwaccel', 'd3d11va', '-hwaccel_output_format', 'd3d11') @() '' 'verbose'
            Add-Experiment 'hw_d3d11va_hevc10_download' 'hw_download' 'hevc10' 'd3d11va' @('-hwaccel', 'd3d11va', '-hwaccel_output_format', 'd3d11') @('-vf', 'hwdownload,format=p010le') 'p010' 'verbose'
            Add-Experiment 'sw_vp8_first_frame' 'decode' 'vp8' 'none' @() @() '' 'error'
            Add-Experiment 'hw_d3d11va_vp8_decode' 'hw_decode' 'vp8' 'd3d11va' @('-hwaccel', 'd3d11va', '-hwaccel_output_format', 'd3d11') @() '' 'verbose'
        }
        catch {
            if ($toolSection.status -ne 'OBSERVED') {
                $toolSection = New-ToolSection 'NOT_MEASURED' 'PROBE_INTERNAL_ERROR' '' '' @()
            }
        }
        finally {
            if (-not $KeepScratch -and $null -ne $scratch -and (Test-Path -LiteralPath $scratch)) {
                try { Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue } catch { }
                $scratchRetained = $false
            }
        }
    }
}

# ---------------------------------------------------------------------------
# Bounded read-only scan for a surface-presentation call site.
#
# This is PARTIAL supporting material and never a route conclusion: a string search that
# finds no identifier does not prove the pinned GPUI revision has no presentation route.
# The scan is bound to what it actually scanned (commit SHA, tree state when a git
# executable is named by absolute path, file/size caps and every file skipped for size);
# a truncated or unbound scan is recorded as such and is not usable as support.
# The pinned API implementation itself is recorded as NOT inspected.
# ---------------------------------------------------------------------------
function Get-GitHead([string]$Root) {
    $result = @{ ok = $false; sha = ''; reason = 'GIT_UNAVAILABLE' }
    try {
        $headPath = [System.IO.Path]::Combine($Root, '.git', 'HEAD')
        if (-not (Test-Path -LiteralPath $headPath -PathType Leaf)) { return $result }
        $head = ([System.IO.File]::ReadAllText($headPath)).Trim()
        if ($head -match '^[0-9a-fA-F]{40,64}$') {
            return @{ ok = $true; sha = $head.ToLowerInvariant(); reason = '' }
        }
        if ($head -match '^ref:\s*(.+)$') {
            $reference = $Matches[1].Trim()
            if ($reference -notmatch '^[A-Za-z0-9._/\-]+$') { return $result }
            $refPath = [System.IO.Path]::Combine($Root, '.git', ($reference -replace '/', '\'))
            if (-not (Test-Path -LiteralPath $refPath -PathType Leaf)) { return $result }
            $sha = ([System.IO.File]::ReadAllText($refPath)).Trim()
            if ($sha -match '^[0-9a-fA-F]{40,64}$') {
                return @{ ok = $true; sha = $sha.ToLowerInvariant(); reason = '' }
            }
        }
        return $result
    }
    catch {
        return $result
    }
}

function Get-GitTreeState([string]$GitPath, [string]$Root) {
    $run = Invoke-Tool @($GitPath, '-C', $Root, 'status', '--porcelain')
    if ($run.status -ne 'OBSERVED' -or $run.timed_out -or $run.capped -or $run.exit_code -ne 0) {
        return @{ ok = $false; clean = $null; reason = 'GIT_STATUS_NOT_MEASURED' }
    }
    return @{ ok = $true; clean = ($run.stdout.Trim().Length -eq 0); reason = '' }
}

$sourceScan = [ordered]@{
    status = 'NOT_MEASURED'; reason = 'SOURCE_SCAN_NOT_RUN'; root = ''
    git_head_sha = ''; git_tree_clean = $null; git_status_reason = 'GIT_UNAVAILABLE'; git_path = ''
    file_cap = $MaxSourceFiles; file_size_cap = $MaxSourceFileBytes
    files_scanned = 0; files_skipped_size = 0; cap_hit = $false; bytes_scanned = 0
    tokens = @($SurfaceTokens); matches = @(); signal = 'NOT_RUN'
}

if (-not [string]::IsNullOrWhiteSpace($SourceRoot)) {
    $source = Resolve-AbsoluteLocal $SourceRoot 'SOURCE_SCAN_UNAVAILABLE' -Directory
    if (-not $source.ok) {
        $sourceScan.status = 'NOT_MEASURED'; $sourceScan.reason = 'SOURCE_SCAN_UNAVAILABLE'
    }
    else {
        try {
            $sourceScan.root = $source.path
            $head = Get-GitHead $source.path
            if ($head.ok) {
                $sourceScan.git_head_sha = $head.sha
                $sourceScan.git_status_reason = 'GIT_STATUS_NOT_MEASURED'
            }
            else {
                $sourceScan.git_status_reason = $head.reason
            }
            if (-not [string]::IsNullOrWhiteSpace($GitPath)) {
                $git = Resolve-AbsoluteLocal $GitPath 'GIT_UNAVAILABLE'
                if ($git.ok) {
                    $sourceScan.git_path = $git.path
                    $tree = Get-GitTreeState $git.path $source.path
                    $sourceScan.git_tree_clean = $tree.clean
                    $sourceScan.git_status_reason = $tree.reason
                }
                else {
                    $sourceScan.git_status_reason = 'GIT_UNAVAILABLE'
                }
            }
            $allFiles = @(Get-ChildItem -LiteralPath $source.path -Recurse -File -Filter '*.rs' -ErrorAction Stop |
                Where-Object { $_.FullName -notmatch '[\\/](\.git|target|node_modules)[\\/]' })
            $files = @($allFiles | Select-Object -First $MaxSourceFiles)
            $sourceScan.cap_hit = ($allFiles.Count -gt $MaxSourceFiles)
            foreach ($file in $files) {
                if ($file.Length -gt $MaxSourceFileBytes) {
                    $sourceScan.files_skipped_size++
                    continue
                }
                $sourceScan.files_scanned++
                $sourceScan.bytes_scanned += $file.Length
                $lineNumber = 0
                foreach ($line in [System.IO.File]::ReadLines($file.FullName)) {
                    $lineNumber++
                    foreach ($token in $SurfaceTokens) {
                        if ($line.IndexOf($token, [System.StringComparison]::Ordinal) -ge 0) {
                            if ($sourceScan.matches.Count -lt $MaxSourceMatches) {
                                $relative = $file.FullName.Substring($source.path.Length).TrimStart([char[]]@('\', '/'))
                                $sourceScan.matches += [ordered]@{ file = $relative.Replace('\', '/'); line = $lineNumber; token = $token }
                            }
                        }
                    }
                }
            }
            $sourceScan.status = 'OBSERVED'; $sourceScan.reason = ''
        }
        catch {
            $sourceScan.status = 'NOT_MEASURED'; $sourceScan.reason = 'SOURCE_SCAN_UNAVAILABLE'
        }
    }
}

# Only a scan that ran, is bound to a commit, saw a clean tree and skipped nothing (by cap
# or by file size) is usable as supporting material. Everything else is recorded as such.
if ($sourceScan.status -ne 'OBSERVED') {
    $sourceScan.signal = 'NOT_RUN'
}
elseif (-not $sourceScan.git_head_sha) {
    $sourceScan.signal = 'UNBOUND'
}
elseif ($sourceScan.cap_hit -or $sourceScan.files_skipped_size -gt 0) {
    $sourceScan.signal = 'TRUNCATED'
}
elseif ($sourceScan.git_tree_clean -ne $true) {
    $sourceScan.signal = 'SHA_BOUND_TREE_STATE_UNKNOWN'
}
else {
    $sourceScan.signal = 'COMPLETE_BOUND'
}

$pinInspection = [ordered]@{
    status = 'NOT_RUN'; reason = 'PIN_API_NOT_INSPECTED'
    pin = ''; api_surface = ''; findings = @()
}

$hardwareRows = @($interop | Where-Object { $_.kind -eq 'hw_decode' -or $_.kind -eq 'hw_download' })
$hardwareObserved = @($hardwareRows | Where-Object { $_.status -eq 'OBSERVED' -and $_.hardware_frame_evidence -eq 'PIX_FMT_OBSERVED' })
$scanUsable = ($sourceScan.signal -eq 'COMPLETE_BOUND')
$scanDetail = 'signal=' + $sourceScan.signal
$scanDetail += '; scanned ' + $sourceScan.files_scanned + ' .rs files (' + $sourceScan.bytes_scanned + ' bytes)'
$scanDetail += '; file_cap=' + $sourceScan.file_cap + ' cap_hit=' + $sourceScan.cap_hit
$scanDetail += '; file_size_cap=' + $sourceScan.file_size_cap + ' files_skipped_size=' + $sourceScan.files_skipped_size
$scanDetail += '; sha=' + $(if ($sourceScan.git_head_sha) { $sourceScan.git_head_sha } else { 'unbound' })
$scanDetail += '; tree_clean=' + $(if ($null -eq $sourceScan.git_tree_clean) { 'not_measured' } else { $sourceScan.git_tree_clean })
$scanDetail += '; matches=' + $sourceScan.matches.Count + ' for ' + ($SurfaceTokens -join ', ')
$missingEvidence = @('PIN_API_INSPECTION', 'PIN_SURFACE_API', 'COPY_COUNT', 'SYNC_STALLS', 'COLOR_METADATA', 'DEVICE_LOSS', 'PLAYBACK_LATENCY')
$route = [ordered]@{
    route_id = 'mf-d3d11va-decode:d3d11-texture:gpui-surface-presentation'
    status = 'UNEXAMINED'
    reason = 'PIN_API_NOT_INSPECTED'
    decode_side_observed = ($hardwareObserved.Count -gt 0)
    scan_signal = $sourceScan.signal
    scan_usable_as_support = $scanUsable
    pin_api_inspection = $pinInspection.status
    grounds = @(
        [ordered]@{
            id = 'decode_side_capability'
            status = $(if ($hardwareObserved.Count -gt 0) { 'OBSERVED' } else { 'NOT_MEASURED' })
            detail = $(if ($hardwareObserved.Count -gt 0) { 'hardware frames observed for ' + (($hardwareObserved | ForEach-Object { $_.codec } | Sort-Object -Unique) -join ', ') } else { 'no hardware decode row was observed' })
        }
        [ordered]@{
            id = 'presentation_call_site_scan'
            status = $(if ($scanUsable) { 'OBSERVED' } else { 'NOT_MEASURED' })
            detail = $scanDetail
        }
        [ordered]@{
            id = 'pin_surface_documented_noop'
            status = 'DOCUMENTED'
            detail = 'DC-MP-VID1 records the pinned GPUI Windows surface path as a no-op; this is documentation, not a measurement'
        }
        [ordered]@{
            id = 'pin_api_inspection'
            status = 'NOT_MEASURED'
            detail = 'the pinned GPUI presentation API implementation was not inspected by this probe, so no route limitation is concluded from the scan'
        }
        [ordered]@{
            id = 'copy_sync_color_device_loss_uninstrumented'
            status = 'NOT_MEASURED'
            detail = 'copy counts, synchronisation stalls, colour metadata and device loss were not instrumented'
        }
    )
    missing_evidence = @($missingEvidence)
}

$endpoints = @(
    [ordered]@{ endpoint = 'gpu_surface_import'; status = 'NOT_MEASURED'; reason = 'GPU_SURFACE_IMPORT_UNVERIFIED' }
    [ordered]@{ endpoint = 'presentation_route'; status = 'NOT_MEASURED'; reason = 'PRESENTATION_PATH_UNMEASURED' }
    [ordered]@{ endpoint = 'copy_count'; status = 'NOT_MEASURED'; reason = 'COPY_COUNT_NOT_INSTRUMENTED' }
    [ordered]@{ endpoint = 'synchronisation_stalls'; status = 'NOT_MEASURED'; reason = 'SYNC_STALL_NOT_INSTRUMENTED' }
    [ordered]@{ endpoint = 'color_metadata'; status = 'NOT_MEASURED'; reason = 'COLOR_METADATA_NOT_PROBED' }
    [ordered]@{ endpoint = 'device_loss'; status = 'NOT_MEASURED'; reason = 'DEVICE_LOSS_NOT_EXERCISED' }
    [ordered]@{ endpoint = 'dxva_profile_names'; status = 'NOT_MEASURED'; reason = 'DXVA_PROFILE_NAMES_NOT_RESOLVED' }
    [ordered]@{ endpoint = 'product_playback_latency'; status = 'NOT_MEASURED'; reason = 'PRODUCT_PATH_NOT_IMPLEMENTED' }
)

$corrections = @(
    [ordered]@{
        id = 'mft_hardware_flag'
        status = 'CORRECTED_IN_THIS_RECORD'
        detail = 'The earlier VID1 record reported video_decoder hardware=0 and video_encoder hardware=1 from MFTEnumEx with flag 0x2, which is MFT_ENUM_FLAG_ASYNCMFT (software V2), not MFT_ENUM_FLAG_HARDWARE (0x4). Those numbers are async-software counts and were never hardware counts; this record measures hardware with 0x4 and keeps the earlier observation as a pre-fix observation only.'
    }
    [ordered]@{
        id = 'route_claim_from_scan_only'
        status = 'CORRECTED_IN_THIS_RECORD'
        detail = 'The previous record concluded CONFIRMED_LIMITATION from a hardware decode observation plus the absence of surface identifiers in the supplied source directory. That does not prove the pinned GPUI revision has no presentation route, so the route is now UNEXAMINED/PIN_API_NOT_INSPECTED and the scan is retained as bounded supporting material with its commit binding, caps and skipped files recorded.'
    }
)

$document = [ordered]@{
    ok = $true
    probe_version = $ProbeVersion
    caps = [ordered]@{
        timeout_ms = $TimeoutMs; max_output_bytes = $MaxOutputBytes
        max_fixture_bytes = $MaxFixtureBytes; max_experiments = $MaxExperiments
    }
    mft_flags = [ordered]@{ sync = '0x1'; async = '0x2'; hardware = '0x4'; all = '0x3F' }
    host = [ordered]@{
        status = 'OBSERVED'; reason = ''
        os_description = [System.Runtime.InteropServices.RuntimeInformation]::OSDescription
        arch = [string][System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
        pwsh = ($PSVersionTable.PSVersion).ToString()
        interop_enabled = (-not [string]::IsNullOrWhiteSpace($FfmpegPath))
        scratch_retained = $scratchRetained
        scratch_path = $(if ($scratchRetained -and $null -ne $scratch) { $scratch } else { '' })
    }
    media_foundation = [ordered]@{
        status = $mediaFoundation.status; reason = $mediaFoundation.reason
        mft_guid_source = $mftGuidSource
        components = @($mfComponents)
        mft_enumeration = @($mftRows)
    }
    d3d11 = [ordered]@{
        status = $d3d11.status; reason = $d3d11.reason
        hardware_hresult = $d3d11.hardware_hresult; hardware_feature_level = $d3d11.hardware_feature_level
        warp_hresult = $d3d11.warp_hresult; warp_feature_level = $d3d11.warp_feature_level
        adapters = @($adapters)
    }
    tool = $toolSection
    fixtures = @($fixtures)
    interop = @($interop)
    source_scan = $sourceScan
    pin_api_inspection = $pinInspection
    route = $route
    endpoints = @($endpoints)
    corrections = @($corrections)
}

Emit $document
exit 0
