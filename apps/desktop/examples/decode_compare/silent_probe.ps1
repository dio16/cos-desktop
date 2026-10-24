#!/usr/bin/env pwsh
#Requires -Version 7.0
<#
.SYNOPSIS
    Comparison-harness fixture: a candidate that reports success and writes no output.

.DESCRIPTION
    This script is not a decoder and must not be used as one. It exists so a real operator
    run can prove, on the record itself, that a candidate which exits `0` but produces no
    file is never treated as a decode: the harness must record `error` with refusal
    `output_missing`, must not invent an output size, and must not contribute a capability
    or resource row. The bytes it would have written are the point — there are none.

    Transport mirrors the script candidates (argv is always passed separately, never through
    a command string):

        -Version                            identity probe: one line on stdout, exit 0
        -InputPath <abs> [-OutputPath <abs>] [-MaxPixels N]
                                            accepted and ignored; no file is written

    It performs no decode, no I/O, no shell, no network and no GPU operation.

.PARAMETER InputPath
    Accepted and ignored, so the harness argv stays identical for every script transport.

.PARAMETER OutputPath
    Accepted and ignored: this fixture deliberately writes nothing.

.PARAMETER MaxPixels
    Accepted and ignored.

.PARAMETER Version
    Identity probe; prints one line and exits 0.
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string] $InputPath = '',

    [string] $OutputPath = '',

    [long] $MaxPixels = 0,

    [switch] $Version
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'

if ($Version) {
    [Console]::Out.WriteLine('decode-compare-silent-probe 1.0 (writes no output)')
    [Console]::Out.Flush()
    exit 0
}

# Deliberately nothing: no decode and no file, even when -OutputPath was supplied.
exit 0
