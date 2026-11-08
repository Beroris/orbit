# Removes Orbit from this Windows account. Orbit installs no service, driver or scheduled task, so
# what's here is everything: stop the process, remove the sign-in entry, and optionally the data.
#
# Your notes and snippets stay unless you pass -DeleteData. Use "Export Orbit Data" first if you
# want a copy. The folder holding orbit.exe is yours to delete afterwards.
param([switch]$DeleteData)

Get-Process orbit -ErrorAction SilentlyContinue | ForEach-Object {
    # Orbit's window is usually hidden, and then this has no effect and the process is stopped
    # instead. That's safe: its hook goes with it, and saved data is already on disk.
    [void]$_.CloseMainWindow()
    if (-not $_.WaitForExit(3000)) { Stop-Process -Id $_.Id -Force }
}

Remove-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'Orbit' -ErrorAction SilentlyContinue

$data = Join-Path $env:LOCALAPPDATA 'Orbit'
if ($DeleteData) {
    if (Test-Path $data) {
        Remove-Item $data -Recurse -Force
        "Deleted $data"
    }
} elseif (Test-Path $data) {
    "Your data is still in $data. Run this again with -DeleteData to remove it."
}
"Orbit won't start at sign-in any more. Caps Lock and every other key are back to normal."
