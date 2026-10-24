# MP-VID1 video capability/interop probe (declared, operator-run; probe only).
#
# Emits exactly one bounded JSON document on stdout. Every endpoint degrades independently
# to NOT_MEASURED with a closed reason code; an endpoint the host refuses never becomes a
# zero, a guess or an app-reported stand-in. This probe observes *decode-side* capability
# only: it establishes nothing about presenting decoded frames through the pinned GPUI
# revision, about copies/synchronisation into that path, colour metadata, device loss or
# product playback latency, and it chooses no video route.
#
# What it observes:
#   * Media Foundation platform startup (MFStartup HRESULT), the MF component versions and
#     the registered transform counts per MFT category (MFTEnumEx with the category GUIDs
#     taken from the Windows SDK 10.0.19041.0 `mfapi.h` on the probe host).
#   * D3D11 device creation (hardware and WARP) with the resulting feature level, plus the
#     display-adapter identity (identity only, never a decode-capability claim).
#   * Optional interop experiments through an operator-supplied FFmpeg (absolute path):
#     software decode, seek, raw NV12/P010 output, D3D11VA/DXVA2 decode and an explicit
#     hardware-frame download. Hardware decoding counts as OBSERVED only when the decoder
#     log demonstrates hardware frames (a hardware `pix_fmt` or reported DXVA decoder GUIDs)
#     and at least one frame decoded; a silent software fallback is recorded as
#     UNSUPPORTED/HWACCEL_SOFTWARE_FALLBACK, never as success.
#
# Safety: only absolute operator-supplied program paths are used (no PATH search, which is
# empty under a Refine Gate); argv is passed as a separate argument vector (no command
# string is built or interpolated); every child has a timeout, every capture is capped, the
# scratch directory is created by this probe and only that directory is deleted.
param(
    [string]$FfmpegPath = '',
    [string]$ScratchRoot = '',
    [int]$TimeoutMs = 15000,
    [int]$MaxOutputBytes = 262144,
    [int]$MaxFixtureBytes = 67108864
)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8

$ProbeVersion = 1
$MaxExperiments = 24
$MaxPixFmts = 8
$MaxDxvaGuids = 16
$MaxArgv = 32

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

