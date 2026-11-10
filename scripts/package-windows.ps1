# Builds a release in dist/:
#
#   pwsh scripts/package-windows.ps1
#
#   dist/Orbit-<version>-Setup.exe     the installer, which most people want
#   dist/Orbit-<version>-Portable.exe  runs from anywhere and keeps its settings beside itself
#
# Both are orbit.exe under another name. Orbit installs itself: named "...-Setup.exe" it opens
# Orbit Setup, and named "...-Portable.exe" it keeps its settings in a folder beside itself.
# See platforms/windows/src/setup.rs and install.rs. There's nothing else to install to build them.
# The build is not code signed, so SmartScreen may warn on first run.
param([switch]$SkipTests)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    if (-not $SkipTests) {
        cargo test --workspace --release
        if ($LASTEXITCODE -ne 0) { throw 'Tests failed, so nothing was packaged.' }
    }
    cargo build --release -p orbit-windows
    if ($LASTEXITCODE -ne 0) { throw 'The build failed.' }

    $version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
    $dist = Join-Path $root 'dist'
    New-Item -ItemType Directory -Force $dist | Out-Null
    # Only earlier release files: dist can hold other things, such as a copy being tried out.
    Get-ChildItem $dist -File -Filter 'Orbit-*-*.exe' | Remove-Item -Force

    $setup = Join-Path $dist "Orbit-$version-Setup.exe"
    $portable = Join-Path $dist "Orbit-$version-Portable.exe"
    Copy-Item target\release\orbit.exe $setup
    Copy-Item target\release\orbit.exe $portable

    "wrote $setup"
    "wrote $portable"
} finally { Pop-Location }
