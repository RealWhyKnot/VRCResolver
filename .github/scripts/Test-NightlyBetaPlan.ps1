#!/usr/bin/env pwsh
#Requires -Version 7.4
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-TestGit {
    [CmdletBinding()]
    [OutputType([object[]])]

    param(
        [string] $RepoRoot,
        [string[]] $Arguments
    )

    Push-Location $RepoRoot
    try {
        $output = & git @Arguments
        if ($LASTEXITCODE -ne 0) {
            throw "git $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
        }
        return @($output)
    } finally {
        Pop-Location
    }
}

function Write-TestFile {
    [CmdletBinding()]
    [OutputType([void])]

    param(
        [string] $Path,
        [string] $Content
    )

    $parent = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $parent)) {
        $null = New-Item -ItemType Directory -Path $parent
    }
    $encoding = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $Content, $encoding)
}

function New-TestRepo {
    [CmdletBinding()]
    [OutputType([string])]
    param()

    $name = 'vrcresolver-nightly-beta-' + [System.Guid]::NewGuid().ToString('N')
    $root = Join-Path -Path ([System.IO.Path]::GetTempPath()) -ChildPath $name
    $null = New-Item -ItemType Directory -Path $root

    $null = Invoke-TestGit -RepoRoot $root -Arguments @('init', '-q', '.')
    $null = Invoke-TestGit -RepoRoot $root -Arguments @('config', 'user.name', 'vrcresolver Tests')
    $null = Invoke-TestGit -RepoRoot $root -Arguments @('config', 'user.email', 'vrcresolver-tests@example.invalid')
    $null = Invoke-TestGit -RepoRoot $root -Arguments @('config', 'core.autocrlf', 'false')

    Write-TestFile -Path (Join-Path $root 'src\package.txt') -Content "initial`n"
    $null = Invoke-TestGit -RepoRoot $root -Arguments @('add', '.')
    $null = Invoke-TestGit -RepoRoot $root -Arguments @('commit', '-q', '-m', 'initial')
    $null = Invoke-TestGit -RepoRoot $root -Arguments @('tag', 'v2026.6.1.0-beta')
    return $root
}

function Invoke-Plan {
    [CmdletBinding()]
    [OutputType([pscustomobject])]

    param(
        [string] $RepoRoot,
        [string] $BaseTag = 'v2026.6.1.0-beta',
        [string] $Tag = '',
        [string] $Today = '',
        [datetime] $NowUtc = ([datetime]::Parse('2026-06-16T01:30:00Z'))
    )

    $outputPath = Join-Path $RepoRoot 'plan.json'
    $arguments = @{
        RepoRoot       = $RepoRoot
        BaseTag        = $BaseTag
        OutputJsonPath = $outputPath
        NowUtc         = $NowUtc
    }
    if (-not [string]::IsNullOrWhiteSpace($Tag)) {
        $arguments['Tag'] = $Tag
    }
    if (-not [string]::IsNullOrWhiteSpace($Today)) {
        $arguments['Today'] = $Today
    }

    & $Planner @arguments | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "Planner failed with exit code $LASTEXITCODE"
    }
    return ([IO.File]::ReadAllText($outputPath) | ConvertFrom-Json)
}

function Assert-Equal {
    [CmdletBinding()]
    [OutputType([void])]

    param(
        [object] $Actual,
        [object] $Expected,
        [string] $Message
    )

    if ($Actual -ne $Expected) {
        throw "$Message. Expected '$Expected', got '$Actual'."
    }
}

$ScriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$Planner = Join-Path $ScriptRoot 'Get-NightlyBetaPlan.ps1'

