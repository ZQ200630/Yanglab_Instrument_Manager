param(
    [switch]$Offline,
    [ValidateSet('debug', 'release')][string]$Profile = 'release',
    [string]$TargetDir,
    [string]$Cargo
)
$ErrorActionPreference = 'Stop'
$taskRepo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$taskManifest = Join-Path $taskRepo 'App/src-tauri/Cargo.toml'
$taskCargo = $Cargo
if (!$taskCargo) {
    $taskCommand = Get-Command cargo -ErrorAction SilentlyContinue
    $taskCargo = if ($taskCommand) { $taskCommand.Source } else { Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' }
}
if (!(Test-Path -LiteralPath $taskCargo -PathType Leaf)) { throw 'Cargo is missing. Install the Rust MSVC toolchain before building.' }
$taskVswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
if (!(Test-Path -LiteralPath $taskVswhere -PathType Leaf)) { throw 'Visual Studio C++ Build Tools and Windows SDK are required.' }
$taskShell = & $taskVswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find 'Common7/Tools/Launch-VsDevShell.ps1'
if ($LASTEXITCODE -ne 0 -or !$taskShell -or !(Test-Path -LiteralPath $taskShell -PathType Leaf)) { throw 'No complete MSVC x64 build installation was found.' }
if (!$TargetDir) { $TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'App/src-tauri/target' } }
$taskTarget = if ([IO.Path]::IsPathRooted($TargetDir)) { [IO.Path]::GetFullPath($TargetDir) } else { [IO.Path]::GetFullPath((Join-Path $taskRepo $TargetDir)) }
$taskSavedEnvironment = [Collections.Generic.Dictionary[string,string]]::new([StringComparer]::OrdinalIgnoreCase)
foreach ($taskEntry in [Environment]::GetEnvironmentVariables('Process').GetEnumerator()) {
    $taskSavedEnvironment.Add($taskEntry.Key, $taskEntry.Value)
}
try {
    & $taskShell -Arch amd64 -HostArch amd64 -SkipAutomaticLocation
    $env:CARGO_TARGET_DIR = $taskTarget
    # Build the reusable Rust TLB driver first; it is a fixed bundled runtime resource.
    $env:CARGO_TARGET_DIR = Join-Path $taskTarget 'tlb-native'
    $taskNativeArgs = @('build', '--locked', '--manifest-path', (Join-Path $taskRepo 'Code/Utils/tlb_native/Cargo.toml'), '--bin', 'yang-lab-tlb')
    if ($Offline) { $taskNativeArgs += '--offline' }
    if ($Profile -eq 'release') { $taskNativeArgs += '--release' }
    & $taskCargo @taskNativeArgs
    if ($LASTEXITCODE -ne 0) { throw 'Native TLB build failed. Do not substitute a Python instrument transport.' }
    $taskNativeBuilt = Join-Path $env:CARGO_TARGET_DIR "$Profile/yang-lab-tlb.exe"
    if (!(Test-Path -LiteralPath $taskNativeBuilt -PathType Leaf)) { throw 'Cargo did not produce the expected native TLB artifact.' }
    $taskNativeGenerated = Join-Path $taskRepo 'App/src-tauri/binaries/yang-lab-tlb.exe'
    New-Item -ItemType Directory -Path (Split-Path -Parent $taskNativeGenerated) -Force | Out-Null
    Copy-Item -LiteralPath $taskNativeBuilt -Destination $taskNativeGenerated -Force
    $env:CARGO_TARGET_DIR = $taskTarget
    # Bootstrap the Host before its generated Tauri sidecar exists.
    $env:TAURI_CONFIG = '{"bundle":{"externalBin":[]}}'
    $taskArgs = @('build', '--locked', '--manifest-path', $taskManifest, '--features', 'host-bin', '--bin', 'yang-lab-host')
    if ($Offline) { $taskArgs += '--offline' }
    if ($Profile -eq 'release') { $taskArgs += '--release' }
    & $taskCargo @taskArgs
    if ($LASTEXITCODE -ne 0) { throw 'Native Host build failed. Do not substitute another binary.' }
    $taskGenerated = Join-Path $taskRepo 'App/src-tauri/binaries/yang-lab-host-x86_64-pc-windows-msvc.exe'
    $taskBuilt = Join-Path $taskTarget "$Profile/yang-lab-host.exe"
    if (!(Test-Path -LiteralPath $taskBuilt -PathType Leaf)) { throw 'Cargo did not produce the expected Host artifact.' }
    New-Item -ItemType Directory -Path (Split-Path -Parent $taskGenerated) -Force | Out-Null
    Copy-Item -LiteralPath $taskBuilt -Destination $taskGenerated -Force
    Get-FileHash -LiteralPath $taskGenerated -Algorithm SHA256
} finally {
    # MSVC can change, add and remove variables. Restore the entire process
    # environment on success and on failure, including initialization failure.
    foreach ($taskEnvName in [Environment]::GetEnvironmentVariables('Process').Keys) {
        if (!$taskSavedEnvironment.ContainsKey($taskEnvName)) {
            # PowerShell/.NET versions differ when binding $null to string;
            # the provider's removal operation deletes rather than empties it.
            Remove-Item -LiteralPath ('Env:' + $taskEnvName)
        }
    }
    foreach ($taskEntry in $taskSavedEnvironment.GetEnumerator()) {
        if ([Environment]::GetEnvironmentVariable($taskEntry.Key, 'Process') -cne $taskEntry.Value) {
            # .NET Framework treats an empty value as removal; Windows permits
            # an existing empty process variable and it must survive restoration.
            if ($taskEntry.Value -eq '') {
                if (!('Yanglab.Build.ProcessEnvironment' -as [type])) {
                    Add-Type -TypeDefinition @'
namespace Yanglab.Build {
    public static class ProcessEnvironment {
        [System.Runtime.InteropServices.DllImport("kernel32.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode, SetLastError=true)]
        [return: System.Runtime.InteropServices.MarshalAs(System.Runtime.InteropServices.UnmanagedType.Bool)]
        public static extern bool SetEnvironmentVariable(string name, string value);
    }
}
'@
                }
                if (![Yanglab.Build.ProcessEnvironment]::SetEnvironmentVariable($taskEntry.Key, '')) {
                    throw 'Could not restore an empty caller environment variable.'
                }
            } else {
                [Environment]::SetEnvironmentVariable($taskEntry.Key, $taskEntry.Value, 'Process')
            }
        }
    }
}
