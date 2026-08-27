#!/usr/bin/env pwsh

[CmdletBinding()]
param(
    [string] $Tag = $(if ($env:TAG_NAME) { $env:TAG_NAME } else { $env:GITHUB_REF_NAME }),
    [string] $Repo = $env:GITHUB_REPOSITORY,
    [string] $Extras = $null,
    [string] $TemplateDir = $null,
    [string] $Manifest = $null,
    [string] $ZipPath = $null,
    [string] $ZipName = $null,
    [long]   $ZipSize = 0,
    [string] $ZipSha256 = $null,
    [switch] $AllowEmpty,
    [string[]] $PrereleaseTags = @(),
    [switch] $SkipScrub
)

$ErrorActionPreference = 'Stop'

if (-not $Tag) { throw "No tag provided (pass -Tag or set TAG_NAME / GITHUB_REF_NAME)." }

if (-not $Extras) {
    $Extras = Join-Path -Path (Get-Location) -ChildPath ".github/release-extras/$Tag.md"
}

if (-not $TemplateDir) {
    $TemplateDir = Join-Path -Path (Get-Location) -ChildPath ".github/release-template"
}

function Get-KnownPrereleaseTags([string]$Repo, [string[]]$AdditionalTags) {
    $set = @{}
    foreach ($tagName in $AdditionalTags) {
        if ($tagName) { $set[$tagName] = $true }
    }

    if ($Repo) {
        $listJson = & gh release list --repo $Repo --limit 100 --json tagName, isPrerelease 2>$null
        if ($LASTEXITCODE -eq 0 -and $listJson) {
            try {
                $releases = $listJson | ConvertFrom-Json
                foreach ($release in $releases) {
                    if ($release.isPrerelease -and $release.tagName) {
                        $set[$release.tagName] = $true
                    }
                }
            }
            catch {
                Write-Host "::warning::Failed to parse 'gh release list' output while reading prerelease tags: $_."
            }
        }
    }

    return $set
}

function Test-IsPrereleaseTag([string]$Tag, [hashtable]$KnownPrereleaseTags) {
    if ($KnownPrereleaseTags -and $KnownPrereleaseTags.ContainsKey($Tag)) { return $true }
    return $Tag -match '^v?\d{4}\.\d+\.\d+\.\d+-.+'
}

