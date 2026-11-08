# Builds a release in dist/:
#
#   pwsh scripts/package-windows.ps1
#
#   dist/Orbit-<version>-Setup.exe     the installer, which most people want
#   dist/Orbit-<version>-Portable.exe  runs from anywhere and keeps its settings beside itself
#
# The installer needs Inno Setup 6: winget install JRSoftware.InnoSetup
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

    # Portable: the same exe. "Portable" in its name is what makes it keep settings beside itself.
    $portable = Join-Path $dist "Orbit-$version-Portable.exe"
    Copy-Item target\release\orbit.exe $portable

    $iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe") |
        Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $iscc) { throw "Inno Setup 6 isn't installed: winget install JRSoftware.InnoSetup" }
    & $iscc /Q "/DAppVersion=$version" "/DSourceExe=$root\target\release\orbit.exe" installer\orbit.iss
    if ($LASTEXITCODE -ne 0) { throw 'The installer build failed.' }

    "wrote $(Join-Path $dist "Orbit-$version-Setup.exe")"
    "wrote $portable"
} finally { Pop-Location }
