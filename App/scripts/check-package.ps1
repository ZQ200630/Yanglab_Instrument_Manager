param(
    [Parameter(Mandatory=$true)][string]$Root,
    [Parameter(Mandatory=$true)][string]$SourceRoot,
    [string]$GuiHash,
    [string]$HostHash
)
$ErrorActionPreference = 'Stop'
$taskPackage = [IO.Path]::GetFullPath($Root).TrimEnd('\','/')
$taskSource = [IO.Path]::GetFullPath($SourceRoot)
$taskTauri = Join-Path $taskSource 'App/src-tauri'
$taskMap = (Get-Content -LiteralPath (Join-Path $taskTauri 'tauri.conf.json') -Raw | ConvertFrom-Json).bundle.resources
foreach ($taskObsolete in @('App/worker/simulation.py','Code/Debugs','Code/Experiments','App/web/scene','App/web/vendor/three')) {
    if (Test-Path -LiteralPath (Join-Path $taskPackage $taskObsolete)) {
        throw "Retired resource is installed: $taskObsolete"
    }
}
$taskExpected = @()
foreach ($taskEntry in $taskMap.PSObject.Properties) {
    $taskPattern = Join-Path $taskTauri $taskEntry.Name
    $taskSources = @(Get-ChildItem -Path $taskPattern -File)
    if (!$taskSources.Count) { throw "Empty resource mapping: $($taskEntry.Name)" }
    foreach ($taskFile in $taskSources) {
        $taskRelative = if ($taskEntry.Name.Contains('*')) { Join-Path $taskEntry.Value $taskFile.Name } else { $taskEntry.Value }
        $taskTarget = [IO.Path]::GetFullPath((Join-Path $taskPackage $taskRelative))
        if (!$taskTarget.StartsWith($taskPackage + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Package resource escapes its root'
        }
        if (!(Test-Path -LiteralPath $taskTarget -PathType Leaf) -or
            (Get-FileHash -LiteralPath $taskFile.FullName).Hash -ne (Get-FileHash -LiteralPath $taskTarget).Hash) {
            throw "Missing or mismatched packaged source: $taskRelative"
        }
        $taskExpected += $taskTarget
    }
}
foreach ($taskDirectory in @('Code','Config','App/worker','App/catalog','drivers')) {
    foreach ($taskFile in Get-ChildItem -LiteralPath (Join-Path $taskPackage $taskDirectory) -File -Recurse) {
        if ($taskFile.FullName -notin $taskExpected) { throw "Unexpected runtime resource: $($taskFile.FullName)" }
    }
}
$taskGui = (Get-FileHash -LiteralPath (Join-Path $taskPackage 'sil-instrument-console.exe')).Hash
$taskHost = (Get-FileHash -LiteralPath (Join-Path $taskPackage 'yang-lab-host.exe')).Hash
if ($GuiHash -and $taskGui -ne $GuiHash) { throw 'GUI binary hash mismatch' }
if ($HostHash -and $taskHost -ne $HostHash) { throw 'Host binary hash mismatch' }
[PSCustomObject]@{Resources=$taskExpected.Count;GuiHash=$taskGui;HostHash=$taskHost;ObsoleteResources=$false} | ConvertTo-Json -Compress
