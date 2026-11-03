#!/usr/bin/env pwsh
#Requires -Version 7.4
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Gate {
    [CmdletBinding()]
    [OutputType([void])]
    param(
        [Parameter(Mandatory)][string] $Name,
        [Parameter(Mandatory)][scriptblock] $Command
    )
    Write-Host $Name
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$Name failed with exit code $LASTEXITCODE" }
}

$repoRoot = (Resolve-Path -LiteralPath (Join-Path -Path $PSScriptRoot -ChildPath '../..')).Path
Set-Location -LiteralPath $repoRoot

Invoke-Gate -Name 'Shared style checks' -Command { python -B .github/lint/style_check.py }
Invoke-Gate -Name 'PowerShell checks' -Command { pwsh -NoProfile -File .github/lint/Invoke-PSLint.ps1 }
Invoke-Gate -Name 'Rust format' -Command { cargo fmt --check }
Invoke-Gate -Name 'Rust lint' -Command { cargo clippy --workspace --all-targets --locked -- -D warnings }
Invoke-Gate -Name 'Rust tests' -Command { cargo test --workspace --all-targets --locked }

$rustFiles = @(git ls-files --cached --others --exclude-standard -- '*.rs')
if ($LASTEXITCODE -ne 0) {
    throw "git ls-files failed with exit code $LASTEXITCODE"
}
$existingRustFiles = @(
    foreach ($file in $rustFiles) {
        if (Test-Path -LiteralPath (Join-Path -Path $repoRoot -ChildPath $file) -PathType Leaf) {
            $file
        }
    }
)
Invoke-Gate -Name 'Rust style' -Command { awk -f .github/lint/rust-style.awk @existingRustFiles }
if ($env:OS -eq 'Windows_NT' -and
    (-not (Get-Command -Name shellcheck -ErrorAction SilentlyContinue) -or
    -not (Get-Command -Name shfmt -ErrorAction SilentlyContinue))) {
    Invoke-Gate -Name 'Shell checks' -Command {
        wsl -d agent --cd $repoRoot -- bash .github/lint/check-shell
    }
} else {
    Invoke-Gate -Name 'Shell checks' -Command { bash .github/lint/check-shell }
}
Invoke-Gate -Name 'Python format' -Command { ruff format --check . }
Invoke-Gate -Name 'Python lint' -Command { ruff check --ignore-noqa . }

exit 0
