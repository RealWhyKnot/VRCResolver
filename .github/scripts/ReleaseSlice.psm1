#Requires -Version 7.4
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Get-KnownPrereleaseTag {
    [CmdletBinding()]
    [OutputType([hashtable])]
    param([string]$Repo, [string[]]$AdditionalTags)

    $set = @{}
    foreach ($tagName in $AdditionalTags) {
        if ($tagName) { $set[$tagName] = $true }
    }

    if ($Repo) {
        $listJson = & gh release list --repo $Repo --limit 100 --json tagName, isPrerelease 2>$null
        if ($LASTEXITCODE -eq 0 -and $listJson) {
            try {
                $releases = $listJson | ConvertFrom-Json -AsHashtable
                foreach ($release in $releases) {
                    if ($release -is [Collections.IDictionary] -and $release['isPrerelease'] -and $release['tagName']) {
                        $set[$release['tagName']] = $true
                    }
                }
            } catch [System.ArgumentException] {
                Write-Host "::warning::Failed to parse 'gh release list' output while reading prerelease tags: $_."
            }
        }
    }

    return $set
}

function Test-IsPrereleaseTag {
    [CmdletBinding()]
    [OutputType([bool])]
    param([string]$Tag, [hashtable]$KnownPrereleaseTags)

    if ($KnownPrereleaseTags -and $KnownPrereleaseTags.ContainsKey($Tag)) { return $true }
    return $Tag -match '^v?\d{4}\.\d+\.\d+\.\d+-.+'
}

function Resolve-PrevTagForSlice {
    [CmdletBinding()]
    [OutputType([hashtable])]
    param([string]$Tag, [string]$Repo, [string[]]$AdditionalPrereleaseTags)

    $ErrorActionPreference = 'Continue'

    $knownPrereleaseTags = Get-KnownPrereleaseTag -Repo $Repo -AdditionalTags $AdditionalPrereleaseTags

    $describeArgs = [Collections.Generic.List[string]]::new()
    $describeArgs.AddRange([string[]]@('describe', '--tags', '--abbrev=0'))
    if (-not (Test-IsPrereleaseTag -Tag $Tag -KnownPrereleaseTags $knownPrereleaseTags)) {
        $describeArgs.AddRange([string[]]@('--exclude', '*-*'))
        foreach ($preTag in $knownPrereleaseTags.Keys) {
            if ($preTag -and $preTag -ne $Tag) {
                $describeArgs.AddRange([string[]]@('--exclude', $preTag))
            }
        }
    }
    $describeArgs.Add("$Tag^")
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
        Write-Host (
            "::warning::Slice from $prevTag..$Tag is $count commits (>50 cap). Falling back to " +
            'subject-match against the most recent published release.'
        )
    }

    if ($Repo) {
        $listJson = & gh release list --repo $Repo --limit 20 --json tagName, publishedAt, isPrerelease 2>$null
        if ($LASTEXITCODE -eq 0 -and $listJson) {
            $candidatePrevTag = $null
            try {
                $releases = $listJson | ConvertFrom-Json -AsHashtable
                $candidate = $releases |
                    Where-Object {
                        $_ -is [Collections.IDictionary] -and $_['tagName'] -ne $Tag -and -not $_['isPrerelease']
                    } |
                    Sort-Object publishedAt -Descending |
                    Select-Object -First 1
                if ($candidate) { $candidatePrevTag = $candidate['tagName'] }
            } catch [System.ArgumentException] {
                Write-Host "::warning::Failed to parse 'gh release list' output: $_."
            }

            if ($candidatePrevTag) {
                $orphanSha = & git rev-list -n 1 $candidatePrevTag 2>$null
                if ($LASTEXITCODE -eq 0 -and $orphanSha) {
                    $orphanSha = $orphanSha.Trim()
                    & git merge-base --is-ancestor $orphanSha $Tag 2>$null
                    if ($LASTEXITCODE -eq 0) {
                        Write-Host (
                            "::warning::Using published stable release $candidatePrevTag as slice base " +
                            'after describe exceeded the sanity cap.'
                        )
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
                            Write-Host (
                                "::warning::Subject-matched slice: prev tag $candidatePrevTag (orphan sha " +
                                "$($orphanSha.Substring(0,12))) matches current-history sha $shortSha by " +
                                "subject; using $shortSha..$Tag."
                            )
                            return @{
                                Tag     = $candidatePrevTag
                                LogArgs = @("$rebasedSha..$Tag")
                                Display = "$candidatePrevTag..$Tag (subject-matched at $shortSha)"
                                Source  = 'subject-match'
                            }
                        }
                        Write-Host (
                            "::warning::Prev tag $candidatePrevTag subject '$orphanSubject' not found in " +
                            "current $Tag history. Falling back to first-release changelog notes."
                        )
                    }
                }
            }
        } else {
            Write-Host (
                "::warning::'gh release list' produced no usable output (gh not authed or no releases " +
                'yet). Falling back to first-release changelog notes.'
            )
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

Export-ModuleMember -Function Resolve-PrevTagForSlice
