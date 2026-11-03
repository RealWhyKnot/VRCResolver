#!/usr/bin/env pwsh
[CmdletBinding()]
param(
    [string]$Version = '',

    [switch]$Package,

    [switch]$SkipZip,

    [string]$ArtifactsDir = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Set-Location $PSScriptRoot

& { $ErrorActionPreference = 'Continue'; git config --local core.hooksPath .githooks 2>$null }
if ($LASTEXITCODE -ne 0) { Write-Warning 'Could not set the local hooks path.' }

$BuildDir = Join-Path $PSScriptRoot 'dist'
$StateFile = Join-Path $PSScriptRoot '.local_build_state.json'
$CreatePackage = $Package -and -not $SkipZip

if ($Version) {
    if ($Version -notmatch '^\d{4}\.\d+\.\d+\.\d+(-([A-Fa-f0-9]{4}|beta))?$') {
        throw (
            "Invalid -Version '$Version'. Expected YYYY.M.D.N (release), YYYY.M.D.N-XXXX (dev), or " +
            'YYYY.M.D.N-beta (prerelease).'
        )
    }
    $FullVersion = $Version
} else {
    $Today = Get-Date -Format 'yyyy.M.d'
    $BuildCount = 0
    if (Test-Path $StateFile) {
        $State = [IO.File]::ReadAllText($StateFile) | ConvertFrom-Json
        $stateDate = $State.PSObject.Properties['Date']
        $stateCount = $State.PSObject.Properties['Count']
        if ($stateDate -and $stateDate.Value -eq $Today -and $stateCount) {
            $BuildCount = [int]$stateCount.Value + 1
        }
    }
    $UID = [Guid]::NewGuid().ToString().Substring(0, 4).ToUpper()
    $FullVersion = "$Today.$BuildCount-$UID"
    $stateJson = @{ Date = $Today; Count = $BuildCount } | ConvertTo-Json -Depth 4
    [IO.File]::WriteAllText($StateFile, $stateJson + [Environment]::NewLine)
}
$AsmVersion = ($FullVersion -split '-')[0]
$VersionFile = Join-Path $PSScriptRoot 'version.txt'
[System.IO.File]::WriteAllText($VersionFile, $FullVersion, [System.Text.UTF8Encoding]::new($false))
Write-Host "Building Version: $FullVersion" -ForegroundColor Magenta

if (Test-Path -LiteralPath $BuildDir) {
    $resolvedBuildDir = (Resolve-Path -LiteralPath $BuildDir).Path
    $expectedBuildDir = [IO.Path]::GetFullPath((Join-Path -Path $PSScriptRoot -ChildPath 'dist'))
    if ($resolvedBuildDir -ne $expectedBuildDir) { throw 'Build directory resolves outside this checkout.' }
    Remove-Item -LiteralPath $resolvedBuildDir -Recurse -Force
}
$null = New-Item -ItemType Directory $BuildDir -Force

$gitSha = '<unknown>'
$resolved = & { $ErrorActionPreference = 'Continue'; git rev-parse --short=12 HEAD 2>$null }
if ($LASTEXITCODE -eq 0 -and $resolved) { $gitSha = $resolved.Trim() }
$gitDirty = ''
$statusOut = & { $ErrorActionPreference = 'Continue'; git status --porcelain 2>$null }
if ($LASTEXITCODE -eq 0 -and $statusOut) { $gitDirty = '-dirty' }
$buildTime = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')

$dirtyLocal = ($env:GITHUB_ACTIONS -ne 'true') -and ($gitDirty -ne '')
if ($FullVersion -match '-beta$') {
    $channel = 'nightly'
} elseif (($FullVersion -match '-') -or $dirtyLocal) {
    $channel = 'dev'
} else {
    $channel = 'release'
}
Write-Host "BuildInfo: GitSha=$gitSha$gitDirty BuildTime=$buildTime Channel=$channel" -ForegroundColor DarkGray

Write-Host "`n--- Building ---" -ForegroundColor Cyan
$env:VRCR_VERSION = $AsmVersion
$env:VRCR_GIT_SHA = "$gitSha$gitDirty"
$env:VRCR_BUILD_TIME = $buildTime
$env:VRCR_CHANNEL = $channel
cargo build --release --locked -p vrcr-watchdog -p vrcr-updater -p vrcr-uninstaller -p vrcr-ytdlp
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $PSScriptRoot 'target' }
$ReleaseDir = Join-Path $TargetDir 'release'
$BuildTools = Join-Path $BuildDir 'tools'
$null = New-Item -ItemType Directory $BuildTools -Force

