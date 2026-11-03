#!/usr/bin/env pwsh
#Requires -Version 7.4
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet('Append', 'Promote', 'Notes')]
    [string]$Mode,

    [string]$Range,
    [string]$Version,
    [switch]$ForVersion,
    [string]$Repo = $env:GITHUB_REPOSITORY,
    [string]$RepoRoot,
    [datetime]$NowUtc = ([datetime]::UtcNow)
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Read-TextUtf8 {
    [CmdletBinding()]
    [OutputType([string])]

    param([string]$Path)
    return [System.IO.File]::ReadAllText($Path, [System.Text.UTF8Encoding]::new($false))
}

function Write-TextUtf8 {
    [CmdletBinding()]
    [OutputType([void])]

    param([string]$Path, [string]$Content)
    [System.IO.File]::WriteAllText($Path, $Content, [System.Text.UTF8Encoding]::new($false))
}

function Get-CentralTimeZone {
    [CmdletBinding()]
    [OutputType([TimeZoneInfo])]
    param()

    foreach ($id in @('Central Standard Time', 'America/Chicago')) {
        try { return [System.TimeZoneInfo]::FindSystemTimeZoneById($id) }
        catch [TimeZoneNotFoundException], [InvalidTimeZoneException] { continue }
    }
    throw 'Could not resolve the America/Chicago release time zone.'
}

function Get-ReleaseDateStamp {
    [CmdletBinding()]
    [OutputType([string])]

    param([datetime]$NowUtc, [string]$Format)

    $utc = $NowUtc
    if ($utc.Kind -ne [System.DateTimeKind]::Utc) {
        $utc = $utc.ToUniversalTime()
    }
    $central = [System.TimeZoneInfo]::ConvertTimeFromUtc($utc, (Get-CentralTimeZone))
    return $central.ToString($Format, [System.Globalization.CultureInfo]::InvariantCulture)
}

function Remove-BuildStamp {
    [CmdletBinding()]
    [OutputType([string])]

    param([string]$Subject)
    return ($Subject -replace ' \(\d{4}\.\d+\.\d+\.\d+(-([A-Fa-f0-9]{4,8}|beta))?\)$', '').Trim()
}

function ConvertFrom-CommitSubject {
    [CmdletBinding()]
    [OutputType([hashtable])]

    param([string]$Sha, [string]$Subject)

    $stripped = Remove-BuildStamp -Subject $Subject
    if ([string]::IsNullOrWhiteSpace($stripped)) { return $null }

    if ($stripped -match '\[skip changelog\]') { return $null }

    if ($stripped -match '^Merge ') { return $null }

    $pattern = '^(?<type>feat|fix|perf|refactor|docs|build|ci|chore|test|revert)' +
    '(?:\((?<scope>[^)]+)\))?(?<bang>!)?:\s+(?<desc>.+)$'
    $m = [regex]::Match($stripped, $pattern)
    if (-not $m.Success) {
        return @{
            Bucket = 'Changed'
            Bullet = "- $stripped (" + $Sha.Substring(0, 7) + ')'
        }
    }

    $type = $m.Groups['type'].Value
    $scope = $m.Groups['scope'].Value
    $isBreaking = $m.Groups['bang'].Success
    $desc = $m.Groups['desc'].Value

    if ($desc.Length -gt 0) {
        $desc = $desc.Substring(0, 1).ToUpper() + $desc.Substring(1)
    }

    $bucket = switch ($type) {
        'feat' { 'Added' }
        'fix' { 'Fixed' }
        'perf' { 'Changed' }
        'refactor' { 'Changed' }
        'revert' { 'Changed' }
        'chore' {
            if ($scope -and $scope -match '^deps') { 'Changed' } else { $null }
        }
        default { $null }
    }

    if (-not $bucket) { return $null }
    if ($isBreaking) { $bucket = 'Breaking' }

    $scopePrefix = if ($scope) { "**${scope}:** " } else { '' }
    $shortSha = $Sha.Substring(0, 7)
    $bullet = "- $scopePrefix$desc ($shortSha)"

    return @{ Bucket = $bucket; Bullet = $bullet }
}

function Find-UnreleasedSection {
    [CmdletBinding()]
    [OutputType([hashtable])]

    param([string]$Content)

    $lines = $Content -split "`n"
    $startIdx = -1
    for ($i = 0; $i -lt $lines.Length; $i++) {
        if ($lines[$i] -match '^##\s+Unreleased\s*$') {
            $startIdx = $i
            break
        }
    }
    if ($startIdx -lt 0) { return $null }

    $endIdx = $lines.Length
    for ($j = $startIdx + 1; $j -lt $lines.Length; $j++) {
        if ($lines[$j] -match '^---\s*$') {
            $endIdx = $j
            break
        }
        if ($lines[$j] -match '^##\s+') {
            $endIdx = $j
            break
        }
    }

    return @{
        StartIdx = $startIdx
        EndIdx   = $endIdx
        Lines    = $lines
    }
}