function Resolve-PrevTagForSlice([string]$Tag, [string]$Repo, [string[]]$AdditionalPrereleaseTags) {
    $ErrorActionPreference = 'Continue'

    $knownPrereleaseTags = Get-KnownPrereleaseTags -Repo $Repo -AdditionalTags $AdditionalPrereleaseTags

    $describeArgs = @('describe', '--tags', '--abbrev=0')
    if (-not (Test-IsPrereleaseTag -Tag $Tag -KnownPrereleaseTags $knownPrereleaseTags)) {
        $describeArgs += @('--exclude', '*-*')
        foreach ($preTag in $knownPrereleaseTags.Keys) {
            if ($preTag -and $preTag -ne $Tag) {
                $describeArgs += @('--exclude', $preTag)
            }
        }
    }
    $describeArgs += "$Tag^"
    $prevRef = & git @describeArgs 2>$null
    if ($LASTEXITCODE -eq 0 -and $prevRef) {
        $prevTag = $prevRef.Trim()
        $count = & git rev-list --count "$prevTag..$Tag" 2>$null
        if ($LASTEXITCODE -eq 0 -and [int]$count -le 50) {
            return @{
                Tag     = $prevTag
                LogArgs = @("$prevTag..$Tag")
                Display = "$prevTag..$Tag"
                Source  = 'describe'
            }
        }
        Write-Host "::warning::Slice from $prevTag..$Tag is $count commits (>50 cap). Falling back to subject-match against the most recent published release."
    }

    if ($Repo) {
        $listJson = & gh release list --repo $Repo --limit 20 --json tagName, publishedAt, isPrerelease 2>$null
        if ($LASTEXITCODE -eq 0 -and $listJson) {
            $candidatePrevTag = $null
            try {
                $releases = $listJson | ConvertFrom-Json
                $candidate = $releases |
                    Where-Object { $_.tagName -ne $Tag -and -not $_.isPrerelease } |
                    Sort-Object publishedAt -Descending |
                    Select-Object -First 1
                if ($candidate) { $candidatePrevTag = $candidate.tagName }
            }
            catch {
                Write-Host "::warning::Failed to parse 'gh release list' output: $_."
            }

            if ($candidatePrevTag) {
                $orphanSha = & git rev-list -n 1 $candidatePrevTag 2>$null
                if ($LASTEXITCODE -eq 0 -and $orphanSha) {
                    $orphanSha = $orphanSha.Trim()
                    & git merge-base --is-ancestor $orphanSha $Tag 2>$null
                    if ($LASTEXITCODE -eq 0) {
                        Write-Host "::warning::Using published stable release $candidatePrevTag as slice base after describe exceeded the sanity cap."
                        return @{
                            Tag     = $candidatePrevTag
                            LogArgs = @("$candidatePrevTag..$Tag")
                            Display = "$candidatePrevTag..$Tag"
                            Source  = 'published-release'
                        }
                    }

                    $orphanSubject = & git show -s --format=%s $orphanSha 2>$null
                    if ($LASTEXITCODE -eq 0 -and $orphanSubject) {
                        $rebasedSha = $null
                        $logLines = & git log $Tag --format='%H%x09%s' 2>$null
                        if ($LASTEXITCODE -eq 0 -and $logLines) {
                            $lineArr = if ($logLines -is [array]) { $logLines } else { , $logLines }
                            foreach ($line in $lineArr) {
                                if (-not $line) { continue }
                                $parts = $line -split "`t", 2
                                if ($parts.Count -eq 2 -and $parts[1] -eq $orphanSubject) {
                                    $rebasedSha = $parts[0]
                                    break
                                }
                            }
                        }
                        if ($rebasedSha) {
                            $shortSha = $rebasedSha.Substring(0, 12)
                            Write-Host "::warning::Subject-matched slice: prev tag $candidatePrevTag (orphan sha $($orphanSha.Substring(0,12))) matches current-history sha $shortSha by subject; using $shortSha..$Tag."
                            return @{
                                Tag     = $candidatePrevTag
                                LogArgs = @("$rebasedSha..$Tag")
                                Display = "$candidatePrevTag..$Tag (subject-matched at $shortSha)"
                                Source  = 'subject-match'
                            }
                        }
                        Write-Host "::warning::Prev tag $candidatePrevTag subject '$orphanSubject' not found in current $Tag history. Falling back to first-release changelog notes."
                    }
                }
            }
        }
        else {
            Write-Host "::warning::'gh release list' produced no usable output (gh not authed or no releases yet). Falling back to first-release changelog notes."
        }
    }

    Write-Host "::warning::No prior tag or published release matched; treating $Tag as the first release."
    return @{
        Tag     = $null
        LogArgs = @()
        Display = "$Tag (first release)"
        Source  = 'first-release'
    }
}

$prevInfo = Resolve-PrevTagForSlice -Tag $Tag -Repo $Repo -AdditionalPrereleaseTags $PrereleaseTags
$prevTag = $prevInfo.Tag
$logArgs = $prevInfo.LogArgs
$range = $prevInfo.Display

function Read-ChangelogNotes([string]$Version) {
    $path = Join-Path -Path (Get-Location) -ChildPath 'CHANGELOG.md'
    if (-not (Test-Path -LiteralPath $path)) { return $null }

    $content = Get-Content -LiteralPath $path -Raw -Encoding UTF8
    if ($Version) {
        $escaped = [regex]::Escape($Version)
        $versionPattern = "(?ms)^##\s+\[$escaped\][^\n]*\n(.*?)(?=^---\s*$|^##\s+|\z)"
        $versionMatch = [regex]::Match($content, $versionPattern)
        if ($versionMatch.Success) {
            $notes = $versionMatch.Groups[1].Value.Trim()
            if ($notes) { return $notes }
        }
    }

    $unreleasedPattern = '(?ms)^##\s+Unreleased\s*\n(.*?)(?=^---\s*$|^##\s+|\z)'
    $unreleasedMatch = [regex]::Match($content, $unreleasedPattern)
    if ($unreleasedMatch.Success) {
        $notes = $unreleasedMatch.Groups[1].Value.Trim()
        if ($notes -and $notes -notmatch '^_No notable changes since the last release\._$') {
            return $notes
        }
    }

    return $null
}

$changelogNotes = $null
if ($prevInfo.Source -eq 'first-release') {
    $changelogNotes = Read-ChangelogNotes -Version $Tag
    if (-not $changelogNotes) {
        if ($AllowEmpty) {
            $changelogNotes = '_First release; see commit log for details._'
        }
        else {
            throw "No prior tag or release exists, and CHANGELOG.md has no notes for $Tag or Unreleased. " +
            "For the first release, write curated public notes under ## Unreleased before tagging."
        }
    }
}

$raw = @()
if (-not $changelogNotes) {
    $raw = & git log @logArgs --no-merges --pretty=format:"%H`t%h`t%an`t%s" 2>$null
    if ($LASTEXITCODE -ne 0) { $raw = @() }
}