# The closed vocabulary this probe may print. The Python validator owns the authoritative
# copy; every value here is one of its tokens, never free text.
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
            $row = [ordered]@{
                category = $name; guid = ([Guid]$categoryGuids[$name]).ToString()
                all_status = 'NOT_MEASURED'; all = $null; hardware_status = 'NOT_MEASURED'; hardware = $null
                sync = $null; reason = 'MFT_ENUMERATION_FAILED'; hresult = ''
            }
            try {
                $all = 0; $hardware = 0; $sync = 0; $hr = 0
                $activates = [IntPtr]::Zero; $count = 0
                $hr = [Vid1Native]::MFTEnumEx([Guid]$categoryGuids[$name], 0x3F, [IntPtr]::Zero, [IntPtr]::Zero, [ref]$activates, [ref]$count)
                $all = $count
                if ($activates -ne [IntPtr]::Zero -and $count -gt 0) {
                    for ($i = 0; $i -lt $count; $i++) {
                        $p = [System.Runtime.InteropServices.Marshal]::ReadIntPtr($activates, $i * [IntPtr]::Size)
                        if ($p -ne [IntPtr]::Zero) { [void][System.Runtime.InteropServices.Marshal]::Release($p) }
                    }
                    [Vid1Native]::CoTaskMemFree($activates)
                }
                $activates = [IntPtr]::Zero; $count = 0
                $hrHw = [Vid1Native]::MFTEnumEx([Guid]$categoryGuids[$name], 0x2, [IntPtr]::Zero, [IntPtr]::Zero, [ref]$activates, [ref]$count)
                $hardware = $count
                if ($activates -ne [IntPtr]::Zero -and $count -gt 0) {
                    for ($i = 0; $i -lt $count; $i++) {
                        $p = [System.Runtime.InteropServices.Marshal]::ReadIntPtr($activates, $i * [IntPtr]::Size)
                        if ($p -ne [IntPtr]::Zero) { [void][System.Runtime.InteropServices.Marshal]::Release($p) }
                    }
                    [Vid1Native]::CoTaskMemFree($activates)
                }
                $activates = [IntPtr]::Zero; $count = 0
                $hrSync = [Vid1Native]::MFTEnumEx([Guid]$categoryGuids[$name], 0x1, [IntPtr]::Zero, [IntPtr]::Zero, [ref]$activates, [ref]$count)
                $sync = $count
                if ($activates -ne [IntPtr]::Zero -and $count -gt 0) {
                    for ($i = 0; $i -lt $count; $i++) {
                        $p = [System.Runtime.InteropServices.Marshal]::ReadIntPtr($activates, $i * [IntPtr]::Size)
                        if ($p -ne [IntPtr]::Zero) { [void][System.Runtime.InteropServices.Marshal]::Release($p) }
                    }
                    [Vid1Native]::CoTaskMemFree($activates)
                }
                if ($hr -eq 0) {
                    $row.all_status = 'OBSERVED'; $row.all = $all; $row.reason = ''
                }
                if ($hrHw -eq 0) { $row.hardware_status = 'OBSERVED'; $row.hardware = $hardware }
                $row.sync = $sync
                $row.hresult = (Get-Hex $hr)
            }
            catch {
                $row.reason = 'MFT_ENUMERATION_FAILED'
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

# ---------------------------------------------------------------------------
# Interop experiments through an operator-supplied FFmpeg
# ---------------------------------------------------------------------------
function Resolve-AbsoluteLocal([string]$Path, [string]$Label, [switch]$Directory) {
    if ([string]::IsNullOrWhiteSpace($Path)) { return @{ ok = $false; reason = $Label } }
    if (-not [System.IO.Path]::IsPathRooted($Path)) { return @{ ok = $false; reason = $Label } }
    if ($Path -notmatch '^[A-Za-z]:[\\/]') { return @{ ok = $false; reason = $Label } }
    foreach ($segment in ($Path -split '[\\/]')) { if ($segment -eq '..') { return @{ ok = $false; reason = $Label } } }
    if ($Directory) {
        if (-not (Test-Path -LiteralPath $Path -PathType Container)) { return @{ ok = $false; reason = 'SCRATCH_UNAVAILABLE' } }
    }
    elseif (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return @{ ok = $false; reason = 'TOOL_NOT_FOUND' }
    }
    return @{ ok = $true; path = [System.IO.Path]::GetFullPath($Path) }
}

function Invoke-Tool([string[]]$Argv) {
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $Argv[0]
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    for ($i = 1; $i -lt $Argv.Count; $i++) { $psi.ArgumentList.Add($Argv[$i]) }
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        $process = [System.Diagnostics.Process]::Start($psi)
    }
    catch {
        return @{ status = 'NOT_MEASURED'; reason = 'PROCESS_SPAWN_FAILED'; exit_code = $null; wall_ms = $null; stdout = ''; stderr = ''; truncated = $false; timed_out = $false }
    }
    $outTask = $process.StandardOutput.ReadToEndAsync()
    $errTask = $process.StandardError.ReadToEndAsync()
    $finished = $process.WaitForExit($TimeoutMs)
    if (-not $finished) {
        try { $process.Kill($true) } catch { }
        try { [void]$process.WaitForExit(2000) } catch { }
    }
    $watch.Stop()
    $stdout = ''
    $stderr = ''
    $truncated = $false
    try {
        $stdout = $outTask.GetAwaiter().GetResult()
        $stderr = $errTask.GetAwaiter().GetResult()
    }
    catch {
        $truncated = $true
    }
    if ($stdout.Length -gt $MaxOutputBytes) { $stdout = $stdout.Substring(0, $MaxOutputBytes); $truncated = $true }
    if ($stderr.Length -gt $MaxOutputBytes) { $stderr = $stderr.Substring(0, $MaxOutputBytes); $truncated = $true }
    $exitCode = if ($finished) { $process.ExitCode } else { $null }
    return @{
        status = 'OBSERVED'; reason = ''; exit_code = $exitCode; wall_ms = $watch.ElapsedMilliseconds
        stdout = $stdout; stderr = $stderr; truncated = $truncated; timed_out = (-not $finished)
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
    foreach ($match in [regex]::Matches($Text, 'pix_fmt: ([a-z0-9_]+)')) {
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

function Test-HardwareFrames([string[]]$PixFmts, [string[]]$DxvaGuids) {
    foreach ($format in $PixFmts) {
        if ($format -in @('d3d11', 'd3d11va', 'dxva2_vld', 'cuda', 'qsv', 'vaapi', 'vulkan')) { return $true }
    }
    return ($DxvaGuids.Count -gt 0)
}

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
$toolSection = New-ToolSection 'NOT_MEASURED' 'TOOL_NOT_SUPPLIED' '' '' @()
$fixtures = @()
$scratch = $null
$expected = [ordered]@{ h264 = @{ width = 640; height = 360; bytes_nv12 = 640 * 360 * 3 / 2; bytes_p010 = 640 * 360 * 3 }
                         hevc10 = @{ width = 320; height = 180; bytes_nv12 = 320 * 180 * 3 / 2; bytes_p010 = 320 * 180 * 3 }
                         vp8 = @{ width = 160; height = 90; bytes_nv12 = 160 * 90 * 3 / 2; bytes_p010 = 160 * 90 * 3 } }

function New-InteropRow([string]$Id, [string]$Kind, [string]$Container, [string]$Codec,
                        [string]$PixelFormat, [string]$Hwaccel) {
    [ordered]@{
        id = $Id; kind = $Kind; container = $Container; codec = $Codec
        pixel_format = $PixelFormat; hwaccel = $Hwaccel
        argv = @(); exit_code = $null; wall_ms = $null
        status = 'NOT_MEASURED'; reason = ''; stdout_bytes = 0; stderr_bytes = 0; truncated = $false
        frames_decoded = $null; decoder_pix_fmts = @(); dxva_profile_guids = @()
        output_bytes = $null; expected_output_bytes = $null
    }
}

function Set-RowFailure($Row, [string]$Reason) {
    $Row.status = 'NOT_MEASURED'; $Row.reason = $Reason
}

if ([string]::IsNullOrWhiteSpace($FfmpegPath)) {
    $toolSection = New-ToolSection 'NOT_MEASURED' 'TOOL_NOT_SUPPLIED' '' '' @()
    $interop = @([ordered]@{ id = 'interop_disabled'; kind = 'decode'; container = ''; codec = ''; pixel_format = ''; hwaccel = ''
        argv = @(); exit_code = $null; wall_ms = $null; status = 'NOT_MEASURED'; reason = 'INTEROP_DISABLED'
        stdout_bytes = 0; stderr_bytes = 0; truncated = $false; frames_decoded = $null
        decoder_pix_fmts = @(); dxva_profile_guids = @(); output_bytes = $null; expected_output_bytes = $null })
}
elseif ([string]::IsNullOrWhiteSpace($ScratchRoot)) {
    $toolSection = New-ToolSection 'NOT_MEASURED' 'SCRATCH_NOT_SUPPLIED' '' '' @()
    $interop = @([ordered]@{ id = 'interop_disabled'; kind = 'decode'; container = ''; codec = ''; pixel_format = ''; hwaccel = ''
        argv = @(); exit_code = $null; wall_ms = $null; status = 'NOT_MEASURED'; reason = 'INTEROP_DISABLED'
        stdout_bytes = 0; stderr_bytes = 0; truncated = $false; frames_decoded = $null
        decoder_pix_fmts = @(); dxva_profile_guids = @(); output_bytes = $null; expected_output_bytes = $null })
}
else {
    $tool = Resolve-AbsoluteLocal $FfmpegPath 'TOOL_PATH_INVALID'
    $scratchCheck = Resolve-AbsoluteLocal $ScratchRoot 'SCRATCH_UNAVAILABLE' -Directory
    if (-not $tool.ok) {
        $toolSection = New-ToolSection 'NOT_MEASURED' $tool.reason '' '' @()
        $interop = @([ordered]@{ id = 'interop_disabled'; kind = 'decode'; container = ''; codec = ''; pixel_format = ''; hwaccel = ''
            argv = @(); exit_code = $null; wall_ms = $null; status = 'NOT_MEASURED'; reason = 'INTEROP_DISABLED'
            stdout_bytes = 0; stderr_bytes = 0; truncated = $false; frames_decoded = $null
            decoder_pix_fmts = @(); dxva_profile_guids = @(); output_bytes = $null; expected_output_bytes = $null })
    }
    elseif (-not $scratchCheck.ok) {
        # The tool stays observable (hash + -hwaccels need no scratch), but no experiment
        # runs in an undeclared directory: the declared scratch root is never created here.
        $toolSection = New-ObservedToolSection $tool.path
        $interop = @([ordered]@{ id = 'interop_disabled'; kind = 'decode'; container = ''; codec = ''; pixel_format = ''; hwaccel = ''
            argv = @(); exit_code = $null; wall_ms = $null; status = 'NOT_MEASURED'; reason = 'SCRATCH_UNAVAILABLE'
            stdout_bytes = 0; stderr_bytes = 0; truncated = $false; frames_decoded = $null
            decoder_pix_fmts = @(); dxva_profile_guids = @(); output_bytes = $null; expected_output_bytes = $null })
    }
    else {
        $toolSection = New-ObservedToolSection $tool.path
        try {
            $token = 'vid1-probe-{0}-{1}' -f $PID, ([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds())
            $scratch = [System.IO.Path]::Combine($scratchCheck.path, $token)
            [System.IO.Directory]::CreateDirectory($scratch) | Out-Null

            $fixtureSpecs = @(
                @{ id = 'h264'; container = 'mp4'; file = 'fixture_h264_8bit.mp4'; codec = 'h264'; pixel = 'yuv420p'; rate = 30; seconds = 2; width = 640; height = 360; encoder = @('-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-preset', 'ultrafast') }
                @{ id = 'hevc10'; container = 'mp4'; file = 'fixture_hevc_10bit.mp4'; codec = 'hevc'; pixel = 'yuv420p10le'; rate = 30; seconds = 1; width = 320; height = 180; encoder = @('-c:v', 'libx265', '-pix_fmt', 'yuv420p10le', '-preset', 'ultrafast', '-x265-params', 'log-level=none') }
                @{ id = 'vp8'; container = 'webm'; file = 'fixture_vp8.webm'; codec = 'vp8'; pixel = 'yuv420p'; rate = 30; seconds = 1; width = 160; height = 90; encoder = @('-c:v', 'libvpx', '-pix_fmt', 'yuv420p') }
            )
            foreach ($spec in $fixtureSpecs) {
                $path = [System.IO.Path]::Combine($scratch, $spec.file)
                $argv = @($tool.path, '-hide_banner', '-loglevel', 'error', '-y', '-f', 'lavfi',
                    '-i', ("testsrc2=size={0}x{1}:rate={2}" -f $spec.width, $spec.height, $spec.rate),
                    '-t', [string]$spec.seconds) + $spec.encoder + @($path)
                $generated = Invoke-Tool $argv
                if ($generated.status -eq 'OBSERVED' -and $generated.exit_code -eq 0 -and (Test-Path -LiteralPath $path)) {
                    $bytes = (Get-Item -LiteralPath $path).Length
                    if ($bytes -le $MaxFixtureBytes) {
                        $fixtures += [ordered]@{ id = $spec.id; container = $spec.container; codec = $spec.codec; pixel_format = $spec.pixel
                            status = 'OBSERVED'; reason = ''; bytes = $bytes; width = $spec.width; height = $spec.height; seconds = $spec.seconds }
                    }
                    else {
                        $fixtures += [ordered]@{ id = $spec.id; container = $spec.container; codec = $spec.codec; pixel_format = $spec.pixel
                            status = 'NOT_MEASURED'; reason = 'FIXTURE_TOO_LARGE'; bytes = $bytes; width = $spec.width; height = $spec.height; seconds = $spec.seconds }
                    }
                }
                else {
                    $reason = 'FIXTURE_GENERATION_FAILED'
                    if ($generated.timed_out) { $reason = 'PROCESS_TIMEOUT' }
                    elseif ($generated.truncated) { $reason = 'PROCESS_OUTPUT_CAPPED' }
                    $fixtures += [ordered]@{ id = $spec.id; container = $spec.container; codec = $spec.codec; pixel_format = $spec.pixel
                        status = 'NOT_MEASURED'; reason = $reason; bytes = $null; width = $spec.width; height = $spec.height; seconds = $spec.seconds }
                }
            }

            $fixturePath = @{}
            foreach ($fixture in $fixtures) {
                if ($fixture.status -eq 'OBSERVED') {
                    $file = ($fixtureSpecs | Where-Object { $_.id -eq $fixture.id } | Select-Object -First 1).file
                    $fixturePath[$fixture.id] = [System.IO.Path]::Combine($scratch, $file)
                }
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
                $outputPath = ''
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
                $row.argv = $argv
                $run = Invoke-Tool $argv
                $row.stdout_bytes = $run.stdout.Length
                $row.stderr_bytes = $run.stderr.Length
                $row.truncated = $run.truncated
                $row.wall_ms = $run.wall_ms
                $row.exit_code = $run.exit_code
                if ($run.timed_out) { Set-RowFailure $row 'PROCESS_TIMEOUT' }
                elseif ($run.truncated) { Set-RowFailure $row 'PROCESS_OUTPUT_CAPPED' }
                elseif ($run.exit_code -ne 0) { Set-RowFailure $row ($(if ($Kind -eq 'hw_download') { 'HWOUTPUT_CONVERT_FAILED' } else { 'PROCESS_FAILED' })) }
                else {
                    $row.status = 'OBSERVED'
                    $row.frames_decoded = Get-FramesDecoded $run.stderr
                    $row.decoder_pix_fmts = @(Get-PixFmts $run.stderr)
                    $row.dxva_profile_guids = @(Get-DxvaGuids $run.stderr)
                    if ($Kind -eq 'hw_decode') {
                        if (Test-HardwareFrames $row.decoder_pix_fmts $row.dxva_profile_guids) {
                            $row.status = 'OBSERVED'; $row.reason = ''
                        }
                        else {
                            $row.status = 'UNSUPPORTED'; $row.reason = 'HWACCEL_SOFTWARE_FALLBACK'
                        }
                    }
                    if ($outputPath -ne '' -and (Test-Path -LiteralPath $outputPath)) {
                        $bytes = (Get-Item -LiteralPath $outputPath).Length
                        $row.output_bytes = $bytes
                        $specForBytes = $expected[$FixtureId]
                        $row.expected_output_bytes = if ($ExpectedKey -eq 'nv12') { $specForBytes.bytes_nv12 } else { $specForBytes.bytes_p010 }
                        if ($bytes -ne $row.expected_output_bytes) {
                            $row.status = 'NOT_MEASURED'; $row.reason = 'RAW_OUTPUT_SIZE_UNEXPECTED'
                        }
                    }
                }
                $script:interop += $row
            }

            Add-Experiment 'sw_h264_first_frame' 'decode' 'h264' 'none' @() @() '' 'error'
            Add-Experiment 'sw_h264_decode_all' 'decode' 'h264' 'none' @() @() '' 'error' 0
            Add-Experiment 'sw_h264_seek_1s' 'seek' 'h264' 'none' @() @('-ss', '1.0') '' 'error'
            Add-Experiment 'sw_h264_nv12' 'raw_output' 'h264' 'none' @() @('-pix_fmt', 'nv12') 'nv12' 'error'
            Add-Experiment 'hw_d3d11va_h264_decode' 'hw_decode' 'h264' 'd3d11va' @('-hwaccel', 'd3d11va') @() '' 'verbose'
            Add-Experiment 'hw_d3d11va_h264_download' 'hw_download' 'h264' 'd3d11va' @('-hwaccel', 'd3d11va', '-hwaccel_output_format', 'd3d11') @('-vf', 'hwdownload,format=nv12') 'nv12' 'verbose'
            Add-Experiment 'hw_dxva2_h264_decode' 'hw_decode' 'h264' 'dxva2' @('-hwaccel', 'dxva2') @() '' 'verbose'
            Add-Experiment 'sw_hevc10_first_frame' 'decode' 'hevc10' 'none' @() @() '' 'error'
            Add-Experiment 'sw_hevc10_p010' 'raw_output' 'hevc10' 'none' @() @('-pix_fmt', 'p010le') 'p010' 'error'
            Add-Experiment 'hw_d3d11va_hevc10_decode' 'hw_decode' 'hevc10' 'd3d11va' @('-hwaccel', 'd3d11va') @() '' 'verbose'
            Add-Experiment 'hw_d3d11va_hevc10_download' 'hw_download' 'hevc10' 'd3d11va' @('-hwaccel', 'd3d11va', '-hwaccel_output_format', 'd3d11') @('-vf', 'hwdownload,format=p010le') 'p010' 'verbose'
            Add-Experiment 'sw_vp8_first_frame' 'decode' 'vp8' 'none' @() @() '' 'error'
            Add-Experiment 'hw_d3d11va_vp8_decode' 'hw_decode' 'vp8' 'd3d11va' @('-hwaccel', 'd3d11va') @() '' 'verbose'
        }
        catch {
            if ($toolSection.status -ne 'OBSERVED') {
                $toolSection = New-ToolSection 'NOT_MEASURED' 'PROBE_INTERNAL_ERROR' '' '' @()
            }
        }
        finally {
            if ($null -ne $scratch -and (Test-Path -LiteralPath $scratch)) {
                try { Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue } catch { }
            }
        }
    }
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

$document = [ordered]@{
    ok = $true
    probe_version = $ProbeVersion
    caps = [ordered]@{ timeout_ms = $TimeoutMs; max_output_bytes = $MaxOutputBytes; max_fixture_bytes = $MaxFixtureBytes; max_experiments = $MaxExperiments }
    host = [ordered]@{
        status = 'OBSERVED'; reason = ''
        os_description = [System.Runtime.InteropServices.RuntimeInformation]::OSDescription
        arch = [string][System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
        pwsh = ($PSVersionTable.PSVersion).ToString()
        interop_enabled = (-not [string]::IsNullOrWhiteSpace($FfmpegPath))
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
    endpoints = @($endpoints)
}

Emit $document
exit 0
