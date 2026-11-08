# Installs Ente's command-line tool the way Ente's own instructions describe: the release archive
# from GitHub, checked against the release's SHA256SUMS. Started by the Install button in Settings.
# Reads ORBIT_INSTALL_DIR, and ORBIT_ADD_TO_PATH=1 to add that folder to the user's PATH.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$dir = $env:ORBIT_INSTALL_DIR
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'amd64' }
$headers = @{ 'User-Agent' = 'Orbit' }

# The newest CLI release. Releases of Ente's other apps are in the same list. If the list can't be
# read, this version is known to work.
$tag = 'ente-cli-v0.4.3'
try {
    $releases = Invoke-RestMethod -UseBasicParsing -Headers $headers 'https://api.github.com/repos/ente/ente/releases?per_page=100'
    foreach ($release in $releases) {
        if ($release.tag_name -like 'ente-cli-v*' -and -not $release.prerelease -and -not $release.draft) {
            $tag = $release.tag_name
            break
        }
    }
} catch {}

$base = "https://github.com/ente/ente/releases/download/$tag"
$archive = "$tag-windows-$arch.zip"
$temp = Join-Path $env:TEMP ('orbit-ente-' + [guid]::NewGuid())
New-Item -ItemType Directory -Force $temp | Out-Null
try {
    Invoke-WebRequest -UseBasicParsing -Headers $headers "$base/$archive" -OutFile "$temp\$archive"
    Invoke-WebRequest -UseBasicParsing -Headers $headers "$base/SHA256SUMS" -OutFile "$temp\SHA256SUMS"
    $line = Select-String -Path "$temp\SHA256SUMS" -SimpleMatch $archive | Select-Object -First 1
    $expected = if ($line) { ($line.Line -split '\s+')[0] } else { '' }
    $actual = (Get-FileHash "$temp\$archive" -Algorithm SHA256).Hash
    if (-not $expected -or $expected -ine $actual) {
        throw 'The download did not match its published checksum, so it was not installed.'
    }
    Expand-Archive "$temp\$archive" "$temp\unpacked"
    New-Item -ItemType Directory -Force $dir | Out-Null
    Copy-Item "$temp\unpacked\ente.exe" "$dir\ente.exe" -Force
    if ($env:ORBIT_ADD_TO_PATH -eq '1') {
        # Read and written unexpanded, so entries like %USERPROFILE%\bin stay as they are.
        $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
        try {
            $path = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            if (($path -split ';') -notcontains $dir) {
                $key.SetValue('Path', ($dir + ';' + $path).TrimEnd(';'), [Microsoft.Win32.RegistryValueKind]::ExpandString)
                # Tells other programs that the environment changed, which writing the key doesn't.
                [Environment]::SetEnvironmentVariable('OrbitEnvironmentChanged', $null, 'User')
            }
        } finally {
            $key.Close()
        }
    }
    Write-Output $tag
} finally {
    Remove-Item -Recurse -Force $temp -ErrorAction SilentlyContinue
}