function ConvertFrom-UnreleasedBody {
    [CmdletBinding()]
    [OutputType([System.Collections.Specialized.OrderedDictionary])]

    param([string[]]$BodyLines)

    $buckets = [ordered]@{}
    $current = $null
    foreach ($line in $BodyLines) {
        if ($line -match '^###\s+(?<name>.+?)\s*$') {
            $current = $matches['name']
            if (-not $buckets.Contains($current)) { $buckets[$current] = [Collections.Generic.List[string]]::new() }
            continue
        }
        if ($line -match '^\s*- ' -and $current) {
            $buckets[$current].Add($line)
        }
    }
    return $buckets
}

function ConvertTo-UnreleasedBody {
    [CmdletBinding()]
    [OutputType([object[]])]

    param([hashtable]$Buckets)

    if ($Buckets.Count -eq 0) {
        return @('', '_No notable changes since the last release._', '')
    }

    $out = [Collections.Generic.List[string]]::new()
    $out.Add('')
    $emitted = @{}
    foreach ($name in $BucketOrder) {
        if ($Buckets.Contains($name) -and $Buckets[$name].Count -gt 0) {
            $out.Add("### $name")
            $out.AddRange([string[]]$Buckets[$name])
            $out.Add('')
            $emitted[$name] = $true
        }
    }
    foreach ($name in $Buckets.Keys) {
        if (-not $emitted.ContainsKey($name) -and $Buckets[$name].Count -gt 0) {
            $out.Add("### $name")
            $out.AddRange([string[]]$Buckets[$name])
            $out.Add('')
        }
    }
    return $out.ToArray()
}

function Update-OneFile {
    [CmdletBinding()]
    [OutputType([void])]

    param(
        [string]$Path,
        [hashtable]$NewBullets
    )

    $content = Read-TextUtf8 -Path $Path
    $section = Find-UnreleasedSection -Content $content
    if (-not $section) {
        throw "$Path is missing the '## Unreleased' section. Add a stub heading at the top before running the appender."
    }

    $bodyStart = $section.StartIdx + 1
    $bodyEnd = $section.EndIdx - 1
    $bodyLines = if ($bodyEnd -ge $bodyStart) { $section.Lines[$bodyStart..$bodyEnd] } else { @() }

    $existing = ConvertFrom-UnreleasedBody -BodyLines $bodyLines

    foreach ($bucket in $NewBullets.Keys) {
        if (-not $existing.Contains($bucket)) { $existing[$bucket] = [Collections.Generic.List[string]]::new() }
        foreach ($bullet in $NewBullets[$bucket]) {
            $sha = if ($bullet -match '\(([a-f0-9]{7})\)\s*$') { $matches[1] } else { $null }
            if ($sha) {
                $alreadyHas = $false
                foreach ($line in $existing[$bucket]) {
                    if ($line -match "\($sha\)") { $alreadyHas = $true; break }
                }
                if ($alreadyHas) { continue }
            }
            $existing[$bucket].Add($bullet)
        }
    }

    $rendered = ConvertTo-UnreleasedBody -Buckets $existing
    $before = if ($section.StartIdx -gt 0) { $section.Lines[0..($section.StartIdx)] } else { @($section.Lines[0]) }
    $after = if ($section.EndIdx -lt $section.Lines.Length) {
        $section.Lines[$section.EndIdx..($section.Lines.Length - 1)]
    } else { @() }

    $newLines = [Collections.Generic.List[string]]::new()
    $newLines.AddRange([string[]]@($before))
    $newLines.AddRange([string[]]@($rendered))
    $newLines.AddRange([string[]]@($after))

    $newContent = ($newLines -join "`n")
    Write-TextUtf8 -Path $Path -Content $newContent
}

if (-not $RepoRoot) {
    $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}

$RootChangelog = Join-Path $RepoRoot 'CHANGELOG.md'

if (-not (Test-Path $RootChangelog)) { throw "CHANGELOG.md not found at $RootChangelog" }

$BucketOrder = @('Breaking', 'Added', 'Changed', 'Fixed')

