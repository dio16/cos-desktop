# Record the documented quality gates with their raw output and exit codes.
#
# Evidence contract: each command's **stdout and stderr are redirected to separate files by
# the OS**, its exit code is recorded, and a manifest binds every file by SHA-256, so a
# reader can re-derive the result instead of trusting a summary. Streams are not merged and
# not re-rendered through PowerShell formatting.
#
# Usage:
#   pwsh -NoProfile -File scripts/record-quality-gates.ps1 -OutDir <dir> [-TargetDir <dir>]
#   pwsh -NoProfile -File scripts/record-quality-gates.ps1 -SelfCheck
#
# Exit codes: 0 all gates passed, 1 at least one gate failed, 2 usage error.

[CmdletBinding()]
param(
    [string]$OutDir = "",
    [string]$TargetDir = "",
    [switch]$SelfCheck
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not [string]::IsNullOrWhiteSpace($TargetDir)) {
    $env:CARGO_TARGET_DIR = $TargetDir
}

function Invoke-Recorded {
    <#
      Run one command with its streams redirected to real files by the OS.
      Returns the exit code; nothing is merged, reformatted or swallowed.
    #>
    param(
        [string]$FilePath,
        [string[]]$ArgumentList,
        [string]$StdoutPath,
        [string]$StderrPath
    )
    $process = Start-Process -FilePath $FilePath `
        -ArgumentList $ArgumentList `
        -WorkingDirectory $repo `
        -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $StdoutPath `
        -RedirectStandardError $StderrPath
    return $process.ExitCode
}

if ($SelfCheck) {
    # Prove the recorder distinguishes pass from fail: one command that succeeds and one that
    # exits non-zero, recorded into a throwaway directory.
    $scratch = Join-Path ([System.IO.Path]::GetTempPath()) ("gate-selfcheck-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $scratch -Force | Out-Null
    $okShell = $IsWindows ? 'cmd.exe' : 'sh'
    $okArgs = $IsWindows ? @('/c', 'echo selfcheck-pass') : @('-c', 'echo selfcheck-pass')
    $badArgs = $IsWindows ? @('/c', 'echo selfcheck-fail 1>&2 & exit 3') : @('-c', 'echo selfcheck-fail 1>&2; exit 3')
    $first = Invoke-Recorded -FilePath $okShell -ArgumentList $okArgs `
        -StdoutPath (Join-Path $scratch 'pass.stdout.txt') -StderrPath (Join-Path $scratch 'pass.stderr.txt')
    $second = Invoke-Recorded -FilePath $okShell -ArgumentList $badArgs `
        -StdoutPath (Join-Path $scratch 'fail.stdout.txt') -StderrPath (Join-Path $scratch 'fail.stderr.txt')
    $failOut = Get-Content -Path (Join-Path $scratch 'fail.stderr.txt') -Raw
    Write-Host ("pass command exit: {0}" -f $first)
    Write-Host ("fail command exit: {0}" -f $second)
    Write-Host ("fail stderr contained: {0}" -f $failOut.Trim())
    $good = ($first -eq 0) -and ($second -ne 0) -and ($failOut -match 'selfcheck-fail')
    Remove-Item -Recurse -Force $scratch
    if ($good) { Write-Host 'self-check: OK'; exit 0 }
    Write-Host 'self-check: FAILED'
    exit 1
}

if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $OutDir = Join-Path $repo "docs/development/evidence-published/quality-gates-$stamp"
}
$OutDir = [System.IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

$head = (& git -C $repo rev-parse HEAD).Trim()
$status = (& git -C $repo status --porcelain)
$dirty = -not [string]::IsNullOrWhiteSpace(($status -join ''))

$gates = @(
    @{ Name = 'fmt';       File = 'cargo';  Args = @('fmt', '--all', '--', '--check') },
    @{ Name = 'metadata';  File = 'cargo';  Args = @('metadata', '--locked', '--no-deps', '--format-version', '1') },
    @{ Name = 'dep-self';  File = 'python'; Args = @('-B', 'scripts/check_dependency_rules.py', '--self-test') },
    @{ Name = 'dep-rules'; File = 'python'; Args = @('-B', 'scripts/check_dependency_rules.py') },
    @{ Name = 'clippy';    File = 'cargo';  Args = @('clippy', '--locked', '--workspace', '--all-targets', '--', '-D', 'warnings') },
    @{ Name = 'tests';     File = 'cargo';  Args = @('test', '--locked', '--workspace') }
)

$results = @()
foreach ($gate in $gates) {
    $stdout = Join-Path $OutDir ("{0}.stdout.txt" -f $gate.Name)
    $stderr = Join-Path $OutDir ("{0}.stderr.txt" -f $gate.Name)
    $started = Get-Date
    $code = Invoke-Recorded -FilePath $gate.File -ArgumentList $gate.Args -StdoutPath $stdout -StderrPath $stderr
    $elapsed = [int]((Get-Date) - $started).TotalSeconds
    $outBytes = (Get-Item $stdout).Length
    $errBytes = (Get-Item $stderr).Length
    $results += [pscustomobject]@{
        name        = $gate.Name
        command     = "$($gate.File) $($gate.Args -join ' ')"
        exit_code   = $code
        seconds     = $elapsed
        stdout      = [System.IO.Path]::GetFileName($stdout)
        stderr      = [System.IO.Path]::GetFileName($stderr)
        stdout_bytes = $outBytes
        stderr_bytes = $errBytes
    }
    Write-Host ("{0}: exit {1} in {2}s (stdout {3} B, stderr {4} B)" -f $gate.Name, $code, $elapsed, $outBytes, $errBytes)
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
    kind        = 'quality-gates'
    head        = $head
    dirty       = $dirty
    target_dir  = $env:CARGO_TARGET_DIR
    recorded_at = (Get-Date).ToString('o')
    gates       = $results
    files       = $files
    summary     = [pscustomobject]@{
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
