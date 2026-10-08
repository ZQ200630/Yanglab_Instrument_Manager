param(
    [Parameter(Mandatory=$true,ParameterSetName='Enumeration')][switch]$ListControllers,
    [Parameter(Mandatory=$true,ParameterSetName='Connection')][ValidatePattern('^[0-9]{1,16}$')][string]$ControllerSerial,
    [string]$PackageRoot
)
# Two separately authorized read-only stages. No output-changing diagnostic exists.
$ErrorActionPreference = 'Stop'
$taskRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
if (!$PackageRoot) { $PackageRoot = Join-Path $taskRoot 'App/src-tauri/target/debug' }
$taskPackage = [IO.Path]::GetFullPath($PackageRoot)
$taskBinary = Join-Path $taskPackage 'drivers/newport/yang-lab-tlb.exe'
if (!(Test-Path -LiteralPath $taskBinary -PathType Leaf)) { throw 'Build and check the native App package before diagnostics.' }
if ($ListControllers) { & $taskBinary --enumerate } else { & $taskBinary --read-only ('6700 SN' + $ControllerSerial) }
if ($LASTEXITCODE -ne 0) { throw 'Native read-only diagnostic failed; never repeat an output command or force the owner to exit.' }
