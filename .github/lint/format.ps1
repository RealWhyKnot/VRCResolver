#!/usr/bin/env pwsh
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
Set-Location $RepoRoot

function Invoke-Native {
    param([string]$Name, [scriptblock]$Command)
    Write-Host "== $Name"
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$Name failed with exit code $LASTEXITCODE" }
}

& git config --local core.hooksPath .githooks

Invoke-Native 'cargo fmt' { cargo fmt --all }

if (Get-Module -ListAvailable -Name PSScriptAnalyzer) {
    Import-Module PSScriptAnalyzer
    $files = & git ls-files '*.ps1' '*.psm1' '*.psd1'
    foreach ($file in $files) {
        $text = [System.IO.File]::ReadAllText((Join-Path $RepoRoot $file)) -replace "`r`n?", "`n"
        $formatted = Invoke-Formatter -ScriptDefinition $text
        if ($formatted -ne $text) {
            [System.IO.File]::WriteAllText((Join-Path $RepoRoot $file), $formatted, [System.Text.UTF8Encoding]::new($false))
            Write-Host "Formatted $file"
        }
    }
} else {
    Write-Warning 'PSScriptAnalyzer is not installed; skipped PowerShell formatting.'
}