$lines = @()
if ($raw) { $lines = $raw -split "`r?`n" | Where-Object { $_ } }

$AuthorHandleMap = @{
    'WhyKnot' = 'RealWhyKnot'
}

$entries = foreach ($line in $lines) {
    if ($line -match '\[skip changelog\]') { continue }
    $parts = $line -split "`t", 4
    if ($parts.Count -lt 4) { continue }
    $sha = $parts[0]
    $short = $parts[1]
    $author = $parts[2]
    if ($AuthorHandleMap.ContainsKey($author)) { $author = $AuthorHandleMap[$author] }
    $subject = $parts[3]

    $subject = $subject -replace '\s*\(\d{4}\.\d+\.\d+\.\d+-[A-Fa-f0-9]+\)\s*', ' '
    $subject = $subject.Trim() -replace '\s{2,}', ' '

    [pscustomobject]@{
        Sha     = $sha
        Short   = $short
        Author  = $author
        Subject = $subject
    }
}

if (-not $changelogNotes -and (-not $entries -or $entries.Count -eq 0)) {
    if ($AllowEmpty) {
        return "## What's Changed`n`n_First release; see commit log for details._`n"
    }
    throw "No commits found in range $range. " +
    "Either the previous tag is misdetected, every commit in the range " +
    "carries [skip changelog], or the tag points at an empty branch. " +
    "Pass -AllowEmpty for a first release. Otherwise amend the offending " +
    "commits or push a real change before tagging."
}

function Get-Category([string] $subject) {
    if ($subject -match '^feat(\(.+?\))?!?:') { return @{ Order = 1; Name = 'Features' } }
    if ($subject -match '^fix(\(.+?\))?!?:') { return @{ Order = 2; Name = 'Bug Fixes' } }
    if ($subject -match '^perf(\(.+?\))?!?:') { return @{ Order = 3; Name = 'Performance' } }
    if ($subject -match '^refactor(\(.+?\))?!?:') { return @{ Order = 4; Name = 'Refactors' } }
    if ($subject -match '^revert(\(.+?\))?!?:') { return @{ Order = 5; Name = 'Reverts' } }
    if ($subject -match '^docs(\(.+?\))?!?:') { return @{ Order = 6; Name = 'Documentation' } }
    if ($subject -match '^style(\(.+?\))?!?:') { return @{ Order = 7; Name = 'Style' } }
    if ($subject -match '^test(\(.+?\))?!?:') { return @{ Order = 8; Name = 'Tests' } }
    if ($subject -match '^ci(\(.+?\))?!?:') { return @{ Order = 9; Name = 'CI' } }
    if ($subject -match '^build(\(.+?\))?!?:') { return @{ Order = 10; Name = 'Build' } }
    if ($subject -match '^chore(\(.+?\))?!?:') { return @{ Order = 11; Name = 'Chores' } }
    return @{ Order = 99; Name = 'Other Changes' }
}

$nonConforming = @()
if (-not $changelogNotes) {
    $nonConforming = @($entries | Where-Object {
            $_.Subject -notmatch '^(feat|fix|perf|refactor|revert|docs|style|test|ci|build|chore)(\(.+?\))?!?:'
        })
    if ($nonConforming.Count -gt 0) {
        Write-Host "::warning::$($nonConforming.Count) commit(s) in range $range do not follow conventional-commit prefixes; bucketed under 'Other Changes':"
        foreach ($e in $nonConforming) {
            Write-Host "::warning::  $($e.Short)  $($e.Subject)"
        }
    }
}

$useGroups = $false
if (-not $changelogNotes) {
    foreach ($e in $entries) {
        if ($e.Subject -match '^(feat|fix|perf|refactor|revert|docs|style|test|ci|build|chore)(\(.+?\))?!?:') {
            $useGroups = $true
            break
        }
    }
}