$Outputs = [ordered]@{
    'vrcresolver.exe'             = 'vrcresolver.exe'
    'vrcresolver-updater.exe'     = 'vrcresolver.Updater.exe'
    'vrcresolver-uninstaller.exe' = 'vrcresolver.Uninstaller.exe'
    'yt-dlp.exe'                  = 'tools/yt-dlp.exe'
}
foreach ($Name in $Outputs.Keys) {
    $Source = Join-Path $ReleaseDir $Name
    if (-not (Test-Path $Source)) { throw "$Name missing after cargo build" }
    Copy-Item $Source (Join-Path $BuildDir $Outputs[$Name]) -Force
}
Copy-Item (Join-Path $BuildDir 'vrcresolver.Updater.exe') (Join-Path $BuildDir 'vrcresolver.Updater.next.exe') -Force

foreach ($Exe in Get-ChildItem $BuildDir -Filter '*.exe' -Recurse) {
    $Stamped = $Exe.VersionInfo.FileVersion
    if ($Stamped -ne $AsmVersion) { throw "$($Exe.Name) carries FileVersion '$Stamped', expected '$AsmVersion'" }
}

$BuildData = Join-Path $BuildDir 'data'
$null = New-Item -ItemType Directory $BuildData -Force
$KnownHashesSrc = Join-Path $PSScriptRoot 'data/wrapper_hashes.txt'
if (Test-Path $KnownHashesSrc) {
    Copy-Item $KnownHashesSrc (Join-Path $BuildData 'wrapper_hashes.txt') -Force
} else {
    Write-Warning 'data/wrapper_hashes.txt missing from repo -- shipping empty list'
    [IO.File]::WriteAllText((Join-Path -Path $BuildData -ChildPath 'wrapper_hashes.txt'), '')
}

$InstallManifestPath = Join-Path $BuildData 'release-manifest.tsv'
$InstallManifestLines = Get-ChildItem $BuildDir -Recurse -File |
    Where-Object { $_.FullName -ne $InstallManifestPath } |
    Sort-Object FullName |
    ForEach-Object {
        $relPath = $_.FullName.Substring($BuildDir.Length + 1) -replace '\\', '/'
        $sha = (Get-FileHash $_.FullName -Algorithm SHA256).Hash
        "$sha`t$($_.Length)`t$relPath"
    }
[System.IO.File]::WriteAllLines(
    $InstallManifestPath, [string[]]$InstallManifestLines, [System.Text.UTF8Encoding]::new($false)
)

if ($CreatePackage) {
    $ArtifactRoot = if ($ArtifactsDir) { $ArtifactsDir } else { $BuildDir }
    if (-not [System.IO.Path]::IsPathRooted($ArtifactRoot)) {
        $ArtifactRoot = Join-Path $PSScriptRoot $ArtifactRoot
    }
    if (-not (Test-Path $ArtifactRoot)) { $null = New-Item -ItemType Directory $ArtifactRoot -Force }

    $ManifestPath = Join-Path $ArtifactRoot "vrcresolver-v$FullVersion.manifest.tsv"
    $ZipPath = Join-Path $ArtifactRoot "vrcresolver-v$FullVersion.zip"
    if (Test-Path $ZipPath) { Remove-Item $ZipPath -Force }

    $PackageFiles = Get-ChildItem $BuildDir -Recurse -File |
        Sort-Object FullName
    $manifestLines = foreach ($packageFile in $PackageFiles) {
        $relPath = $packageFile.FullName.Substring($BuildDir.Length + 1) -replace '\\', '/'
        $sha = (Get-FileHash $packageFile.FullName -Algorithm SHA256).Hash
        "$sha`t$($packageFile.Length)`t$relPath"
    }
    $PackageInputs = Get-ChildItem $BuildDir -Force | ForEach-Object { $_.FullName }
    [System.IO.File]::WriteAllLines($ManifestPath, [string[]]$manifestLines, [System.Text.UTF8Encoding]::new($false))
    Write-Host "Manifest: $ManifestPath ($(@($manifestLines).Count) files)" -ForegroundColor Cyan

    Compress-Archive -Path $PackageInputs -DestinationPath $ZipPath
    Write-Host "`nRelease zip: $ZipPath" -ForegroundColor Green
}

Write-Host "`nBuild complete: v$FullVersion" -ForegroundColor Green

exit 0
