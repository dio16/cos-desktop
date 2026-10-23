<#
.SYNOPSIS
    Comparison-harness fixture: writes a deliberately malformed JPEG scan header.

.DESCRIPTION
    This script is not a decoder and must not be used as one. It exists so a real
    operator run can prove, on the record itself, that a candidate output whose SOS
    segment is malformed is never treated as a complete image: the harness has to
    record `error` / `output_incomplete` with `validity: not_measured` and must not
    produce a capability row from it.

    The bytes are a valid SOI + SOF0 (16x16, 3 components) followed by the malformed
    scan header from the review, `FF DA 00 03 01 FF D9`, whose length field cannot
    carry the required SOS fields.

    It answers the harness identity probe with -Version and otherwise writes the
    fixture bytes to -OutputPath.

.PARAMETER InputPath
    The case input path; accepted and ignored.

.PARAMETER OutputPath
    Absolute path the fixture bytes are written to.

.PARAMETER MaxPixels
    Accepted and ignored, so the harness argv stays identical for every transport.

.PARAMETER Version
    Identity probe; prints one line and exits 0.
#>
[CmdletBinding()]
param(
    [string] $InputPath = '',
    [string] $OutputPath = '',
    [int] $MaxPixels = 0,
    [switch] $Version
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($Version) {
    Write-Output 'decode-compare-fixture-probe 1.0 (writes a malformed JPEG SOS)'
    exit 0
}

if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    exit 2
}

# SOI, SOF0 (16x16, 3 components), then the malformed SOS: FF DA 00 03 01 FF D9.
$fixture = [byte[]](
    0xFF, 0xD8,
    0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x10, 0x00, 0x10, 0x03,
    0x01, 0x11, 0x00, 0x02, 0x11, 0x00, 0x03, 0x11, 0x00,
    0xFF, 0xDA, 0x00, 0x03, 0x01, 0xFF, 0xD9
)

[System.IO.File]::WriteAllBytes($OutputPath, $fixture)
exit 0
