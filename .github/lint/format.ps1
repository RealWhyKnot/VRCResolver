#!/usr/bin/env pwsh
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Native {
    [CmdletBinding()]
    [OutputType([void])]

    param([string]$Name, [scriptblock]$Command)
    Write-Host "== $Name"
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$Name failed with exit code $LASTEXITCODE" }
}
$RepoRoot = (Resolve-Path -LiteralPath (Join-Path -Path $PSScriptRoot -ChildPath '../..')).Path
Set-Location -LiteralPath $RepoRoot

& git config --local core.hooksPath .githooks
if ($LASTEXITCODE -ne 0) { throw "git config failed with exit code $LASTEXITCODE" }

Invoke-Native -Name 'cargo fmt' -Command { cargo fmt --all }

if (Get-Module -ListAvailable -Name PSScriptAnalyzer) {
    Import-Module -Name PSScriptAnalyzer
    $files = @(& git ls-files '*.ps1' '*.psm1' '*.psd1')
    if ($LASTEXITCODE -ne 0) { throw "git ls-files failed with exit code $LASTEXITCODE" }
    $settingsPath = Join-Path -Path $PSScriptRoot -ChildPath 'PSScriptAnalyzerSettings.psd1'
    foreach ($file in $files) {
        $text = [System.IO.File]::ReadAllText((Join-Path $RepoRoot $file)) -replace "`r`n?", "`n"
        $formatted = Invoke-Formatter -ScriptDefinition $text -Settings $settingsPath
        if ($formatted -ne $text) {
            [System.IO.File]::WriteAllText(
                (Join-Path -Path $RepoRoot -ChildPath $file),
                $formatted,
                [System.Text.UTF8Encoding]::new($false)
            )
            Write-Host "Formatted $file"
        }
    }
} else {
    Write-Warning 'PSScriptAnalyzer is not installed; skipped PowerShell formatting.'
}

exit 0
