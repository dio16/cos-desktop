#!/usr/bin/env pwsh
# Regression for the pure WIC admission arithmetic. No WIC, no file, no network.
Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot '..\examples\decode_compare\wic_admission.ps1')

$failures = [System.Collections.Generic.List[string]]::new()
function Check {
    param([string] $Name, [scriptblock] $Condition)
    if (-not (& $Condition)) { $failures.Add($Name) }
}

# Presence is explicit binding, not a numeric sentinel: an explicit zero still counts.
Check 'explicit_max_dimension_is_present' { Get-BudgetPresence -BoundNames @('MaxDimension') }
Check 'explicit_zero_still_present' { Get-BudgetPresence -BoundNames @('MaxDimension', 'InputPath') }
Check 'no_budget_flag_absent' { -not (Get-BudgetPresence -BoundNames @('InputPath', 'MaxPixels')) }
Check 'declared_scratch_alone_is_not_a_budget' { -not (Get-BudgetPresence -BoundNames @('DeclaredScratchBytes')) }

# Widest per-channel depth, not the minimum: Bgr565 is 5/6/5.
Check 'bgr565_depth_is_six' { (Get-DeclaredFormatSpec 'Bgr565').Depth -eq 6 }
Check 'bgr555_depth_is_five' { (Get-DeclaredFormatSpec 'Bgr555').Depth -eq 5 }
Check 'bgr565_bpp_is_sixteen' { (Get-DeclaredFormatSpec 'Bgr565').BitsPerPixel -eq 16 }
Check 'unknown_format_is_null' { $null -eq (Get-DeclaredFormatSpec 'NotAFormat') }

$rgb = Get-AdmissionArithmetic -Width 16 -Height 16 -BitsPerPixel 24 -DeclaredScratch 0
Check 'rgb_row_bytes' { $rgb.RowBytes -eq 48 }
Check 'rgb_decoded_bytes' { $rgb.Decoded -eq 768 }
Check 'rgb_bgra32_bytes' { $rgb.Bgra32 -eq 1024 }
Check 'rgb_scratch_default' { $rgb.Scratch -eq 1024 }

$packed = Get-AdmissionArithmetic -Width 1 -Height 16 -BitsPerPixel 1 -DeclaredScratch 0
Check 'packed_one_bit_row_is_one_byte' { $packed.RowBytes -eq 1 }
Check 'packed_decoded_is_per_row' { $packed.Decoded -eq 16 }

$indexed = Get-AdmissionArithmetic -Width 3 -Height 2 -BitsPerPixel 2 -DeclaredScratch 0
Check 'indexed_row_rounds_up' { $indexed.RowBytes -eq 1 }
Check 'indexed_decoded_two_rows' { $indexed.Decoded -eq 2 }

$scratch = Get-AdmissionArithmetic -Width 16 -Height 16 -BitsPerPixel 24 -DeclaredScratch 1
Check 'scratch_adds_declared' { $scratch.Scratch -eq 1025 }

$huge = Get-AdmissionArithmetic -Width 2147483647 -Height 2147483647 -BitsPerPixel 24 -DeclaredScratch 0
Check 'huge_decoded_reported_as_null' { $null -eq (ConvertTo-ReportedLong $huge.Decoded) }
Check 'small_decoded_reported_as_long' { (ConvertTo-ReportedLong $rgb.Decoded) -eq 768 }

$roi = Get-RoiSpec '1,2,3,4'
Check 'roi_parses_coordinates' { $roi.X -eq 1 -and $roi.Y -eq 2 -and $roi.Width -eq 3 -and $roi.Height -eq 4 }
Check 'roi_zero_origin_is_valid' { $null -ne (Get-RoiSpec '0,0,1,1') }
Check 'roi_zero_width_is_still_spec' { (Get-RoiSpec '0,0,0,1').Width -eq 0 }
Check 'roi_negative_is_rejected' { $null -eq (Get-RoiSpec '-1,0,1,1') }
Check 'roi_plus_is_rejected' { $null -eq (Get-RoiSpec '+1,0,1,1') }
Check 'roi_leading_zero_is_rejected' { $null -eq (Get-RoiSpec '01,0,1,1') }
Check 'roi_spaces_are_rejected' { $null -eq (Get-RoiSpec '1, 2,3,4') }
Check 'roi_three_fields_rejected' { $null -eq (Get-RoiSpec '1,2,3') }
Check 'roi_empty_is_rejected' { $null -eq (Get-RoiSpec '') }

$summary = [ordered]@{ checks = 28; failures = $failures }
[Console]::Out.WriteLine(($summary | ConvertTo-Json -Compress))
if ($failures.Count -gt 0) { exit 1 }
exit 0
