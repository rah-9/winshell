<#
.SYNOPSIS
  Installs WinShellTheme for the current user. No administrator rights, no downloads.

.DESCRIPTION
  Copies winshell.exe to %LOCALAPPDATA%\Programs\WinShellTheme, optionally registers
  it to start with Windows (HKCU Run value) and starts it. A running copy is first
  asked to exit cleanly so it restores the native taskbar.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File .\install.ps1
  powershell -ExecutionPolicy Bypass -File .\install.ps1 -NoAutostart -NoStart
#>
[CmdletBinding()]
param(
    [string]$Source = (Join-Path $PSScriptRoot 'winshell.exe'),
    [switch]$NoAutostart,
    [switch]$NoStart
)
$ErrorActionPreference = 'Stop'

if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) { throw 'LOCALAPPDATA is not set; aborting.' }
if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) { throw "winshell.exe not found at '$Source'." }

$dest = Join-Path $env:LOCALAPPDATA 'Programs\WinShellTheme'
$exe  = Join-Path $dest 'winshell.exe'

function Invoke-Winshell([string]$Path, [string]$Argument) {
    # winshell.exe is a GUI program: Start-Process -Wait is needed to wait for it.
    $p = Start-Process -FilePath $Path -ArgumentList $Argument -Wait -PassThru -WindowStyle Hidden
    return $p.ExitCode
}

New-Item -ItemType Directory -Force -Path $dest | Out-Null

if (Test-Path -LiteralPath $exe -PathType Leaf) {
    Write-Host 'Stopping the running copy (it restores the taskbar)...'
    [void](Invoke-Winshell $exe '--quit')
}

Copy-Item -LiteralPath $Source -Destination $exe -Force
Write-Host "Installed to $exe"

if (-not $NoAutostart) {
    if ((Invoke-Winshell $exe '--enable-autostart') -eq 0) { Write-Host 'Will start with Windows.' }
    else { Write-Warning 'Could not register autostart (see %APPDATA%\WinShellTheme\winshell.log).' }
}

if (-not $NoStart) {
    Start-Process -FilePath $exe
    Write-Host 'Started. Emergency exit: Ctrl+Shift+Alt+Esc.'
}
