param(
    [string]$Version = "",

    [switch]$Package,

    [switch]$SkipZip,

    [string]$ArtifactsDir = ""
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

try { & git config --local core.hooksPath .githooks 2>$null } catch {}

$BuildDir = Join-Path $PSScriptRoot "dist"
$StateFile = Join-Path $PSScriptRoot ".local_build_state.json"
$CreatePackage = $Package -and -not $SkipZip

if ($Version) {
    if ($Version -notmatch '^\d{4}\.\d+\.\d+\.\d+(-([A-Fa-f0-9]{4}|beta))?$') {
        throw "Invalid -Version '$Version'. Expected YYYY.M.D.N (release), YYYY.M.D.N-XXXX (dev), or YYYY.M.D.N-beta (prerelease)."
    }
    $FullVersion = $Version
}
else {
    $Today = Get-Date -Format "yyyy.M.d"
    $BuildCount = 0
    if (Test-Path $StateFile) {
        $State = Get-Content $StateFile | ConvertFrom-Json
        if ($State.Date -eq $Today) { $BuildCount = [int]$State.Count + 1 }
    }
    $UID = [Guid]::NewGuid().ToString().Substring(0, 4).ToUpper()
    $FullVersion = "$Today.$BuildCount-$UID"
    @{ Date = $Today; Count = $BuildCount } | ConvertTo-Json | Out-File $StateFile -Encoding utf8
}
$AsmVersion = ($FullVersion -split '-')[0]
$VersionFile = Join-Path $PSScriptRoot "version.txt"
[System.IO.File]::WriteAllText($VersionFile, $FullVersion, [System.Text.UTF8Encoding]::new($false))
Write-Host "Building Version: $FullVersion" -ForegroundColor Magenta

if (Test-Path $BuildDir) { Remove-Item $BuildDir -Recurse -Force }
New-Item -ItemType Directory $BuildDir -Force | Out-Null

$gitSha = "<unknown>"
try {
    $resolved = & git rev-parse --short=12 HEAD 2>$null
    if ($LASTEXITCODE -eq 0 -and $resolved) { $gitSha = $resolved.Trim() }
}
catch { }
$gitDirty = ""
try {
    $statusOut = & git status --porcelain 2>$null
    if ($LASTEXITCODE -eq 0 -and $statusOut) { $gitDirty = "-dirty" }
}
catch { }
$buildTime = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")

$dirtyLocal = ($env:GITHUB_ACTIONS -ne 'true') -and ($gitDirty -ne "")
if ($FullVersion -match '-beta$') {
    $channel = "nightly"
}
elseif (($FullVersion -match '-') -or $dirtyLocal) {
    $channel = "dev"
}
else {
    $channel = "release"
}
Write-Host "BuildInfo: GitSha=$gitSha$gitDirty BuildTime=$buildTime Channel=$channel" -ForegroundColor DarkGray

Write-Host "`n--- Building ---" -ForegroundColor Cyan
$env:VRCR_VERSION = $AsmVersion
$env:VRCR_GIT_SHA = "$gitSha$gitDirty"
$env:VRCR_BUILD_TIME = $buildTime
$env:VRCR_CHANNEL = $channel
cargo build --release --locked -p vrcr-watchdog -p vrcr-updater -p vrcr-uninstaller -p vrcr-ytdlp
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $PSScriptRoot "target" }
$ReleaseDir = Join-Path $TargetDir "release"
$BuildTools = Join-Path $BuildDir "tools"
New-Item -ItemType Directory $BuildTools -Force | Out-Null

