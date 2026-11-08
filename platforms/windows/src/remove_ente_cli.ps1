# Takes the folder Orbit installed Ente's tool into off the user's PATH, and leaves every other entry
# as it was written. Started by the Remove button in Settings. Reads ORBIT_INSTALL_DIR.
$ErrorActionPreference = 'Stop'
$dir = $env:ORBIT_INSTALL_DIR
$key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
try {
    $path = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $entries = $path -split ';'
    if ($entries -contains $dir) {
        $kept = @($entries | Where-Object { $_ -ne $dir })
        $key.SetValue('Path', ($kept -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString)
        # Tells other programs that the environment changed, which writing the key doesn't.
        [Environment]::SetEnvironmentVariable('OrbitEnvironmentChanged', $null, 'User')
    }
} finally {
    $key.Close()
}
