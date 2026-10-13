#!/usr/bin/env pwsh
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location $RepoRoot

function Invoke-Native {
    param([string]$Name, [scriptblock]$Command)
    Write-Host "== $Name"
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$Name failed with exit code $LASTEXITCODE" }
}

& git config --local core.hooksPath .githooks

Write-Host '== PowerShell syntax'
& .\.github\scripts\Test-WorkflowSyntax.ps1

if (-not (Get-Module -ListAvailable -Name PSScriptAnalyzer)) {
    throw 'PSScriptAnalyzer is required. Install it with: Install-Module PSScriptAnalyzer -Scope CurrentUser'
}
Import-Module PSScriptAnalyzer
$psFiles = & git ls-files '*.ps1' '*.psm1' '*.psd1'
if ($psFiles) {
    Write-Host '== PSScriptAnalyzer'
    $findings = foreach ($file in $psFiles) {
        Invoke-ScriptAnalyzer -Path (Join-Path $RepoRoot $file) -Severity Error
    }
    if ($findings) {
        $findings | Format-Table -AutoSize
        throw "PSScriptAnalyzer reported $($findings.Count) error(s)."
    }
}

Invoke-Native 'cargo fmt check' { cargo fmt --all --check }
Invoke-Native 'cargo clippy' { cargo clippy --workspace --all-targets --locked -- -D warnings }