$tempRoots = New-Object System.Collections.Generic.List[string]
try {
    $repo = New-TestRepo
    $null = $tempRoots.Add($repo)
    $plan = Invoke-Plan -RepoRoot $repo
    $callOptions1 = @{
        Actual   = $plan.has_changes
        Expected = $false
        Message  = 'Latest tag at HEAD should skip nightly beta tag creation'
    }
    Assert-Equal @callOptions1
    $callOptions2 = @{
        Actual   = $plan.changed_path_count
        Expected = 0
        Message  = 'Changed path count should be zero at the latest tag'
    }
    Assert-Equal @callOptions2

    $repo = New-TestRepo
    $null = $tempRoots.Add($repo)
    Write-TestFile -Path (Join-Path $repo 'CHANGELOG.md') -Content "# Changelog`n"
    $null = Invoke-TestGit -RepoRoot $repo -Arguments @('add', '.')
    $null = Invoke-TestGit -RepoRoot $repo -Arguments @('commit', '-q', '-m', 'docs(changelog): promote release')
    $plan = Invoke-Plan -RepoRoot $repo
    $callOptions3 = @{
        Actual   = $plan.has_changes
        Expected = $false
        Message  = 'Changelog-only changes should skip nightly beta tag creation'
    }
    Assert-Equal @callOptions3
    $callOptions4 = @{
        Actual   = $plan.changed_path_count
        Expected = 0
        Message  = 'Changelog-only changes should not count as release inputs'
    }
    Assert-Equal @callOptions4

    $repo = New-TestRepo
    $null = $tempRoots.Add($repo)
    Write-TestFile -Path (Join-Path $repo 'data\wrapper_hashes.txt') -Content "abc123`n"
    $null = Invoke-TestGit -RepoRoot $repo -Arguments @('add', '.')
    $callOptions5 = @{
        RepoRoot  = $repo
        Arguments = @('commit', '-q', '-m', 'chore(release): append wrapper hash [skip changelog]')
    }
    $null = Invoke-TestGit @callOptions5
    $plan = Invoke-Plan -RepoRoot $repo
    $callOptions6 = @{
        Actual   = $plan.has_changes
        Expected = $false
        Message  = 'Wrapper-hash-only changes should skip nightly beta tag creation'
    }
    Assert-Equal @callOptions6
    $callOptions7 = @{
        Actual   = $plan.changed_path_count
        Expected = 0
        Message  = 'Wrapper-hash-only changes should not count as release inputs'
    }
    Assert-Equal @callOptions7

    $repo = New-TestRepo
    $null = $tempRoots.Add($repo)
    Write-TestFile -Path (Join-Path $repo 'src\package.txt') -Content "changed`n"
    $null = Invoke-TestGit -RepoRoot $repo -Arguments @('add', '.')
    $null = Invoke-TestGit -RepoRoot $repo -Arguments @('commit', '-q', '-m', 'fix: update proxy input')
    $plan = Invoke-Plan -RepoRoot $repo -Today '2026.6.9'
    $callOptions8 = @{
        Actual   = $plan.has_changes
        Expected = $true
        Message  = 'Changes since the latest release should create a plan'
    }
    Assert-Equal @callOptions8
    $callOptions9 = @{
        Actual   = $plan.changed_path_count
        Expected = 1
        Message  = 'Planner should count release input path changes'
    }
    Assert-Equal @callOptions9
    $callOptions10 = @{
        Actual   = $plan.next_tag
        Expected = 'v2026.6.9.0-beta'
        Message  = 'First same-day beta tag should use sequence zero'
    }
    Assert-Equal @callOptions10

    $null = Invoke-TestGit -RepoRoot $repo -Arguments @('tag', 'v2026.6.9.0-beta')
    $null = Invoke-TestGit -RepoRoot $repo -Arguments @('tag', 'v2026.6.9.1')
    $plan = Invoke-Plan -RepoRoot $repo -Today '2026.6.9'
    $callOptions11 = @{
        Actual   = $plan.next_tag
        Expected = 'v2026.6.9.2-beta'
        Message  = 'Same-day tags should increment from the highest release revision'
    }
    Assert-Equal @callOptions11

    $repo = New-TestRepo
    $null = $tempRoots.Add($repo)
    Write-TestFile -Path (Join-Path $repo 'src\package.txt') -Content "changed after utc rollover`n"
    $null = Invoke-TestGit -RepoRoot $repo -Arguments @('add', '.')
    $null = Invoke-TestGit -RepoRoot $repo -Arguments @('commit', '-q', '-m', 'fix: update after utc rollover')
    $plan = Invoke-Plan -RepoRoot $repo
    $callOptions12 = @{
        Actual   = $plan.next_tag
        Expected = 'v2026.6.15.0-beta'
        Message  = 'Default beta tag date should use Central release date'
    }
    Assert-Equal @callOptions12

    $failed = $false
    try {
        $null = Invoke-Plan -RepoRoot $repo -Tag 'v2026.6.9.0-beta.1'
    } catch [System.Management.Automation.RuntimeException] {
        $failed = $true
    }
    Assert-Equal -Actual $failed -Expected $true -Message 'Planner should reject numbered beta suffixes'

    Write-Host 'Nightly beta planner tests passed.'
} finally {
    foreach ($tempRoot in $tempRoots) {
        if (Test-Path -LiteralPath $tempRoot) {
            Remove-Item -LiteralPath $tempRoot -Recurse -Force
        }
    }
}

exit 0
