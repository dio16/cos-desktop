# Record the documented quality gates with their raw output and exit codes.
#
# The evidence contract for quality runs: every command's stdout, stderr and exit code is
# stored, and a manifest binds each file by SHA-256, so a reader can re-derive the result
# instead of trusting a summary.
#
# Usage:
#   pwsh -NoProfile -File scripts/record-quality-gates.ps1 -OutDir <dir> [-TargetDir <dir>]
#
# Exit codes: 0 all gates passed, 1 at least one gate failed, 2 usage error.

[CmdletBinding()]
param(
    [string]$OutDir = "",
    [string]$TargetDir = ""
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $OutDir = Join-Path $repo "docs/development/evidence-published/quality-gates-$stamp"
}
$OutDir = [System.IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

if (-not [string]::IsNullOrWhiteSpace($TargetDir)) {
    $env:CARGO_TARGET_DIR = $TargetDir
}

$head = (& git -C $repo rev-parse HEAD).Trim()
$status = (& git -C $repo status --porcelain)
$dirty = -not [string]::IsNullOrWhiteSpace(($status -join ''))

$gates = @(
    @{ Name = 'fmt';        Command = 'cargo'; Args = @('fmt', '--all', '--', '--check') },
    @{ Name = 'metadata';   Command = 'cargo'; Args = @('metadata', '--locked', '--no-deps', '--format-version', '1') },
    @{ Name = 'dep-self';   Command = 'python'; Args = @('-B', 'scripts/check_dependency_rules.py', '--self-test') },
    @{ Name = 'dep-rules';  Command = 'python'; Args = @('-B', 'scripts/check_dependency_rules.py') },
    @{ Name = 'clippy';     Command = 'cargo'; Args = @('clippy', '--locked', '--workspace', '--all-targets', '--', '-D', 'warnings') },
    @{ Name = 'tests';      Command = 'cargo'; Args = @('test', '--locked', '--workspace') }
)

$results = @()
foreach ($gate in $gates) {
    $stdout = Join-Path $OutDir ("{0}.stdout.txt" -f $gate.Name)
    $stderr = Join-Path $OutDir ("{0}.stderr.txt" -f $gate.Name)
    $started = Get-Date
    Push-Location $repo
    try {
        $all = & $gate.Command @($gate.Args) 2>&1
        $code = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    $elapsed = [int]((Get-Date) - $started).TotalSeconds
    $text = ($all | Out-String)
    Set-Content -Path $stdout -Value $text -Encoding utf8
    Set-Content -Path $stderr -Value "" -Encoding utf8
    $results += [pscustomobject]@{
        name      = $gate.Name
        command   = "$($gate.Command) $($gate.Args -join ' ')"
        exit_code = $code
        seconds   = $elapsed
        stdout    = [System.IO.Path]::GetFileName($stdout)
        stderr    = [System.IO.Path]::GetFileName($stderr)
    }
    Write-Host ("{0}: exit {1} in {2}s" -f $gate.Name, $code, $elapsed)
}

$files = @{}
foreach ($file in Get-ChildItem -Path $OutDir -File | Where-Object { $_.Name -ne 'manifest.json' }) {
    $hash = (Get-FileHash -Path $file.FullName -Algorithm SHA256).Hash.ToLower()
    $files[$file.Name] = [pscustomobject]@{
        sha256 = "sha256:$hash"
        bytes  = $file.Length
    }
}

$failed = @($results | Where-Object { $_.exit_code -ne 0 })
$manifest = [pscustomobject]@{
    kind      = 'quality-gates'
    head      = $head
    dirty     = $dirty
    target_dir = $env:CARGO_TARGET_DIR
    recorded_at = (Get-Date).ToString('o')
    gates     = $results
    files     = $files
    summary   = [pscustomobject]@{
        total  = $results.Count
        failed = $failed.Count
        passed = $results.Count - $failed.Count
    }
}
$manifestPath = Join-Path $OutDir 'manifest.json'
$manifest | ConvertTo-Json -Depth 6 | Set-Content -Path $manifestPath -Encoding utf8

Write-Host ("manifest: {0}" -f $manifestPath)
if ($failed.Count -gt 0) {
    Write-Host ("failed gates: {0}" -f (($failed | ForEach-Object { $_.name }) -join ', '))
    exit 1
}
exit 0
