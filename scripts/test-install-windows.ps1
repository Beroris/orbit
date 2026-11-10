# Tests Orbit Setup without a window: a silent install, an update over a stand-in 1.0.0, a silent
# update that keeps sign-in off, a silent uninstall as Settings -> Apps runs it, an uninstall that
# leaves another copy of Orbit alone, and a reinstall.
#
# It runs in a sandbox in the temp folder: LOCALAPPDATA and APPDATA point there, so the install
# folder, the data folder and the Start menu shortcut are all inside it, and an Orbit you have
# running is never found or closed. The Settings -> Apps entry and the sign-in entry are real
# registry values, so the test refuses to run if Orbit is installed, and puts the sign-in entry
# back as it found it. Any run that shows a window is ended at once and the test stops.
#
#   pwsh scripts/test-install-windows.ps1                      # target\release\orbit.exe
#   pwsh scripts/test-install-windows.ps1 dist\Orbit-1.1.0-Setup.exe

param([string]$Exe = (Join-Path $PSScriptRoot '..\target\release\orbit.exe'))

$ErrorActionPreference = 'Stop'
$source = (Resolve-Path $Exe).Path
$version = (Get-Item $source).VersionInfo.ProductVersion
$sandbox = Join-Path ([IO.Path]::GetTempPath()) "orbit-install-test-$PID"
$local = Join-Path $sandbox 'Local'
$roaming = Join-Path $sandbox 'Roaming'
$installed = Join-Path $local 'Programs\Orbit'
$exePath = Join-Path $installed 'orbit.exe'
$setup = Join-Path $sandbox 'Orbit-test-Setup.exe'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{2124CE11-6877-42C6-B33E-D4C37E050DE4}'
$innoKey = "${uninstallKey}_is1"
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$shortcut = Join-Path $roaming 'Microsoft\Windows\Start Menu\Programs\Orbit.lnk'
$shell = New-Object -ComObject WScript.Shell
$script:fails = 0

if ((Test-Path $uninstallKey) -or (Test-Path $innoKey)) {
    throw 'Orbit is installed for this account. Uninstall it first: this test would remove its Settings -> Apps entry.'
}
$hadRun = $null -ne (Get-ItemProperty $runKey).PSObject.Properties['Orbit']
$theirRun = (Get-ItemProperty $runKey).Orbit

function Check([string]$what, [bool]$ok) {
    if ($ok) { "  ok   $what" } else { "  FAIL $what"; $script:fails++ }
}
function Value([string]$key, [string]$name) {
    try { (Get-ItemProperty $key -Name $name -ErrorAction Stop).$name } catch { $null }
}
function Target([string]$link) { if (Test-Path $link) { $shell.CreateShortcut($link).TargetPath } }
function Leftovers { Get-ChildItem ([IO.Path]::GetTempPath()) -File | Where-Object { $_.Name -like 'Orbit-uninstall-*' -or $_.Name -like 'Orbit-removed-*' } }
# The test's own Orbit processes: Setup and the installed exe in the sandbox, and the uninstall's
# copy in the temp folder. Never an Orbit started some other way.
function Ours { Get-Process | Where-Object { ($_.Path -and $_.Path.StartsWith($sandbox, 'OrdinalIgnoreCase')) -or $_.ProcessName -like 'Orbit-uninstall-*' } }

# Runs an Orbit exe with the sandbox's folders. Gives its exit code.
function Run([string]$program, [string[]]$arguments) {
    $info = [Diagnostics.ProcessStartInfo]::new($program)
    foreach ($argument in $arguments) { $info.ArgumentList.Add($argument) }
    $info.UseShellExecute = $false
    $info.Environment['LOCALAPPDATA'] = $local
    $info.Environment['APPDATA'] = $roaming
    $process = [Diagnostics.Process]::Start($info)
    $deadline = (Get-Date).AddSeconds(60)
    while (-not $process.HasExited) {
        $windowed = Ours | Where-Object { $_.MainWindowHandle -ne 0 }
        if ($windowed -or (Get-Date) -gt $deadline) {
            Ours | Stop-Process -Force -Confirm:$false
            throw "$(Split-Path -Leaf $program) $arguments showed a window or didn't finish, so it was ended."
        }
        Start-Sleep -Milliseconds 50
    }
    $process.ExitCode
}