$Outputs = [ordered]@{
    "vrcresolver.exe"             = "vrcresolver.exe"
    "vrcresolver-updater.exe"     = "vrcresolver.Updater.exe"
    "vrcresolver-uninstaller.exe" = "vrcresolver.Uninstaller.exe"
    "yt-dlp.exe"                  = "tools/yt-dlp.exe"
}
foreach ($Name in $Outputs.Keys) {
    $Source = Join-Path $ReleaseDir $Name
    if (-not (Test-Path $Source)) { throw "$Name missing after cargo build" }
    Copy-Item $Source (Join-Path $BuildDir $Outputs[$Name]) -Force
}
Copy-Item (Join-Path $BuildDir "vrcresolver.Updater.exe") (Join-Path $BuildDir "vrcresolver.Updater.next.exe") -Force

foreach ($Exe in Get-ChildItem $BuildDir -Filter "*.exe" -Recurse) {
    $Stamped = $Exe.VersionInfo.FileVersion
    if ($Stamped -ne $AsmVersion) { throw "$($Exe.Name) carries FileVersion '$Stamped', expected '$AsmVersion'" }
}

$BuildData = Join-Path $BuildDir "data"
New-Item -ItemType Directory $BuildData -Force | Out-Null
$KnownHashesSrc = Join-Path $PSScriptRoot "data/wrapper_hashes.txt"
if (Test-Path $KnownHashesSrc) {
    Copy-Item $KnownHashesSrc (Join-Path $BuildData "wrapper_hashes.txt") -Force
}
else {
    Write-Warning "data/wrapper_hashes.txt missing from repo -- shipping empty list"
    "" | Out-File (Join-Path $BuildData "wrapper_hashes.txt") -Encoding utf8 -NoNewline
}


$InstallManifestPath = Join-Path $BuildData "release-manifest.tsv"
$InstallManifestLines = Get-ChildItem $BuildDir -Recurse -File |
    Where-Object { $_.FullName -ne $InstallManifestPath } |
    Sort-Object FullName |
    ForEach-Object {
        $relPath = $_.FullName.Substring($BuildDir.Length + 1) -replace '\\', '/'
        $sha = (Get-FileHash $_.FullName -Algorithm SHA256).Hash
        "$sha`t$($_.Length)`t$relPath"
    }
[System.IO.File]::WriteAllLines($InstallManifestPath, [string[]]$InstallManifestLines, [System.Text.UTF8Encoding]::new($false))

if ($CreatePackage) {
    $ArtifactRoot = if ($ArtifactsDir) { $ArtifactsDir } else { $BuildDir }
    if (-not [System.IO.Path]::IsPathRooted($ArtifactRoot)) {
        $ArtifactRoot = Join-Path $PSScriptRoot $ArtifactRoot
    }
    if (-not (Test-Path $ArtifactRoot)) { New-Item -ItemType Directory $ArtifactRoot -Force | Out-Null }

    $ManifestPath = Join-Path $ArtifactRoot "vrcresolver-v$FullVersion.manifest.tsv"
    $ZipPath = Join-Path $ArtifactRoot "vrcresolver-v$FullVersion.zip"
    if (Test-Path $ZipPath) { Remove-Item $ZipPath -Force }

    $PackageFiles = Get-ChildItem $BuildDir -Recurse -File |
        Sort-Object FullName
    $manifestLines = $PackageFiles | ForEach-Object {
        $relPath = $_.FullName.Substring($BuildDir.Length + 1) -replace '\\', '/'
        $sha = (Get-FileHash $_.FullName -Algorithm SHA256).Hash
        "$sha`t$($_.Length)`t$relPath"
    }
    $PackageInputs = Get-ChildItem $BuildDir -Force | ForEach-Object { $_.FullName }
    [System.IO.File]::WriteAllLines($ManifestPath, [string[]]$manifestLines, [System.Text.UTF8Encoding]::new($false))
    Write-Host "Manifest: $ManifestPath ($($manifestLines.Count) files)" -ForegroundColor Cyan

    Compress-Archive -Path $PackageInputs -DestinationPath $ZipPath
    Write-Host "`nRelease zip: $ZipPath" -ForegroundColor Green
}

Write-Host "`nBuild complete: v$FullVersion" -ForegroundColor Green
