#!/usr/bin/env pwsh
#Requires -Version 7.4

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Git {
    [CmdletBinding()]
    [OutputType([object[]])]

    param([string[]] $Arguments)
    & git @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "git $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

function Write-TestFile {
    [CmdletBinding()]
    [OutputType([void])]

    param(
        [string]$Path,
        [string]$Content
    )

    $FullPath = Join-Path (Get-Location) $Path
    $Dir = Split-Path -Parent $FullPath
    if ($Dir -and -not (Test-Path -LiteralPath $Dir)) {
        $null = New-Item -ItemType Directory -Force -Path $Dir
    }
    [System.IO.File]::WriteAllText($FullPath, $Content, $Utf8NoBom)
}

function Save-TestChange {
    [CmdletBinding()]
    [OutputType([void])]

    param(
        [string]$Path,
        [string]$Content,
        [string]$Subject
    )

    Write-TestFile -Path $Path -Content $Content
    Invoke-Git -Arguments @('add', '--', $Path)
    Invoke-Git -Arguments @('commit', '-q', '-m', $Subject)
}

function Assert-IncludedText {
    [CmdletBinding()]
    [OutputType([void])]

    param(
        [string]$Text,
        [string]$Expected
    )

    if (-not $Text.Contains($Expected)) {
        throw "Expected release notes to contain '$Expected'."
    }
}

function Assert-ExcludedText {
    [CmdletBinding()]
    [OutputType([void])]

    param(
        [string]$Text,
        [string]$Unexpected
    )

    if ($Text.Contains($Unexpected)) {
        throw "Expected release notes not to contain '$Unexpected'."
    }
}

$ScriptRoot = Split-Path -Parent $PSCommandPath
$Generator = Join-Path $ScriptRoot 'Generate-ReleaseNotes.ps1'
$TempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('vrcresolver.ReleaseNotes.' + [Guid]::NewGuid().ToString('N'))
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$PushedLocation = $false

try {
    $null = New-Item -ItemType Directory -Force -Path $TempRoot
    Push-Location $TempRoot
    $PushedLocation = $true

    Invoke-Git -Arguments @('init', '-q')
    Invoke-Git -Arguments @('config', 'core.autocrlf', 'false')
    Invoke-Git -Arguments @('config', 'user.name', 'WhyKnot')
    Invoke-Git -Arguments @('config', 'user.email', 'whyknot@example.invalid')

    $null = New-Item -ItemType Directory -Force -Path '.github/release-template'
    foreach ($TemplateName in @('links', 'install', 'uninstall', 'what-you-need-to-do')) {
        Write-TestFile -Path ".github/release-template/$TemplateName.md" -Content ''
    }

    $callOptions1 = @{
        Path    = 'CHANGELOG.md'
        Content = "# Changelog`n`n## Unreleased`n`n_No notable changes since the last release._`n"
        Subject = 'chore: initial changelog'
    }
    Save-TestChange @callOptions1
    Invoke-Git -Arguments @('tag', '-a', 'v2026.5.1.0', '-m', 'vrcresolver 2026.5.1.0')

    $callOptions2 = @{
        Path    = 'src.txt'
        Content = "plain-prerelease`n"
        Subject = 'fix(helper): plain prerelease fallback'
    }
    Save-TestChange @callOptions2
    Invoke-Git -Arguments @('tag', '-a', 'v2026.5.2.0', '-m', 'vrcresolver 2026.5.2.0')

    $callOptions3 = @{
        Path    = 'src.txt'
        Content = "beta`n"
        Subject = 'fix(relay): beta port fallback'
    }
    Save-TestChange @callOptions3
    Invoke-Git -Arguments @('tag', '-a', 'v2026.5.2.1-beta', '-m', 'vrcresolver 2026.5.2.1-beta')

    $callOptions4 = @{
        Path    = 'src.txt'
        Content = "stable`n"
        Subject = 'fix(vrclog): stable startup tail'
    }
    Save-TestChange @callOptions4
    Invoke-Git -Arguments @('tag', '-a', 'v2026.5.3.0', '-m', 'vrcresolver 2026.5.3.0')

    $TemplateDir = Join-Path $TempRoot '.github/release-template'
    $callOptions5 = @{
        Tag            = 'v2026.5.3.0'
        Repo           = 'RealWhyKnot/VRCResolver'
        TemplateDir    = $TemplateDir
        PrereleaseTags = 'v2026.5.2.0'
        SkipScrub      = $true
    }
    $StableNotes = (& $Generator @callOptions5) -join "`n"

    Assert-IncludedText -Text $StableNotes -Expected 'fix(helper): plain prerelease fallback'
    Assert-IncludedText -Text $StableNotes -Expected 'fix(relay): beta port fallback'
    Assert-IncludedText -Text $StableNotes -Expected 'fix(vrclog): stable startup tail'
    Assert-IncludedText -Text $StableNotes -Expected 'compare/v2026.5.1.0...v2026.5.3.0'
    Assert-ExcludedText -Text $StableNotes -Unexpected 'compare/v2026.5.2.0...v2026.5.3.0'
    Assert-ExcludedText -Text $StableNotes -Unexpected 'compare/v2026.5.2.1-beta...v2026.5.3.0'

    $callOptions6 = @{
        Tag            = 'v2026.5.2.1-beta'
        Repo           = 'RealWhyKnot/VRCResolver'
        TemplateDir    = $TemplateDir
        PrereleaseTags = 'v2026.5.2.0'
        SkipScrub      = $true
    }
    $BetaNotes = (& $Generator @callOptions6) -join "`n"

    Assert-IncludedText -Text $BetaNotes -Expected 'fix(relay): beta port fallback'
    Assert-IncludedText -Text $BetaNotes -Expected 'compare/v2026.5.2.0...v2026.5.2.1-beta'
    Assert-ExcludedText -Text $BetaNotes -Unexpected 'fix(helper): plain prerelease fallback'
    Assert-ExcludedText -Text $BetaNotes -Unexpected 'fix(vrclog): stable startup tail'

    $callOptions7 = @{
        Tag            = 'v2026.5.2.0'
        Repo           = 'RealWhyKnot/VRCResolver'
        TemplateDir    = $TemplateDir
        PrereleaseTags = 'v2026.5.2.0'
        SkipScrub      = $true
    }
    $PlainPrereleaseNotes = (& $Generator @callOptions7) -join "`n"

    Assert-IncludedText -Text $PlainPrereleaseNotes -Expected 'fix(helper): plain prerelease fallback'
    Assert-IncludedText -Text $PlainPrereleaseNotes -Expected 'compare/v2026.5.1.0...v2026.5.2.0'
    Assert-ExcludedText -Text $PlainPrereleaseNotes -Unexpected 'fix(relay): beta port fallback'

    $null = New-Item -ItemType Directory -Force -Path 'artifacts'
    $zipPath = Join-Path $TempRoot 'artifacts/vrcresolver-v2026.5.3.0.zip'
    $manifestPath = Join-Path $TempRoot 'artifacts/vrcresolver-v2026.5.3.0.manifest.tsv'
    $zipSha = 'E8966F33BE8246922756E3E8234CF8309FB6D3151665594203F53BBF5725164B'
    Write-TestFile -Path 'artifacts/vrcresolver-v2026.5.3.0.zip' -Content 'zip'
    $callOptions8 = @{
        Path    = 'artifacts/vrcresolver-v2026.5.3.0.manifest.tsv'
        Content = "A24EA7D3DF2B0718AFF60B5B9EBEBDF590ED4938D81A6B08CDEC7A880B326B0C`t123`tvrcresolver.exe`n"
    }
    Write-TestFile @callOptions8

    $callOptions9 = @{
        Tag            = 'v2026.5.3.0'
        Repo           = 'RealWhyKnot/VRCResolver'
        TemplateDir    = $TemplateDir
        PrereleaseTags = 'v2026.5.2.0'
        ZipPath        = $zipPath
        ZipName        = 'vrcresolver-v2026.5.3.0.zip'
        ZipSize        = 157017922
        ZipSha256      = $zipSha
        Manifest       = $manifestPath
        SkipScrub      = $true
    }
    $IntegrityNotes = (& $Generator @callOptions9) -join "`n"

    Assert-IncludedText -Text $IntegrityNotes -Expected 'vrcresolver-v2026.5.3.0.integrity.tsv'
    Assert-ExcludedText -Text $IntegrityNotes -Unexpected "SHA256: $zipSha"
    Assert-ExcludedText -Text $IntegrityNotes -Unexpected 'vrcresolver.exe                        '

    Write-Host 'Generate-ReleaseNotes tests passed.'
} finally {
    if ($PushedLocation) {
        Pop-Location
    }
    if (Test-Path -LiteralPath $TempRoot) {
        Remove-Item -LiteralPath $TempRoot -Recurse -Force
    }
}

exit 0