if ($Mode -eq 'Append') {
    if (-not $Range) { throw 'Append mode requires -Range (e.g. abc..def).' }

    Push-Location $RepoRoot
    try {
        $log = & git log --no-merges --format='%H%x09%s%x09%ae' $Range 2>$null
        if ($LASTEXITCODE -ne 0) {
            Write-Host "git log returned non-zero for range '$Range' -- treating as no commits."
            return
        }
    } finally {
        Pop-Location
    }

    if (-not $log) {
        Write-Host "No commits in range $Range -- nothing to append."
        return
    }

    $newBullets = @{}
    $considered = 0
    $included = 0
    foreach ($line in ($log -split "`r?`n")) {
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        $parts = $line -split "`t"
        if ($parts.Length -lt 3) { continue }
        $sha = $parts[0]; $subject = $parts[1]; $email = $parts[2]
        $considered++

        if ($email -match 'github-actions\[bot\]' -or $email -match 'noreply@github.com') { continue }

        $parsed = ConvertFrom-CommitSubject -Sha $sha -Subject $subject
        if (-not $parsed) { continue }

        $bucket = $parsed.Bucket
        if (-not $newBullets.ContainsKey($bucket)) { $newBullets[$bucket] = [Collections.Generic.List[string]]::new() }
        $newBullets[$bucket].Add($parsed.Bullet)
        $included++
    }

    Write-Host "Considered $considered commit(s), included $included in changelog."

    if ($included -eq 0) {
        Write-Host 'Nothing user-visible in this push; CHANGELOG unchanged.'
        return
    }

    Update-OneFile -Path $RootChangelog -NewBullets $newBullets

    Write-Host "Appended $included entr(ies) to CHANGELOG.md."
    return
}

if ($Mode -eq 'Promote') {
    if (-not $Version) { throw 'Promote mode requires -Version (e.g. v2026.4.27.3).' }
    if (-not $Repo) { throw "Promote mode requires -Repo or `$env:GITHUB_REPOSITORY (e.g. owner/repo)." }

    $today = Get-ReleaseDateStamp -NowUtc $NowUtc -Format 'yyyy-MM-dd'
    $heading = "## [$Version](https://github.com/$Repo/releases/tag/$Version) - $today"

    $content = Read-TextUtf8 -Path $RootChangelog
    $section = Find-UnreleasedSection -Content $content
    if (-not $section) {
        throw "$RootChangelog is missing the '## Unreleased' section. Cannot promote."
    }

    $lines = $section.Lines
    $bodyStart = $section.StartIdx + 1
    $bodyEnd = $section.EndIdx - 1
    $bodyLines = if ($bodyEnd -ge $bodyStart) { $lines[$bodyStart..$bodyEnd] } else { @() }

    $hasReal = $false
    foreach ($l in $bodyLines) {
        if ($l -match '^\s*- ' -or $l -match '^###\s+') { $hasReal = $true; break }
    }
    if (-not $hasReal) {
        $bodyLines = @('', '_No user-visible changes in this release._', '')
    }

    $before = if ($section.StartIdx -gt 0) { $lines[0..($section.StartIdx - 1)] } else { @() }
    $after = if ($section.EndIdx -lt $lines.Length) { $lines[$section.EndIdx..($lines.Length - 1)] } else { @() }

    $newLines = [Collections.Generic.List[string]]::new()
    $newLines.AddRange([string[]]@($before))
    $newLines.Add('## Unreleased')
    $newLines.Add('')
    $newLines.Add('_No notable changes since the last release._')
    $newLines.Add('')
    $newLines.Add('---')
    $newLines.Add('')
    $newLines.AddRange([string[]]@($heading))
    $newLines.AddRange([string[]]@($bodyLines))
    $newLines.AddRange([string[]]@($after))

    Write-TextUtf8 -Path $RootChangelog -Content (($newLines -join "`n"))

    Write-Host "Promoted Unreleased -> $heading in CHANGELOG.md."
    return
}

if ($Mode -eq 'Notes') {
    $content = Read-TextUtf8 -Path $RootChangelog

    if ($ForVersion) {
        if (-not $Version) { throw 'Notes -ForVersion requires -Version.' }
        $escaped = [regex]::Escape($Version)
        $pattern = '(?ms)^##\s+\[' + $escaped + '\][^\n]*\n(.*?)(?=^---\s*$|^##\s+|\z)'
        $m = [regex]::Match($content, $pattern)
        if (-not $m.Success) {
            [Console]::Error.WriteLine("No section found for $Version in CHANGELOG.md.")
            exit 1
        }
        Write-Output $m.Groups[1].Value.Trim()
        return
    }

    $section = Find-UnreleasedSection -Content $content
    if (-not $section) { [Console]::Error.WriteLine("No '## Unreleased' section found."); exit 1 }
    $bodyStart = $section.StartIdx + 1
    $bodyEnd = $section.EndIdx - 1
    $bodyLines = if ($bodyEnd -ge $bodyStart) { $section.Lines[$bodyStart..$bodyEnd] } else { @() }
    Write-Output (($bodyLines -join "`n").Trim())
    return
}

exit 0
