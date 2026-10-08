param(
    [Parameter(Mandatory=$true)][string]$Root,
    [Parameter(Mandatory=$true)][string]$SourceRoot,
    [string]$GuiHash,[string]$HostHash,[string]$WorkerHash
)
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'native-package.ps1')
$taskPackage=[IO.Path]::GetFullPath($Root).TrimEnd('\','/')
$taskSource=[IO.Path]::GetFullPath($SourceRoot)
Assert-NativePath $Root
Assert-NativePath $SourceRoot
Assert-NativeBundleResources $taskSource
$taskResources=Get-NativeResources
$taskAllowed=@('sil-instrument-console.exe','yang-lab-host.exe','yang-worker.exe') + @($taskResources.Keys)
$taskDirectories=@('App','App/catalog','Config','drivers','drivers/newport','drivers/ch340','drivers/cp210x','drivers/cp210x/arm','drivers/cp210x/arm64','drivers/cp210x/x64','drivers/cp210x/x86')
$taskPending=[Collections.Generic.Queue[string]]::new();$taskPending.Enqueue($taskPackage)
while($taskPending.Count) {
    foreach($taskItem in Get-ChildItem -LiteralPath $taskPending.Dequeue() -Force) {
        if($taskItem.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Package reparse point refused'}
        $taskRelative=$taskItem.FullName.Substring($taskPackage.Length+1).Replace('\','/')
        if($taskItem.PSIsContainer){if($taskRelative -notin $taskDirectories){throw "Unexpected package directory: $taskRelative"};$taskPending.Enqueue($taskItem.FullName)}
        elseif($taskRelative -notin $taskAllowed){throw "Unexpected package file: $taskRelative"}
    }
}
foreach($taskRelative in $taskAllowed){if(!(Test-Path -LiteralPath (Join-Path $taskPackage $taskRelative) -PathType Leaf)){throw "Missing package file: $taskRelative"}}
Assert-NativeDrivers $taskSource (Join-Path $taskPackage 'drivers')
foreach($taskRelative in $taskResources.Keys | Where-Object { $_ -notin @('native-package.json','THIRD_PARTY.txt') }) {
    if((Get-FileHash -LiteralPath (Join-Path $taskSource $taskResources[$taskRelative]) -Algorithm SHA256).Hash -cne (Get-FileHash -LiteralPath (Join-Path $taskPackage $taskRelative) -Algorithm SHA256).Hash){throw "Mismatched approved data: $taskRelative"}
}
$taskManifest=Read-NativeJson (Join-Path $taskPackage 'native-package.json')
$taskKeys=@($taskManifest.PSObject.Properties.Name|Sort-Object)
if(($taskKeys -join ',') -ne 'package_revision,protocol_version,schema,source_revision,startup_revision,worker_sha256' -or $taskManifest.schema -ne 1 -or $taskManifest.protocol_version -ne 3 -or $taskManifest.startup_revision -ne 1 -or !$taskManifest.source_revision -or !$taskManifest.package_revision -or $taskManifest.worker_sha256 -cnotmatch '^[0-9a-f]{64}$'){throw 'Invalid native package identity'}
$taskHashes=@{}
foreach($taskEntry in @(@('Gui','sil-instrument-console.exe',$GuiHash),@('Host','yang-lab-host.exe',$HostHash),@('Worker','yang-worker.exe',$WorkerHash))){
    $taskFile=Join-Path $taskPackage $taskEntry[1]
    $taskBytes=[IO.File]::ReadAllBytes($taskFile)
    if($taskBytes.Length -lt 2 -or $taskBytes[0] -ne 77 -or $taskBytes[1] -ne 90){throw 'Native PE image required'}
    $taskActual=(Get-FileHash -LiteralPath $taskFile -Algorithm SHA256).Hash.ToLowerInvariant()
    if($taskEntry[2] -and $taskActual -cne $taskEntry[2].ToLowerInvariant()){throw "$($taskEntry[0]) image hash mismatch"}
    $taskHashes[$taskEntry[0]]=$taskActual
}
if($taskHashes.Worker -cne $taskManifest.worker_sha256){throw 'Worker differs from packaged identity'}
[PSCustomObject]@{GuiHash=$taskHashes.Gui;HostHash=$taskHashes.Host;WorkerHash=$taskHashes.Worker;Resources=$taskResources.Count;Files=$taskAllowed.Count;DriverPackages=3;DriverPins=18;PythonPayload=$false;Fixtures=$false;SourceRevision=$taskManifest.source_revision;PackageRevision=$taskManifest.package_revision}|ConvertTo-Json -Compress