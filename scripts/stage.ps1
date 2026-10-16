# Collects everything the setup carries into dist-stage and builds the NSIS setup.
#   scripts\stage.ps1            build the release binaries first, then stage and pack
#   scripts\stage.ps1 -NoBuild   stage what is already built
param([switch]$NoBuild)

$ErrorActionPreference = 'Stop'
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
$root = Split-Path -Parent $PSScriptRoot
$stage = Join-Path $root 'dist-stage'
$appTauri = Join-Path $root 'app\src-tauri'

if (-not $NoBuild) {
    Push-Location $root
    cargo build --release -p klip-recorder
    if ($LASTEXITCODE -ne 0) { throw 'recorder build failed' }
    Pop-Location
    if (Test-Path (Join-Path $root 'app\package.json')) {
        # Only the Tauri build embeds the frontend. A plain cargo build gives a binary
        # that looks for the dev server and shows an empty window.
        Push-Location (Join-Path $root 'app')
        npx tauri build --no-bundle
        if ($LASTEXITCODE -ne 0) { throw 'app build failed' }
        Pop-Location
    }
}

# Start from an empty stage so nothing stale gets packed.
if (Test-Path $stage) {
    Get-ChildItem $stage -Recurse -File | ForEach-Object { [IO.File]::Delete($_.FullName) }
}
New-Item -ItemType Directory -Force (Join-Path $stage 'fonts') | Out-Null

Copy-Item (Join-Path $root 'target\release\klip-recorder.exe') $stage

# The Tauri build names the app Klip.exe. klip-app.exe next to it is the dev binary.
$appExe = Join-Path $appTauri 'target\release\Klip.exe'
if (Test-Path $appExe) { Copy-Item $appExe (Join-Path $stage 'Klip.exe') }
else { Write-Warning 'no app binary found, the setup will contain the recorder only' }

# Fonts for the overlay, the same files the app ships. Latin subsets only.
$fontsource = Join-Path $root 'app\node_modules\@fontsource'
if (Test-Path $fontsource) {
    Get-ChildItem $fontsource -Recurse -Include *.woff2 -File |
        Where-Object { $_.Name -match '-latin-(400|500|600|700)-normal\.woff2$' } |
        Copy-Item -Destination (Join-Path $stage 'fonts')
    Get-ChildItem $fontsource -Directory | ForEach-Object {
        $license = Join-Path $_.FullName 'LICENSE'
        if (Test-Path $license) { Copy-Item $license (Join-Path $stage "fonts\LICENSE-$($_.Name).txt") }
    }
}

# The setup is an NSIS installer (installer\nsis\klip.nsi). makensis comes from tools\nsis
# in the project or from an NSIS installation.
$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$makensis = @(
    (Join-Path $root 'tools\nsis\makensis.exe'),
    "${env:ProgramFiles(x86)}\NSIS\makensis.exe",
    "$env:ProgramFiles\NSIS\makensis.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $makensis) { throw 'makensis not found: install NSIS or put it into tools\nsis' }
$setup = Join-Path $root 'target\release\Klip-Setup.exe'
& $makensis /V2 "/DVERSION=$version" "/DSTAGE=$stage" "/DOUTFILE=$setup" (Join-Path $root 'installer\nsis\klip.nsi')
if ($LASTEXITCODE -ne 0) { throw 'setup build failed' }

'{0}  {1:N1} MB, version {2}' -f $setup, ((Get-Item $setup).Length / 1MB), $version
Get-ChildItem $stage -Recurse -File | ForEach-Object { '  {0,-40} {1,10:N0} KB' -f $_.FullName.Substring($stage.Length + 1), ($_.Length / 1KB) }