$ownerOnly = ''
$repoShort = ''
if ($Repo -and ($Repo -match '/')) {
    $parts = $Repo -split '/', 2
    $ownerOnly = $parts[0]
    $repoShort = $parts[1]
}
elseif ($Repo) {
    $repoShort = $Repo
}
$tagCommitSha = ''
$tagCommitShort = ''
$prevErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
try {
    $tagSha = & git rev-parse "$Tag^{}" 2>$null
    if ($LASTEXITCODE -eq 0 -and $tagSha) {
        $tagCommitSha = $tagSha.Trim()
        if ($tagCommitSha.Length -ge 12) { $tagCommitShort = $tagCommitSha.Substring(0, 12) }
    }
}
finally {
    $ErrorActionPreference = $prevErrorActionPreference
}
$priorTagToken = if ($prevTag) { $prevTag } else { '' }
$zipNameToken = if ($ZipName) { $ZipName } elseif ($ZipPath) { (Split-Path -Leaf $ZipPath) } else { '' }
$tokens = @{
    '{tag}'              = $Tag
    '{version}'          = ($Tag -replace '^v', '')
    '{owner}'            = $ownerOnly
    '{repo}'             = $repoShort
    '{full-repo}'        = $Repo
    '{commit-sha}'       = $tagCommitSha
    '{commit-sha-short}' = $tagCommitShort
    '{prior-tag}'        = $priorTagToken
    '{zip-name}'         = $zipNameToken
}

function Expand-Tokens([string] $text, [hashtable] $map) {
    if (-not $text) { return $text }
    foreach ($key in $map.Keys) {
        $val = $map[$key]
        if ($null -eq $val) { $val = '' }
        $text = $text.Replace($key, $val)
    }
    return $text
}

function Format-Bytes([long] $bytes) {
    if ($bytes -ge 1MB) { return ('{0:F2} MB' -f ($bytes / 1MB)) }
    if ($bytes -ge 1KB) { return ('{0:F2} KB' -f ($bytes / 1KB)) }
    return ('{0} B' -f $bytes)
}

function Read-TemplateSection([string] $name, [string] $dir, [hashtable] $tokenMap) {
    $path = Join-Path -Path $dir -ChildPath "$name.md"
    if (-not (Test-Path -LiteralPath $path)) {
        Write-Host "::warning::Release-body template missing: $path. Section '$name' will not render."
        return $null
    }
    $rawContent = Get-Content -LiteralPath $path -Raw -Encoding UTF8
    if ($null -eq $rawContent) { return $null }
    $content = $rawContent.Trim()
    if (-not $content) { return $null }
    return (Expand-Tokens -text $content -map $tokenMap)
}

$sb = [System.Text.StringBuilder]::new()
if ($repoShort) {
    [void]$sb.AppendLine("# $repoShort $Tag")
    [void]$sb.AppendLine()
}
[void]$sb.AppendLine("## What's Changed")
[void]$sb.AppendLine()

if ($changelogNotes) {
    [void]$sb.AppendLine($changelogNotes)
    [void]$sb.AppendLine()
}
elseif ($useGroups) {
    $tagged = foreach ($e in $entries) {
        $cat = Get-Category $e.Subject
        [pscustomobject]@{ Order = $cat.Order; Name = $cat.Name; Entry = $e }
    }
    $groups = $tagged | Group-Object Name | Sort-Object { ($_.Group | Select-Object -First 1).Order }
    foreach ($g in $groups) {
        [void]$sb.AppendLine("### $($g.Name)")
        foreach ($t in $g.Group) {
            $e = $t.Entry
            [void]$sb.AppendLine("- $($e.Subject) by @$($e.Author) in $($e.Short)")
        }
        [void]$sb.AppendLine()
    }
}
else {
    foreach ($e in $entries) {
        [void]$sb.AppendLine("- $($e.Subject) by @$($e.Author) in $($e.Short)")
    }
    [void]$sb.AppendLine()
}

if ($Repo -and $prevTag) {
    [void]$sb.AppendLine("**Full Changelog**: https://github.com/$Repo/compare/$prevTag...$Tag")
}

$includeIntegrity = $ZipPath -and $ZipSha256 -and $ZipSize -gt 0 -and $Manifest -and (Test-Path -LiteralPath $Manifest)
if ($includeIntegrity) {
    $zipNameForLine = if ($zipNameToken) { $zipNameToken } else { Split-Path -Leaf $ZipPath }
    $integrityName = $zipNameForLine -replace '\.zip$', '.integrity.tsv'
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("## File integrity")
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("Full SHA256 hashes are attached as ``$integrityName``.")
}
elseif ($Manifest -or $ZipPath -or $ZipSha256) {
    Write-Host "::warning::File-integrity section skipped: -Manifest, -ZipPath, -ZipSize, and -ZipSha256 must all be set. Got Manifest='$Manifest' ZipPath='$ZipPath' ZipSize=$ZipSize ZipSha256='$ZipSha256'."
}

$templateOrder = @('links', 'install', 'uninstall', 'what-you-need-to-do')
foreach ($name in $templateOrder) {
    $section = Read-TemplateSection -name $name -dir $TemplateDir -tokenMap $tokens
    if ($section) {
        [void]$sb.AppendLine()
        [void]$sb.AppendLine($section)
    }
}