"Testing Orbit Setup $version from $source"
try {
    New-Item -ItemType Directory -Force $local, $roaming | Out-Null
    Copy-Item $source $setup

    '1. Fresh silent install'
    $code = Run $setup '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'
    Check "exit code 0 (was $code)" ($code -eq 0)
    Check 'orbit.exe installed' (Test-Path $exePath)
    Check 'licence and notices beside it' ((Test-Path "$installed\Geist-LICENSE.txt") -and (Test-Path "$installed\THIRD_PARTY_NOTICES.md"))
    Check "in Settings -> Apps as Orbit $version" ((Value $uninstallKey DisplayName) -eq 'Orbit' -and (Value $uninstallKey DisplayVersion) -eq $version)
    Check 'its Uninstall runs orbit.exe --uninstall' ((Value $uninstallKey UninstallString) -eq "`"$exePath`" --uninstall")
    Check 'quiet uninstall' ((Value $uninstallKey QuietUninstallString) -eq "`"$exePath`" --uninstall --silent")
    Check 'icon and publisher' ((Value $uninstallKey DisplayIcon) -eq "$exePath,0" -and (Value $uninstallKey Publisher) -eq 'Beroris')
    Check 'starts at sign-in, as a fresh install does' ((Value $runKey Orbit) -eq "`"$exePath`"")
    Check 'Start menu shortcut to the installed exe' ((Target $shortcut) -eq $exePath)
    Check 'Orbit not started by a fresh silent install' (-not (Ours | Where-Object { $_.Path -eq $exePath }))

    '2. Silent update over a stand-in 1.0.0 from Inno Setup'
    Remove-Item $uninstallKey
    New-Item $innoKey -Force | Out-Null
    New-ItemProperty $innoKey -Name DisplayVersion -Value '1.0.0' | Out-Null
    Set-Content "$installed\unins000.exe" 'stand-in'
    Set-Content "$installed\unins000.dat" 'stand-in'
    $code = Run $setup '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'
    Check "exit code 0 (was $code)" ($code -eq 0)
    Check "Inno Setup's entry taken over" (-not (Test-Path $innoKey))
    Check 'its uninstaller removed' (-not (Test-Path "$installed\unins000.exe") -and -not (Test-Path "$installed\unins000.dat"))
    Check "one entry, now $version" ((Value $uninstallKey DisplayVersion) -eq $version)
    Check 'still starts at sign-in' ((Value $runKey Orbit) -eq "`"$exePath`"")

    '3. A silent update keeps sign-in turned off'
    Remove-ItemProperty $runKey -Name Orbit
    $code = Run $setup '--silent'
    Check "exit code 0 (was $code)" ($code -eq 0)
    Check 'no sign-in entry written' ($null -eq (Value $runKey Orbit))
    Set-ItemProperty $runKey -Name Orbit -Value "`"$exePath`""

    '4. Silent uninstall, as Settings -> Apps runs it'
    New-Item -ItemType Directory -Force "$local\Orbit" | Out-Null
    Set-Content "$local\Orbit\settings.json" '{}'
    $code = Run $exePath '--uninstall', '--silent'
    Check "exit code 0 (was $code)" ($code -eq 0)
    Check 'program folder gone' (-not (Test-Path $installed))
    Check 'gone from Settings -> Apps' (-not (Test-Path $uninstallKey))
    Check 'sign-in entry removed' ($null -eq (Value $runKey Orbit))
    Check 'Start menu shortcut removed' (-not (Test-Path $shortcut))
    Check 'data kept' (Test-Path "$local\Orbit\settings.json")
    Start-Sleep -Seconds 5
    Check 'the uninstall copy and the old exe deleted themselves' (-not (Leftovers))

    "5. Uninstall leaves another copy's shortcut and sign-in entry alone"
    New-Item -ItemType Directory -Force (Split-Path $shortcut) | Out-Null
    $link = $shell.CreateShortcut($shortcut); $link.TargetPath = 'C:\Elsewhere\orbit.exe'; $link.Save()
    Set-ItemProperty $runKey -Name Orbit -Value '"C:\Elsewhere\orbit.exe"'
    $code = Run $setup '--uninstall', '--silent'
    Check "exit code 0 (was $code)" ($code -eq 0)
    Check 'their shortcut stays' (Test-Path $shortcut)
    Check 'their sign-in entry stays' ((Value $runKey Orbit) -eq '"C:\Elsewhere\orbit.exe"')
    Check 'the Setup that ran it, in the temp folder, is still there' (Test-Path $setup)
    Remove-Item $shortcut

    '6. Reinstall, then uninstall again'
    $code = Run $setup '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'
    Check "reinstall exit code 0 (was $code)" ($code -eq 0)
    Check 'installed again' (Test-Path $exePath)
    Check 'in Settings -> Apps again' ((Value $uninstallKey DisplayVersion) -eq $version)
    $code = Run $exePath '--uninstall', '--silent'
    Check "uninstall exit code 0 (was $code)" ($code -eq 0)
    Check 'gone again' (-not (Test-Path $installed) -and -not (Test-Path $uninstallKey))
    Start-Sleep -Seconds 5
    Check 'nothing left in the temp folder' (-not (Leftovers))
} catch {
    "  STOPPED $($_.Exception.Message)"
    $script:fails++
} finally {
    '7. Putting things back'
    Ours | Stop-Process -Force -Confirm:$false -ErrorAction SilentlyContinue
    foreach ($key in $uninstallKey, $innoKey) { if (Test-Path $key) { Remove-Item $key -Recurse -Force } }
    if ($hadRun) { Set-ItemProperty $runKey -Name Orbit -Value $theirRun } else { Remove-ItemProperty $runKey -Name Orbit -ErrorAction SilentlyContinue }
    Check 'sign-in entry as it was' ($(if ($hadRun) { (Value $runKey Orbit) -eq $theirRun } else { $null -eq (Value $runKey Orbit) }))
    Check 'no Orbit in Settings -> Apps' (-not (Test-Path $uninstallKey) -and -not (Test-Path $innoKey))
    Check 'none of the test still running' (-not (Ours))
    Remove-Item $sandbox -Recurse -Force -ErrorAction SilentlyContinue
    "$script:fails failed"
}
exit [int]($script:fails -gt 0)
