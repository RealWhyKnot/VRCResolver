#!/usr/bin/env pwsh
#Requires -Version 7.4
[CmdletBinding()]
param(
    [string] $RepoRoot = (Get-Location).Path
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Find-GitBash {
    [CmdletBinding()]
    [OutputType([hashtable])]
    param()
    $git = Get-Command -Name git -CommandType Application -ErrorAction Stop | Select-Object -First 1
    $folder = Split-Path -Parent $git.Source
    while ($folder) {
        foreach ($relative in @('bin\bash.exe', 'usr\bin\bash.exe')) {
            $candidate = Join-Path $folder $relative
            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                return @{ Bash = $candidate; UsrBin = (Join-Path $folder 'usr\bin') }
            }
        }
        $folder = Split-Path -Parent $folder
    }
    throw "Could not find Git's bash.exe next to $($git.Source)."
}

function Assert-Tool {
    [CmdletBinding()]
    [OutputType([void])]
    param([string] $Name)
    if (-not (Get-Command -Name $Name -CommandType Application -ErrorAction SilentlyContinue)) {
        throw "$Name is missing. Install it and add it to PATH."
    }
}

$root = (Resolve-Path -LiteralPath $RepoRoot).Path
$script = Join-Path $root '.github/lint/check-shell'
if (-not (Test-Path -LiteralPath $script -PathType Leaf)) {
    $script = Join-Path $root '.githooks/check-shell'
}
if (-not (Test-Path -LiteralPath $script -PathType Leaf)) {
    throw "No check-shell script in $root. Run Install-Lint with -Lang shell."
}

$savedPath = $env:PATH
try {
    $bash = 'bash'
    if ($env:OS -eq 'Windows_NT') {
        $found = Find-GitBash
        $bash = $found.Bash
        $env:PATH = $found.UsrBin + [IO.Path]::PathSeparator + $env:PATH
    }
    Assert-Tool -Name shellcheck
    Assert-Tool -Name shfmt
    Push-Location -LiteralPath $root
    try {
        & $bash $script
        if ($LASTEXITCODE -ne 0) {
            exit $LASTEXITCODE
        }
    } finally {
        Pop-Location
    }
} finally {
    $env:PATH = $savedPath
}
exit 0
