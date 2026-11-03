#!/usr/bin/env pwsh
#Requires -Version 7.4
[CmdletBinding()]
param(
    [string] $Root = (Get-Location).Path,
    [string] $SevenProfile = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Test-SevenOnly {
    [CmdletBinding()]
    [OutputType([bool])]
    param(
        [Parameter(Mandatory)][string] $Path
    )
    $tokens = $null
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($Path, [ref]$tokens, [ref]$errors)
    $needs = $ast.ScriptRequirements
    if (-not $needs) {
        return $false
    }
    $version = $needs.RequiredPSVersion
    ($version -and $version.Major -ge 7) -or @($needs.RequiredPSEditions) -contains 'Core'
}

function Get-VendoredPattern {
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][string] $Path
    )
    $parts = @(foreach ($line in [IO.File]::ReadAllLines($Path)) {
            $entry = $line.Trim()
            if (-not $entry) {
                continue
            }
            $name = [regex]::Escape($entry.Trim('/'))
            if ($entry.StartsWith('/')) {
                "^$name/"
            } else {
                "(^|/)$name/"
            }
        })
    if ($parts.Count -eq 0) {
        return '(?!)'
    }
    '(' + ($parts -join '|') + ')'
}

function New-SevenProfile {
    [CmdletBinding()]
    [OutputType([string])]
    param()
    $version = $PSVersionTable.PSVersion
    $platform = 'linux'
    if ($env:OS -eq 'Windows_NT') {
        $platform = 'win'
    }
    $id = '{0}_pwsh_{1}.{2}' -f $platform, $version.Major, $version.Minor
    $cache = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'PSScriptAnalyzer'
    $path = Join-Path $cache "$id.json"
    if (Test-Path -LiteralPath $path -PathType Leaf) {
        return $path
    }
    $analyzer = @(Get-Module -ListAvailable -Name PSScriptAnalyzer | Sort-Object -Property Version -Descending)
    if ($analyzer.Count -eq 0) {
        return ''
    }
    $dll = Join-Path $analyzer[0].ModuleBase 'PSv7/Microsoft.PowerShell.CrossCompatibility.dll'
    $temp = "$path.tmp"
    $null = [IO.Directory]::CreateDirectory($cache)
    $build = {
        param([string] $Dll, [string] $Out, [string] $Id)
        $WarningPreference = 'SilentlyContinue'
        $own = @($PSHOME, $env:windir) | Where-Object { $_ }
        $exclude = @($env:PSModulePath -split [IO.Path]::PathSeparator | Where-Object {
                $entry = $_
                $entry -and -not ($own | Where-Object { $entry.StartsWith($_, [StringComparison]::OrdinalIgnoreCase) })
            })
        Import-Module -Name $Dll
        $null = New-PSCompatibilityProfile -OutFile $Out -PlatformId $Id -ExcludeModulePathPrefix $exclude
    }
    [Console]::Error.WriteLine("Building the $id compatibility profile in $cache (once per pwsh version)")
    & ([Environment]::ProcessPath) -NoProfile -NonInteractive -Command $build -args $dll, $temp, $id
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $temp -PathType Leaf)) {
        return ''
    }
    [IO.File]::Move($temp, $path, $true)
    $path
}

function New-LintSetting {
    [CmdletBinding()]
    [OutputType([hashtable])]
    param(
        [Parameter(Mandatory)][string] $Folder,
        [Parameter(Mandatory)][string[]] $Profiles,
        [Parameter(Mandatory)][string[]] $Versions
    )
    $settings = Import-PowerShellDataFile -LiteralPath (Join-Path $Folder 'PSScriptAnalyzerSettings.psd1')
    $settings.CustomRulePath = [string[]]@(foreach ($path in $settings.CustomRulePath) { Join-Path $Folder $path })
    $settings.Rules.PSUseCompatibleCommands.TargetProfiles = $Profiles
    $settings.Rules.PSUseCompatibleTypes.TargetProfiles = $Profiles
    $settings.Rules.PSUseCompatibleSyntax.TargetVersions = $Versions
    $settings
}

function Invoke-Analyzer {
    [CmdletBinding()]
    [OutputType([object[]])]
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][hashtable] $Settings
    )
    for ($try = 1; ; $try++) {
        try {
            return @(Invoke-ScriptAnalyzer -Path $Path -Settings $Settings)
        } catch [System.NullReferenceException] {
            if ($try -ge 3) {
                throw
            }
        }
    }
}

$folder = (Resolve-Path -LiteralPath $Root).Path
$lint = Join-Path $folder '.github/lint'
$vendoredList = Join-Path $lint 'vendored-paths.txt'
if (-not (Test-Path -LiteralPath $vendoredList -PathType Leaf)) {
    [Console]::Error.WriteLine("vendored-paths.txt is missing in $lint. Run Install-Lint again.")
    exit 2
}
$vendored = Get-VendoredPattern -Path $vendoredList

if (-not $SevenProfile) {
    $SevenProfile = New-SevenProfile
}
if (-not $SevenProfile -or -not (Test-Path -LiteralPath $SevenProfile -PathType Leaf)) {
    [Console]::Error.WriteLine("Can't find or build a pwsh compatibility profile. Install PSScriptAnalyzer 1.25.0.")
    exit 2
}
$windows = 'win-8_x64_10.0.17763.0_5.1.17763.316_x64_4.0.30319.42000_framework'
$both = New-LintSetting -Folder $lint -Profiles $windows, $SevenProfile -Versions '5.1', '7.4'
$seven = New-LintSetting -Folder $lint -Profiles $SevenProfile -Versions '7.4'

$listed = @(& { $ErrorActionPreference = 'Continue'; git -c core.quotepath=off -C $folder ls-files 2>$null })
if ($LASTEXITCODE -ne 0) {
    [Console]::Error.WriteLine("Can't list the files in $folder with git.")
    exit 2
}
$files = @($listed | Where-Object { $_ -match '\.ps[md]?1$' -and $_ -notmatch $vendored })
$found = [System.Collections.Generic.List[object]]::new()
foreach ($file in $files) {
    $full = Join-Path $folder $file
    if (-not (Test-Path -LiteralPath $full -PathType Leaf)) {
        continue
    }
    $settings = $both
    if (Test-SevenOnly -Path $full) {
        $settings = $seven
    }
    foreach ($record in Invoke-Analyzer -Path $full -Settings $settings) {
        $found.Add($record)
    }
}
foreach ($record in $found) {
    $path = [IO.Path]::GetRelativePath($folder, $record.ScriptPath) -replace '\\', '/'
    $text = '{0}:{1}:{2}: {3} {4}' -f $path, $record.Line, $record.Column, $record.RuleName, $record.Message
    [Console]::Out.WriteLine($text)
}
if ($found.Count -gt 0) {
    exit 1
}
exit 0