if (Test-Path -LiteralPath $Extras) {
    $extrasContent = (Get-Content -LiteralPath $Extras -Raw -Encoding UTF8).Trim()
    if ($extrasContent) {
        [void]$sb.AppendLine()
        [void]$sb.AppendLine("---")
        [void]$sb.AppendLine()
        [void]$sb.AppendLine("## Additional notes")
        [void]$sb.AppendLine()
        [void]$sb.AppendLine($extrasContent)
    }
}

$body = $sb.ToString().TrimEnd()

$asciiSubs = @(
    @{ Pattern = [string][char]0x2014; Replacement = '--' }
    @{ Pattern = [string][char]0x2013; Replacement = '-' }
    @{ Pattern = [string][char]0x2026; Replacement = '...' }
    @{ Pattern = [string][char]0x201C; Replacement = '"' }
    @{ Pattern = [string][char]0x201D; Replacement = '"' }
    @{ Pattern = [string][char]0x2018; Replacement = "'" }
    @{ Pattern = [string][char]0x2019; Replacement = "'" }
    @{ Pattern = [string][char]0x00A0; Replacement = ' ' }
    @{ Pattern = [string][char]0x2022; Replacement = '*' }
    @{ Pattern = [string][char]0x00D7; Replacement = 'x' }
    @{ Pattern = [string][char]0x2192; Replacement = '->' }
    @{ Pattern = [string][char]0x2190; Replacement = '<-' }
    @{ Pattern = [string][char]0x21D2; Replacement = '=>' }
    @{ Pattern = [string][char]0x21D0; Replacement = '<=' }
    @{ Pattern = [string][char]0x00A7; Replacement = 'section' }
    @{ Pattern = [string][char]0x00B6; Replacement = 'paragraph' }
)
foreach ($sub in $asciiSubs) {
    $body = $body.Replace($sub.Pattern, $sub.Replacement)
}

if (-not $SkipScrub) {
    $lineNumber = 0
    $offenders = foreach ($line in ($body -split "`r?`n")) {
        $lineNumber++
        for ($i = 0; $i -lt $line.Length; $i++) {
            $ch = $line[$i]
            $code = [int][char]$ch
            $isAllowed = ($code -ge 0x20 -and $code -le 0x7E) -or $code -eq 9
            if (-not $isAllowed) {
                [pscustomobject]@{
                    Line = $lineNumber
                    Col  = $i + 1
                    Char = $ch
                    Code = ('U+{0:X4}' -f $code)
                    Text = $line
                }
            }
        }
    }
    if ($offenders) {
        $report = $offenders | ForEach-Object { "  line $($_.Line) col $($_.Col): $($_.Code) in: $($_.Text)" }
        throw "Non-ASCII characters in release body after normalisation:`n$($report -join "`n")`n" +
        "Fix: amend the offending commit subject (or extras file) to use ASCII equivalents. " +
        "Common substitutes are pre-mapped in Generate-ReleaseNotes.ps1; if a new character " +
        "trips this, add it to `$asciiSubs and try again."
    }

    $forbiddenPatterns = @(
        '\bcomprehensive\b'
        '\bleveraging\b'
        '\bwhether\s+you''?re\b'
        '\bempowers?\b'
        '\bstreamline\b'
        '\belevate\b'
        '\bcutting-edge\b'
        '\bseamless(ly)?\b'
        '\belegant\b'
        '\binvestigator\b'
        '\btriage\b'
        '\bscope plan\b'
        '\btier [0-9]\b'
        '\bdiagnostic gap\b'
        '\bship report\b'
        '\bmemory entry\b'
        '\bverification matrix\b'
        '\borchestrator\b'
        '\bcowork\b'
        '\bfuture-you\b'
        '\bfuture contributor\b'
        '\bfuture spelunker\b'
        '\b\d+ weeks of work\b'
        '\bmonths of effort\b'
        '\byears in the making\b'
    )
    $matches = foreach ($pat in $forbiddenPatterns) {
        $found = [regex]::Matches($body, $pat, 'IgnoreCase')
        foreach ($m in $found) {
            [pscustomobject]@{ Pattern = $pat; Match = $m.Value; Index = $m.Index }
        }
    }
    if ($matches) {
        $report = $matches | ForEach-Object { "  pattern $($_.Pattern) matched '$($_.Match)' at index $($_.Index)" }
        throw "voice or internal-only-vocabulary patterns in release body:`n$($report -join "`n")`n" +
        "Fix: amend the offending commit subject (or extras file) to use plainer language, " +
        "or mark the commit [skip changelog] if the term is unavoidable."
    }
}

$body
