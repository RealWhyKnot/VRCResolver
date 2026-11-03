#!/usr/bin/env pwsh
#Requires -Version 7.4
[CmdletBinding()]
param(
    [string] $RepoRoot = '',
    [string] $BaseTag = '',
    [string] $Tag = '',
    [string] $Today = '',
    [datetime] $NowUtc = ([datetime]::UtcNow),
    [string] $OutputJsonPath = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Resolve-RepoRoot {
    [CmdletBinding()]
    [OutputType([string])]

    param([string] $Value)

    if (-not [string]::IsNullOrWhiteSpace($Value)) {
        return (Resolve-Path -LiteralPath $Value).Path
    }
    return (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
}

function Invoke-Git {
    [CmdletBinding()]
    [OutputType([object[]])]

    param(
        [string[]] $Arguments,
        [switch] $AllowFailure
    )

    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $output = & git @Arguments 2>$null
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousPreference
    }

    if ($exitCode -ne 0) {
        if ($AllowFailure) {
            return $null
        }
        throw "git $($Arguments -join ' ') failed with exit code $exitCode"
    }
    return @($output)
}

function Get-FirstOutput {
    [CmdletBinding()]
    [OutputType([string])]

    param([object] $Value)

    $items = @($Value)
    if ($items.Count -eq 0) {
        return $null
    }
    return [string]$items[0]
}

function Test-GitTag {
    [CmdletBinding()]
    [OutputType([bool])]

    param([string] $Name)

    if ([string]::IsNullOrWhiteSpace($Name)) {
        return $false
    }
    & git rev-parse --verify --quiet "refs/tags/$Name^{commit}" *> $null
    return ($LASTEXITCODE -eq 0)
}

function Get-CentralTimeZone {
    [CmdletBinding()]
    [OutputType([TimeZoneInfo])]
    param()

    foreach ($id in @('Central Standard Time', 'America/Chicago')) {
        try {
            return [System.TimeZoneInfo]::FindSystemTimeZoneById($id)
        } catch [TimeZoneNotFoundException], [InvalidTimeZoneException] { continue }
    }
    throw 'Could not resolve the America/Chicago release time zone.'
}

function Get-ReleaseDateStamp {
    [CmdletBinding()]
    [OutputType([string])]

    param(
        [datetime] $CurrentUtc,
        [string] $Format
    )

    $utc = $CurrentUtc
    if ($utc.Kind -ne [System.DateTimeKind]::Utc) {
        $utc = $utc.ToUniversalTime()
    }
    $central = [System.TimeZoneInfo]::ConvertTimeFromUtc($utc, (Get-CentralTimeZone))
    return $central.ToString($Format, [System.Globalization.CultureInfo]::InvariantCulture)
}

function Get-LatestReleaseTag {
    [CmdletBinding()]
    [OutputType([string])]

    param([string] $Override)

    if (-not [string]::IsNullOrWhiteSpace($Override)) {
        return $Override
    }

    $arguments = @('describe', '--tags', '--abbrev=0', '--match', 'v*', 'HEAD')
    $latestTag = Get-FirstOutput -Value (Invoke-Git -Arguments $arguments -AllowFailure)
    if ([string]::IsNullOrWhiteSpace($latestTag)) {
        return ''
    }
    return $latestTag.Trim()
}

function Get-ReleaseDiffPathspec {
    [CmdletBinding()]
    [OutputType([object[]])]
    param()

    return @(
        '.',
        ':(top,exclude)CHANGELOG.md',
        ':(top,exclude)data/wrapper_hashes.txt'
    )
}

function Test-HasChangesSinceTag {
    [CmdletBinding()]
    [OutputType([bool])]

    param([string] $Name)

    if ([string]::IsNullOrWhiteSpace($Name)) {
        return $true
    }
    if (-not (Test-GitTag -Name $Name)) {
        return $true
    }

    $arguments = @('diff', '--quiet', "$Name..HEAD", '--') + (Get-ReleaseDiffPathspec)
    & git @arguments
    $code = $LASTEXITCODE
    if ($code -eq 0) {
        return $false
    }
    if ($code -eq 1) {
        return $true
    }
    throw "git diff failed while comparing $Name..HEAD"
}

function Get-ChangedPathCountSinceTag {
    [CmdletBinding()]
    [OutputType([int])]

    param([string] $Name)

    if ([string]::IsNullOrWhiteSpace($Name) -or -not (Test-GitTag -Name $Name)) {
        return 0
    }

    $arguments = @('diff', '--name-only', "$Name..HEAD", '--') + (Get-ReleaseDiffPathspec)
    $paths = @(Invoke-Git -Arguments $arguments)
    return $paths.Count
}

function Get-NextBetaTag {
    [CmdletBinding()]
    [OutputType([string])]

    param(
        [string] $DateStamp,
        [datetime] $CurrentUtc
    )

    if ([string]::IsNullOrWhiteSpace($DateStamp)) {
        $DateStamp = Get-ReleaseDateStamp -CurrentUtc $CurrentUtc -Format 'yyyy.M.d'
    }

    $escaped = [regex]::Escape($DateStamp)
    $pattern = "^v$escaped\.(\d+)(-[A-Za-z0-9]{4})?$"
    $highest = -1
    foreach ($existing in @(Invoke-Git -Arguments @('tag', '--list', "v$DateStamp.*"))) {
        if ($existing -match $pattern) {
            $value = [int]$Matches[1]
            if ($value -gt $highest) {
                $highest = $value
            }
        }
    }

    return "v$DateStamp.$($highest + 1)-beta"
}

function Write-GitHubOutput {
    [CmdletBinding()]
    [OutputType([void])]

    param(
        [string] $Name,
        [string] $Value
    )

    if ([string]::IsNullOrWhiteSpace($env:GITHUB_OUTPUT)) {
        return
    }
    $encoding = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::AppendAllText($env:GITHUB_OUTPUT, "$Name=$Value`n", $encoding)
}

$repoRootPath = Resolve-RepoRoot -Value $RepoRoot
Push-Location $repoRootPath
try {
    if (-not [string]::IsNullOrWhiteSpace($Tag) -and $Tag -notmatch '^v\d{4}\.\d+\.\d+\.\d+-beta$') {
        throw 'Nightly beta tags must match vYYYY.M.D.N-beta.'
    }

    $head = (Get-FirstOutput -Value (Invoke-Git -Arguments @('rev-parse', 'HEAD'))).Trim()
    $base = Get-LatestReleaseTag -Override $BaseTag
    $hasChanges = Test-HasChangesSinceTag -Name $base
    $changedPathCount = Get-ChangedPathCountSinceTag -Name $base
    if ([string]::IsNullOrWhiteSpace($Tag)) {
        $nextTag = Get-NextBetaTag -DateStamp $Today -CurrentUtc $NowUtc
    } else {
        $nextTag = $Tag
    }

    $plan = [pscustomobject]@{
        has_changes        = $hasChanges
        changed_path_count = $changedPathCount
        base_tag           = $base
        head_sha           = $head
        next_tag           = $nextTag
    }

    if (-not [string]::IsNullOrWhiteSpace($OutputJsonPath)) {
        $resolvedOutputPath = [System.IO.Path]::GetFullPath($OutputJsonPath)
        $parent = Split-Path -Parent $resolvedOutputPath
        if (-not (Test-Path -LiteralPath $parent)) {
            $null = New-Item -ItemType Directory -Path $parent
        }
        $json = $plan | ConvertTo-Json -Depth 4 -Compress
        $encoding = New-Object System.Text.UTF8Encoding($false)
        [System.IO.File]::WriteAllText($resolvedOutputPath, $json, $encoding)
    }

    Write-GitHubOutput -Name 'has_changes' -Value ([string]$hasChanges).ToLowerInvariant()
    Write-GitHubOutput -Name 'changed_path_count' -Value ([string]$changedPathCount)
    Write-GitHubOutput -Name 'base_tag' -Value $base
    Write-GitHubOutput -Name 'head_sha' -Value $head
    Write-GitHubOutput -Name 'next_tag' -Value $nextTag

    Write-Host 'Nightly beta release plan:'
    Write-Host "  Base tag: $base"
    Write-Host "  Head: $head"
    Write-Host "  Changed path count: $changedPathCount"
    Write-Host "  Next tag: $nextTag"
    Write-Host "  Has changes: $hasChanges"
} finally {
    Pop-Location
}

exit 0
