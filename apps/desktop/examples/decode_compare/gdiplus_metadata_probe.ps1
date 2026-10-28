#!/usr/bin/env pwsh
#Requires -Version 7.0
<#
.SYNOPSIS
    Development-only W10 GDI+ JPEG metadata and real-pixel probe.
.DESCRIPTION
    Loads with managed System.Drawing, materializes a Bitmap, accesses a real
    pixel, optionally records bounded EXIF orientation and ICC profile data, and
    optionally writes a fresh PNG/BMP output. GDI+ allocates the first decoded
    surface inside Image.FromStream before geometry or MaxPixels can be checked.
    This limitation is intrinsic and MaxPixels only prevents later work.
    Local drive-rooted paths only; no UNC, device, traversal, or path expansion.
#>
[CmdletBinding()]
param(
    [Parameter(Position=0)][string] $InputPath = '',
    [string] $OutputPath = '',
    [long] $MaxPixels = 0,
    [switch] $Metadata,
    [long] $MaxMetadataBytes = 0
)
Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'
function Test-LocalPath([string] $Path) {
    if ([string]::IsNullOrWhiteSpace($Path) -or $Path -notmatch '^[A-Za-z]:[\\/]') { return $false }
    foreach ($part in ($Path -split '[\\/]')) { if ($part -eq '..') { return $false } }
    return $true
}
function Emit($r, [int] $code) {
    [Console]::Out.WriteLine(($r | ConvertTo-Json -Compress -Depth 3))
    [Console]::Out.Flush()
    exit $code
}
$r = [ordered]@{ outcome='error'; probe='w10-system-drawing-metadata-v1'; input_bytes=$null; width=$null; height=$null; output_bytes=$null; orientation_present=$null; orientation_raw=$null; icc_present=$null; icc_readable=$null; icc_length=$null; icc_sha256=$null; metadata_requested=[bool]$Metadata; metadata_error=$null; reason=$null }
if (-not (Test-LocalPath $InputPath)) { $r.reason='input_not_local'; Emit $r 1 }
if ($OutputPath -and -not (Test-LocalPath $OutputPath)) { $r.reason='output_not_local'; Emit $r 1 }
if ($Metadata -and $MaxMetadataBytes -le 0) { $r.reason='metadata_bound_required'; Emit $r 1 }
$inputFull = [IO.Path]::GetFullPath($InputPath)
if (-not [IO.File]::Exists($inputFull)) { $r.reason='input_missing'; Emit $r 1 }
if ($OutputPath) {
    $outputFull = [IO.Path]::GetFullPath($OutputPath)
    if ([IO.File]::Exists($outputFull)) { $r.reason='output_exists'; Emit $r 1 }
    $extension = [IO.Path]::GetExtension($outputFull).ToLowerInvariant()
    if ($extension -notin @('.png','.bmp')) { $r.outcome='unsupported'; $r.reason='unsupported_output_extension'; Emit $r 2 }
}
$r.input_bytes = ([IO.FileInfo]::new($inputFull)).Length
$stream=$null; $image=$null; $frame=$null; $outStream=$null
try {
    Add-Type -AssemblyName System.Drawing
    $stream=[IO.File]::Open($inputFull,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
    $image=[Drawing.Image]::FromStream($stream)
    $r.width=[long]$image.Width; $r.height=[long]$image.Height
    if ($MaxPixels -gt 0 -and $r.width * $r.height -gt $MaxPixels) { $r.outcome='oversize'; $r.reason='max_pixels_exceeded'; Emit $r 4 }
    $frame=[Drawing.Bitmap]::new($image)
    $null=$frame.GetPixel(0,0)
    if ($Metadata) {
        $r.orientation_present=$false; $r.orientation_raw=$null
        $r.icc_present=$false; $r.icc_readable=$false; $r.icc_length=$null; $r.icc_sha256=$null
        try {
            $ids=@($image.PropertyIdList)
            if ($ids -contains 0x0112) {
                $item=$image.GetPropertyItem(0x0112)
                if ($null -eq $item -or $item.Type -ne 3 -or $item.Len -ne 2 -or $item.Value.Length -ne 2) { throw 'orientation_property_invalid' }
                $r.orientation_present=$true
                $r.orientation_raw=[int][BitConverter]::ToUInt16($item.Value,0)
            }
            if ($ids -contains 0x8773) {
                $item=$image.GetPropertyItem(0x8773)
                if ($null -eq $item -or $item.Type -notin @(1,7) -or $item.Len -lt 1) { throw 'icc_property_invalid' }
                $r.icc_present=$true
                if ($item.Len -gt $MaxMetadataBytes) { $r.reason='metadata_cap_exceeded'; $r.outcome='oversize'; Emit $r 4 }
                $bytes=[byte[]]$item.Value
                if ($bytes.Length -ne $item.Len) { throw 'icc_length_mismatch' }
                $sha=[Security.Cryptography.SHA256]::Create()
                try { $r.icc_sha256='sha256:'+[BitConverter]::ToString($sha.ComputeHash($bytes)).Replace('-','').ToLowerInvariant() } finally { $sha.Dispose() }
                $r.icc_length=[long]$bytes.Length; $r.icc_readable=$true
            }
        } catch {
            $r.metadata_error=$_.Exception.GetType().Name + ':' + $_.Exception.Message
            # Do not report failed metadata enumeration as verified absence.
            $r.orientation_present=$null
            $r.orientation_raw=$null
            $r.icc_present=$null
            $r.icc_readable=$false
            $r.icc_length=$null
            $r.icc_sha256=$null
        }
    }
    if ($OutputPath) {
        $format=if ($extension -eq '.png') { [Drawing.Imaging.ImageFormat]::Png } else { [Drawing.Imaging.ImageFormat]::Bmp }
        $outStream=[IO.File]::Open($outputFull,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
        $frame.Save($outStream,$format)
        $outStream.Flush($true)
        $outStream.Dispose()
        $outStream=$null
        $r.output_bytes=([IO.FileInfo]::new($outputFull)).Length
    }
    $r.outcome='decoded'; Emit $r 0
} catch {
    $r.reason=$_.Exception.GetType().Name
    Emit $r 1
} finally {
    if ($null -ne $outStream) { $outStream.Dispose() }
    if ($null -ne $frame) { $frame.Dispose() }
    if ($null -ne $image) { $image.Dispose() }
    if ($null -ne $stream) { $stream.Dispose() }
}
