<#
.SYNOPSIS
  Removes WinShellTheme for the current user and restores the native taskbar.

.DESCRIPTION
  Asks the running shell to exit cleanly, restores the taskbar from any leftover
  state file, removes the autostart value and deletes the program folder.
  Settings in %APPDATA%\WinShellTheme are kept unless -RemoveSettings is given.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File .\uninstall.ps1
  powershell -ExecutionPolicy Bypass -File .\uninstall.ps1 -RemoveSettings
#>
[CmdletBinding()]
param([switch]$RemoveSettings)
$ErrorActionPreference = 'Stop'

foreach ($v in 'LOCALAPPDATA', 'APPDATA') {
    if ([string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($v))) { throw "$v is not set; aborting." }
}

$dest     = Join-Path $env:LOCALAPPDATA 'Programs\WinShellTheme'
$exe      = Join-Path $dest 'winshell.exe'
$settings = Join-Path $env:APPDATA 'WinShellTheme'

function Invoke-Winshell([string]$Argument) {
    $p = Start-Process -FilePath $exe -ArgumentList $Argument -Wait -PassThru -WindowStyle Hidden
    return $p.ExitCode
}

if (Test-Path -LiteralPath $exe -PathType Leaf) {
    Write-Host 'Stopping WinShellTheme (it restores the taskbar)...'
    if ((Invoke-Winshell '--quit') -ne 0) { throw 'WinShellTheme could not exit safely; uninstall cancelled.' }
    if ((Invoke-Winshell '--restore-taskbar') -ne 0) { throw 'Desktop restoration is incomplete; files kept for recovery.' }
    [void](Invoke-Winshell '--disable-autostart')
} else {
    Remove-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'WinShellTheme' -ErrorAction SilentlyContinue
}

# Only ever delete the exact folders this app created.
if ((Test-Path -LiteralPath $dest -PathType Container) -and ((Split-Path -Leaf $dest) -eq 'WinShellTheme')) {
    Remove-Item -LiteralPath $dest -Recurse -Force
    Write-Host "Removed $dest"
}

if ($RemoveSettings) {
    if ((Test-Path -LiteralPath (Join-Path $settings 'taskbar_state.toml')) -or (Test-Path -LiteralPath (Join-Path $settings 'immersive_state.toml'))) {
        Write-Warning 'Desktop recovery state still exists: settings kept so it can be restored.'
    } elseif ((Test-Path -LiteralPath $settings -PathType Container) -and ((Split-Path -Leaf $settings) -eq 'WinShellTheme')) {
        Remove-Item -LiteralPath $settings -Recurse -Force
        Write-Host "Removed $settings"
    }
} else {
    Write-Host "Settings kept in $settings (use -RemoveSettings to delete them)."
}
