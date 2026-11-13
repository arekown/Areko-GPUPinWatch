#Requires -RunAsAdministrator
<#
  Areko GPUPinWatch - Windows installer.
  Run from the unpacked release folder in an administrator PowerShell:
      powershell -ExecutionPolicy Bypass -File .\install.ps1
  Remove again:
      powershell -ExecutionPolicy Bypass -File .\install.ps1 -Uninstall
  Copies the programs to "C:\Program Files\Areko GPUPinWatch", registers and starts the service
  (runs as LocalSystem, starts with Windows) and adds a Start menu entry for the window app.
  Settings stay in %ProgramData%\Areko GPUPinWatch.
#>
param([switch]$Uninstall)
$ErrorActionPreference = 'Stop'

$dest = Join-Path $env:ProgramFiles 'Areko GPUPinWatch'
$svc = Join-Path $dest 'areko-gpupinwatchd.exe'
$app = Join-Path $dest 'areko-gpupinwatch.exe'
$lnk = Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Areko GPUPinWatch.lnk'

function Stop-Old {
    Get-Process -Name 'areko-gpupinwatch' -ErrorAction SilentlyContinue | Stop-Process -Force
    if (Test-Path $svc) {
        & $svc uninstall 2>$null | Out-Null
        Start-Sleep -Seconds 2
    }
}

if ($Uninstall) {
    Stop-Old
    Remove-Item $lnk -ErrorAction SilentlyContinue
    Remove-Item $dest -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host 'Areko GPUPinWatch removed. Settings in %ProgramData%\Areko GPUPinWatch were kept.'
    return
}

Stop-Old
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item (Join-Path $PSScriptRoot 'areko-gpupinwatchd.exe') $dest -Force
$hasApp = Test-Path (Join-Path $PSScriptRoot 'areko-gpupinwatch.exe')
if ($hasApp) { Copy-Item (Join-Path $PSScriptRoot 'areko-gpupinwatch.exe') $dest -Force }
Copy-Item (Join-Path $PSScriptRoot 'LICENSE.md') $dest -Force -ErrorAction SilentlyContinue

& $svc install
if ($LASTEXITCODE -ne 0) { throw 'Service could not be installed.' }

if ($hasApp) {
    $sh = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
    $sh.TargetPath = $app
    $sh.WorkingDirectory = $dest
    $sh.IconLocation = "$app,0"
    $sh.Save()
    Start-Process -FilePath $app
}
Write-Host 'Areko GPUPinWatch installed: service "Areko GPUPinWatch" is running and starts with Windows.'
