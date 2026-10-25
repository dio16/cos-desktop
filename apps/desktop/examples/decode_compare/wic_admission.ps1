#!/usr/bin/env pwsh
# Pure, side-effect-free admission arithmetic for wic_probe.ps1. Dot-sourced by the
# probe and by wic_admission_selftest.ps1, so the fail-closed rules are testable
# without a WIC decode, a file write or a budget literal of their own.
Set-StrictMode -Version 3.0

# Declared format support is a closed mapping, not a guess. An unmodelled WIC format
# is refused before materialization rather than assumed to be 8-bit RGBA. Depth is
# the widest per-channel depth (Bgr565 is 5/6/5, so 6); BitsPerPixel is WIC's own
# declared storage width and drives the row byte requirement.
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
        'Bgr565' { return @{ Depth = 6; BitsPerPixel = 16 } }
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
# The decoded requirement is summed per row: a packed 1-bit row of width 1 occupies
# one byte, so 16 such rows require 16 bytes, not the two a whole-image bit total
# would suggest. WIC's own stride alignment and internal buffers are NOT_MEASURED
# and are deliberately not guessed here.
function Get-AdmissionArithmetic {
    param(
        [long] $Width,
        [long] $Height,
        [int] $BitsPerPixel,
        [long] $DeclaredScratch
    )

    $w = [System.Numerics.BigInteger] $Width
    $h = [System.Numerics.BigInteger] $Height
    $area = [System.Numerics.BigInteger]::Multiply($w, $h)
    $rowBits = [System.Numerics.BigInteger]::Multiply($w, [System.Numerics.BigInteger] $BitsPerPixel)
    $rowBytes = ([System.Numerics.BigInteger]::Add($rowBits, 7)) / 8
    $decoded = [System.Numerics.BigInteger]::Multiply($rowBytes, $h)
    $bgra32 = [System.Numerics.BigInteger]::Multiply($area, 4)
    $scratch = [System.Numerics.BigInteger]::Add($bgra32, [System.Numerics.BigInteger] $DeclaredScratch)
    return @{
        Area = $area
        RowBytes = $rowBytes
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

# Presence is the caller's *explicit* parameter binding, never a numeric sentinel:
# an explicit `-MaxDimension 0` must refuse, not fall back to the legacy path.
function Get-BudgetPresence {
    param([string[]] $BoundNames)

    $keys = 'MaxDimension', 'MaxBitDepth', 'MaxDecodedBytes', 'MaxScratchBytes'
    return @($keys | Where-Object { $BoundNames -contains $_ }).Count -gt 0
}
