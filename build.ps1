param(
    # Release.yml passes the bare git tag (no leading "v") so the published tag,
    # zip filename, and embedded assembly version stay in sync. Local builds
    # leave this empty and get an auto-derived YYYY.M.D.N-XXXX dev stamp.
    [string]$Version = "",

    # Build the GitHub release zip + manifest. Local builds leave this off so
    # the repo root stays clean and only dist/ is refreshed.
    [switch]$Package,

    # Deprecated compatibility switch. Equivalent to leaving -Package off.
    [switch]$SkipZip,

    # Optional directory for package artifacts. Defaults to dist/ so all local
    # build output stays under one ignored tree.
    [string]$ArtifactsDir = ""
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

# Activate the repo's tracked git hooks (.githooks/) on first build in a clone.
# Idempotent; harmlessly no-ops outside a git checkout.
try { & git config --local core.hooksPath .githooks 2>$null } catch {}

$BuildDir = Join-Path $PSScriptRoot "dist"
$StateFile = Join-Path $PSScriptRoot ".local_build_state.json"
$CreatePackage = $Package -and -not $SkipZip

# --- Versioning ---
if ($Version) {
    # Accepted shapes:
    #   YYYY.M.D.N            release
    #   YYYY.M.D.N-XXXX       local dev rebuild (4 hex)
    #   YYYY.M.D.N-beta       prerelease (matches release.yml's '*-beta*'
    #                         detector which adds --prerelease on the
    #                         GitHub release)
    # For a second prerelease against the same numeric base, bump the
    # patch instead of numbering the suffix (e.g. 2026.5.22.1-beta).
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
# AssemblyVersion must be pure numeric (no -XXXX dev suffix)
$AsmVersion = ($FullVersion -split '-')[0]
$VersionFile = Join-Path $PSScriptRoot "version.txt"
[System.IO.File]::WriteAllText($VersionFile, $FullVersion, [System.Text.UTF8Encoding]::new($false))
Write-Host "Building Version: $FullVersion" -ForegroundColor Magenta

# --- Clean dist ---
if (Test-Path $BuildDir) { Remove-Item $BuildDir -Recurse -Force }
New-Item -ItemType Directory $BuildDir -Force | Out-Null

# --- BuildInfo stamp (git short-SHA + build timestamp) ---
# Stamps the watchdog startup banner so the operator can correlate a
# running watchdog to a specific source revision without parsing the
# version number. Values are passed to MSBuild and generated under obj/,
# so builds do not modify tracked source files.
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

# IsDevBuild gates verbose [relay] req= logging in LocalRelayServer. The
# CI-produced release artifact is always treated as release mode (the only
# way to get a dev-mode CI build is to push a -XXXX-suffixed tag, which is
# vanishingly rare). CI's release.yml runs the changelog-promotion step
# before build.ps1, which leaves CHANGELOG.md modified -- a clean release
# tag would otherwise trip the dirty check and ship a verbose-logging
# binary. GITHUB_ACTIONS=true overrides the dirty check there.
#
# Local builds with a dirty tree DO get dev-mode -- they're prerelease
# experiments and the verbose log is the whole point.
if ($env:GITHUB_ACTIONS -eq 'true') {
    $isDev = $FullVersion -match '-'
}
else {
    $isDev = ($FullVersion -match '-') -or ($gitDirty -ne "")
}
$isDevLiteral = if ($isDev) { "true" } else { "false" }
Write-Host "BuildInfo: GitSha=$gitSha$gitDirty BuildTime=$buildTime IsDevBuild=$isDevLiteral" -ForegroundColor DarkGray

$BuildInfoProps = @(
    "/p:BuildInfoGitSha=$gitSha$gitDirty",
    "/p:BuildInfoBuildTime=$buildTime",
    "/p:BuildInfoIsDevBuild=$isDevLiteral"
)

# --- Publish .NET projects ---
# The four exes split into two publish profiles:
#   * Watchdog (vrcresolver.exe) — regular self-contained single-file with R2R.
#     Long-lived process; size doesn't matter as much; AOT audit pending.
#   * Updater + Uninstaller — AOT (csproj sets PublishAot/Trimmed/full).
#     Single native exe each; PublishSingleFile is incompatible with AOT
#     so the cmdline arg is split out into the watchdog-only PubArgs below.
#   * Wrapper (yt-dlp.exe in dist/tools/) — AOT, published separately
#     into dist/tools/ further down.
#
# Three of the four publishes (Updater, Uninstaller, YtDlp wrapper) run
# AOT, which means MSBuild's AOT target needs link.exe from MSVC. It looks
# up link.exe via vswhere.exe but vswhere isn't on PATH by default — VS
# Build Tools install it at %ProgramFiles(x86)%\Microsoft Visual Studio\
# Installer but the installer doesn't add the dir to PATH. Front-load the
# PATH munge once before any publish runs.
Write-Host "`n--- Publishing ---" -ForegroundColor Cyan
$ProgFilesX86 = [Environment]::GetEnvironmentVariable('ProgramFiles(x86)')
$VsInstaller = Join-Path $ProgFilesX86 'Microsoft Visual Studio\Installer'
if (Test-Path (Join-Path $VsInstaller 'vswhere.exe')) {
    if ($env:PATH -notlike "*$VsInstaller*") { $env:PATH = "$VsInstaller;$env:PATH" }
}
else {
    Write-Warning "vswhere.exe not found at $VsInstaller -- AOT link step may fail. Install Visual Studio Build Tools with the Desktop C++ workload."
}

# All four watchdog/Updater/Uninstaller/YtDlp now AOT-publish. The
# WatchdogPubArgs profile (single-file, R2R, JIT) is no longer used --
# AOT produces a single native exe inherently and PublishSingleFile is
# incompatible with PublishAot. Same $AotPubArgs for all of them.
$AotPubArgs = @("-c", "Release", "-r", "win-x64", "--self-contained", "true",
    "/p:Version=$AsmVersion",
    "-o", $BuildDir, "--nologo")
$WatchdogPubArgs = $AotPubArgs + $BuildInfoProps
dotnet publish "src/VrcResolver/VrcResolver.csproj" @WatchdogPubArgs
if ($LASTEXITCODE -ne 0) { throw "VrcResolver publish failed" }
dotnet publish "src/VrcResolver.Updater/VrcResolver.Updater.csproj" @AotPubArgs
if ($LASTEXITCODE -ne 0) { throw "VrcResolver.Updater publish failed" }
$UpdaterExe = Join-Path $BuildDir "vrcresolver.Updater.exe"
if (Test-Path $UpdaterExe) {
    Copy-Item $UpdaterExe (Join-Path $BuildDir "vrcresolver.Updater.next.exe") -Force
}
else {
    throw "vrcresolver.Updater.exe missing after publish"
}
dotnet publish "src/VrcResolver.Uninstaller/VrcResolver.Uninstaller.csproj" @AotPubArgs
if ($LASTEXITCODE -ne 0) { throw "VrcResolver.Uninstaller publish failed" }

# Clear the pre-rename launchers out of an existing output directory. The
# rename transition is over and they are no longer built; without this a dist
# from an older build keeps serving them up.
foreach ($StaleName in @("WKVRCProxy.exe", "WKVRCProxy.Updater.exe", "WKVRCProxy.Updater.next.exe")) {
    $StalePath = Join-Path $BuildDir $StaleName
    if (Test-Path $StalePath) { Remove-Item $StalePath -Force }
}

# --- Stage tools/ subdir in dist ---
$BuildTools = Join-Path $BuildDir "tools"
New-Item -ItemType Directory $BuildTools -Force | Out-Null

# Publish the patched yt-dlp wrapper directly into dist/tools/. AssemblyName=yt-dlp
# in the project produces yt-dlp.exe; PatchManager copies this over VRChat's
# Tools/yt-dlp.exe at runtime.
#
# AOT publish for the wrapper specifically (csproj sets PublishAot/PublishTrimmed
# /TrimMode=full/InvariantGlobalization). Cuts the wrapper from ~79 MB to ~3 MB
# and removes JIT cold-start cost — VRChat invokes this binary per video player
# so size + startup directly impact in-game stutter on world load. The watchdog
# stays on regular self-contained .NET 10 (one-shot launch; size doesn't matter).
#
# Drops PublishSingleFile (AOT incompatible — produces a single native .exe
# inherently). vswhere.exe is already on PATH from the front-loaded munge
# at the top of the publish section.
$YtDlpPubArgs = @("-c", "Release", "-r", "win-x64", "--self-contained", "true",
    "/p:Version=$AsmVersion",
    "-o", $BuildTools, "--nologo")
dotnet publish "src/VrcResolver.YtDlp/VrcResolver.YtDlp.csproj" @YtDlpPubArgs
if ($LASTEXITCODE -ne 0) { throw "VrcResolver.YtDlp publish failed" }

# AOT publish leaves a yt-dlp.pdb (~14 MB) we don't need to ship — the .pdb
# strip below picks it up.

# --- Stage data/ subdir in dist (known wrapper hashes list) ---
# Ships `data/wrapper_hashes.txt` alongside vrcresolver.exe so the watchdog
# can identify older wrapper binaries that pre-date the current install.
# The file is repo-tracked and maintained by the release workflow (one SHA
# appended per published release). Dev builds carry whatever state the
# working tree has and rely on the PE-metadata + embedded marker signals
# for self-identification.
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

# --- Trim debug symbols we don't ship ---
Get-ChildItem $BuildDir -Filter "*.pdb" -Recurse | Remove-Item -Force -ErrorAction SilentlyContinue

# --- Stage shipped-file manifest in dist ---
# The updater uses this manifest on the next update to remove files that were
# shipped by an older release but no longer exist in the new release. Unknown
# user-created files are not listed here, so they are left alone.
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

# --- Package artifacts for GitHub releases ---
# Local builds stop at dist/. GitHub release builds pass -Package, which writes
# the zip + manifest into dist/ (or -ArtifactsDir) without creating a repo-root
# release/ directory.
if ($CreatePackage) {
    $ArtifactRoot = if ($ArtifactsDir) { $ArtifactsDir } else { $BuildDir }
    if (-not [System.IO.Path]::IsPathRooted($ArtifactRoot)) {
        $ArtifactRoot = Join-Path $PSScriptRoot $ArtifactRoot
    }
    if (-not (Test-Path $ArtifactRoot)) { New-Item -ItemType Directory $ArtifactRoot -Force | Out-Null }

    $ManifestPath = Join-Path $ArtifactRoot "vrcresolver-v$FullVersion.manifest.tsv"
    $ZipPath = Join-Path $ArtifactRoot "vrcresolver-v$FullVersion.zip"
    if (Test-Path $ZipPath) { Remove-Item $ZipPath -Force }

    # Manifest is tab-separated: <sha256>\t<size_bytes>\t<relative_path>.
    # Generate-ReleaseNotes.ps1 reads this to compose the inner-file hash table
    # without needing to unzip the artifact. Capture package inputs before
    # writing the manifest so the manifest itself never appears in the zip.
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
