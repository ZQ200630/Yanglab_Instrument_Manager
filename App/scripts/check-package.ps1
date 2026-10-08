param(
    [Parameter(Mandatory=$true)][string]$Root,
    [Parameter(Mandatory=$true)][string]$SourceRoot,
    [string]$GuiHash,[string]$HostHash,[string]$WorkerHash
)
$ErrorActionPreference='Stop'
foreach($taskPath in @($Root,$SourceRoot)){if(![IO.Path]::IsPathRooted($taskPath)){throw 'Absolute package/source paths required'}}
$taskPackage=[IO.Path]::GetFullPath($Root).TrimEnd('\','/')
$taskSource=[IO.Path]::GetFullPath($SourceRoot)
$taskAllowed=@('sil-instrument-console.exe','yang-lab-host.exe','yang-worker.exe','native-package.json','THIRD_PARTY.txt','App/catalog/devices.json','Config/fiber_coupling.json')
foreach($taskItem in Get-ChildItem -LiteralPath $taskPackage -Force -Recurse) {
    if($taskItem.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Package reparse point refused'}
    $taskRelative=$taskItem.FullName.Substring($taskPackage.Length+1).Replace('\','/')
    if($taskItem.PSIsContainer){if($taskRelative -notin @('App','App/catalog','Config')){throw "Unexpected package directory: $taskRelative"}}
    elseif($taskRelative -notin $taskAllowed){throw "Unexpected package file: $taskRelative"}
}
foreach($taskRelative in $taskAllowed){if(!(Test-Path -LiteralPath (Join-Path $taskPackage $taskRelative) -PathType Leaf)){throw "Missing package file: $taskRelative"}}
foreach($taskRelative in @('App/catalog/devices.json','Config/fiber_coupling.json')){
    if((Get-FileHash -LiteralPath (Join-Path $taskSource $taskRelative)).Hash -ne (Get-FileHash -LiteralPath (Join-Path $taskPackage $taskRelative)).Hash){throw "Mismatched approved data: $taskRelative"}
}
$taskManifest=Get-Content -LiteralPath (Join-Path $taskPackage 'native-package.json') -Raw|ConvertFrom-Json
$taskKeys=@($taskManifest.PSObject.Properties.Name|Sort-Object)
if(($taskKeys -join ',') -ne 'package_revision,protocol_version,schema,source_revision,startup_revision,worker_sha256' -or $taskManifest.schema -ne 1 -or $taskManifest.protocol_version -ne 3 -or $taskManifest.startup_revision -ne 1 -or !$taskManifest.source_revision -or !$taskManifest.package_revision -or $taskManifest.worker_sha256 -cnotmatch '^[0-9a-f]{64}$'){throw 'Invalid native package identity'}
$taskHashes=@{}
foreach($taskEntry in @(@('Gui','sil-instrument-console.exe',$GuiHash),@('Host','yang-lab-host.exe',$HostHash),@('Worker','yang-worker.exe',$WorkerHash))){
    $taskFile=Join-Path $taskPackage $taskEntry[1]
    $taskBytes=[IO.File]::ReadAllBytes($taskFile)
    if($taskBytes.Length -lt 2 -or $taskBytes[0] -ne 77 -or $taskBytes[1] -ne 90){throw 'Native PE image required'}
    $taskActual=(Get-FileHash -LiteralPath $taskFile).Hash.ToLowerInvariant()
    if($taskEntry[2] -and $taskActual -ne $taskEntry[2].ToLowerInvariant()){throw "$($taskEntry[0]) image hash mismatch"}
    $taskHashes[$taskEntry[0]]=$taskActual
}
if($taskHashes.Worker -cne $taskManifest.worker_sha256){throw 'Worker differs from packaged identity'}
[PSCustomObject]@{GuiHash=$taskHashes.Gui;HostHash=$taskHashes.Host;WorkerHash=$taskHashes.Worker;Resources=4;PythonPayload=$false;Fixtures=$false;SourceRevision=$taskManifest.source_revision;PackageRevision=$taskManifest.package_revision}|ConvertTo-Json -Compress
