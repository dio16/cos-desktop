# Runs the TESTING.md quality gates 1-5 plus the dependency-rules check, in documented order.
# Windows-first, repository root resolved from this script's location. The native startup
# smoke (gate 6) stays a recorded manual gate on this workstation and is not simulated here.
#
# Usage:  pwsh scripts/quality.ps1             # required quality gates
#         pwsh scripts/quality.ps1 -CheckOnly  # local feedback; no linking or tests

[CmdletBinding()]
param([switch]$CheckOnly)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

function Invoke-Python {
    param([string[]]$Arguments)
    $python = if (Get-Command python -ErrorAction SilentlyContinue) { "python" } else { "py" }
    & $python @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "python step failed: $python $($Arguments -join ' ')"
    }
}

Push-Location $root
try {
    $gates = [ordered]@{
        "fmt"                              = { cargo fmt --all -- --check }
        "metadata"                         = { cargo metadata --locked --no-deps --format-version 1 | Out-Null }
        "dependency rules (self-test)"     = { Invoke-Python @("-B", "scripts/check_dependency_rules.py", "--self-test") }
        "dependency rules (workspace)"     = { Invoke-Python @("-B", "scripts/check_dependency_rules.py") }
    }
    if ($CheckOnly) {
        $gates["check (no link)"] = { cargo check --locked --workspace --all-targets }
    }
    else {
        $gates["clippy"] = { cargo clippy --locked --workspace --all-targets -- -D warnings }
        $gates["tests"] = { cargo test --locked --workspace }
        $gates["build"] = { cargo build --locked --workspace }
    }

    foreach ($name in $gates.Keys) {
        Write-Host "==> $name" -ForegroundColor Cyan
        & $gates[$name]
        if ($LASTEXITCODE -ne 0) {
            throw "gate failed: $name"
        }
    }
    if ($CheckOnly) {
        Write-Host "local checks passed; clippy, tests, build and native smoke NOT_RUN" -ForegroundColor Yellow
    }
    else {
        Write-Host "all quality gates passed (native smoke is separate)" -ForegroundColor Green
    }
}
catch {
    Write-Host "quality gate failure: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
finally {
    Pop-Location
}
