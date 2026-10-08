param(
    [Parameter(Mandatory=$true)][string]$SourceRoot,
    [Parameter(Mandatory=$true)][string]$Cargo,
    [Parameter(Mandatory=$true)][string]$TargetDir,
    [switch]$Offline
)
$ErrorActionPreference='Stop'
foreach($taskPath in @($SourceRoot,$Cargo,$TargetDir)) {if(![IO.Path]::IsPathRooted($taskPath)){throw 'Build paths must be absolute'}}
$taskSource=[IO.Path]::GetFullPath($SourceRoot)
if (!(Test-Path -LiteralPath (Join-Path $taskSource 'Cargo.toml')) -or !(Test-Path -LiteralPath $Cargo -PathType Leaf)) {throw 'Source or Cargo is missing'}
$taskVersion=& $Cargo --version 2>&1
if($LASTEXITCODE -ne 0 -or "$taskVersion" -notmatch '^cargo \d') {throw 'Not Cargo: select the installed native cargo.exe'}
$taskNames=@('CARGO_TARGET_DIR','TAURI_CONFIG','YANG_PACKAGE_REVISION','RUSTFLAGS','RUSTC_ENCODED_RUSTFLAGS')
$taskBefore=@{};foreach($taskName in $taskNames){$taskBefore[$taskName]=[Environment]::GetEnvironmentVariable($taskName)}
$taskTarget=[IO.Path]::GetFullPath($TargetDir)
$taskStage=Join-Path $taskSource 'App/src-tauri/binaries'
$taskPayload=Join-Path $taskSource 'App/runtime/native-payload'
. (Join-Path $PSScriptRoot 'native-package.ps1')
Assert-NativeBundleResources $taskSource
Assert-NativeDrivers $taskSource (Join-Path $taskSource 'App/drivers')
function Get-SourceHash { Get-NativeSourceFingerprint $taskSource }
$taskSourceHash=Get-SourceHash
$taskPackageVersion=(Get-Content -LiteralPath (Join-Path $taskSource 'App/src-tauri/tauri.conf.json') -Raw|ConvertFrom-Json).version
$taskPackageRevision="$taskPackageVersion-$($taskSourceHash.Substring(0,12))"
$taskFlags=@('--locked','--release','--target','x86_64-pc-windows-msvc');if($Offline){$taskFlags+='--offline'}
Push-Location $taskSource
try {
    $env:CARGO_TARGET_DIR=$taskTarget
    $env:TAURI_CONFIG='{"bundle":{"externalBin":[],"resources":[]}}'
    $env:YANG_PACKAGE_REVISION=$taskPackageRevision
    if($env:RUSTC_ENCODED_RUSTFLAGS) {$env:RUSTC_ENCODED_RUSTFLAGS += [char]31+'-C'+[char]31+'target-feature=+crt-static'} else {$env:RUSTFLAGS=($env:RUSTFLAGS+' -C target-feature=+crt-static').Trim()}
    & $Cargo build @taskFlags -p yang-worker --bin yang-worker
    if($LASTEXITCODE -ne 0){throw 'Native worker build failed'}
    & $Cargo build @taskFlags -p sil-instrument-console --features host-bin --bin yang-lab-host
    if($LASTEXITCODE -ne 0){throw 'Native Host build failed'}
    if((Get-SourceHash) -ne $taskSourceHash){throw 'Source changed during native build; discard candidate'}
    New-Item -ItemType Directory -Force -Path $taskStage,$taskPayload|Out-Null
    foreach($taskBin in @('yang-lab-host','yang-worker')) {Copy-Item -LiteralPath (Join-Path $taskTarget "x86_64-pc-windows-msvc/release/$taskBin.exe") -Destination (Join-Path $taskStage "$taskBin-x86_64-pc-windows-msvc.exe") -Force}
    $taskIdentity=[ordered]@{schema=1;source_revision="tree-$taskSourceHash";package_revision=$taskPackageRevision;protocol_version=3;startup_revision=1;worker_sha256=(Get-FileHash -LiteralPath (Join-Path $taskStage 'yang-worker-x86_64-pc-windows-msvc.exe')).Hash.ToLowerInvariant()}
    # Generated data is not tracked source and never modifies a user runtime.
    $taskJson=$taskIdentity|ConvertTo-Json
    [IO.File]::WriteAllText((Join-Path $taskPayload 'native-package.json'),$taskJson,[Text.UTF8Encoding]::new($false))
    $taskMetadata=& $Cargo metadata --locked --offline --filter-platform x86_64-pc-windows-msvc --format-version 1 | ConvertFrom-Json
    if($LASTEXITCODE -ne 0){throw 'Cannot collect pinned dependency notices'}
    $taskNotice=[Text.StringBuilder]::new([IO.File]::ReadAllText((Join-Path $taskSource 'App/runtime/NATIVE_THIRD_PARTY.txt')))
    foreach($taskDependency in ($taskMetadata.packages|Sort-Object name,version)) {
        [void]$taskNotice.AppendLine("`n--- $($taskDependency.name) $($taskDependency.version) : $($taskDependency.license) ---")
        $taskCrateRoot=Split-Path -Parent $taskDependency.manifest_path
        foreach($taskLicense in (Get-ChildItem -LiteralPath $taskCrateRoot -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|COPYRIGHT|NOTICE)([._-].*)?$' })) {
            if($taskLicense.Length -gt 1MB){throw 'Dependency notice exceeds bound'}
            [void]$taskNotice.AppendLine([IO.File]::ReadAllText($taskLicense.FullName))
        }
        if($taskNotice.Length -gt 8MB){throw 'Dependency notice envelope exceeded'}
    }
    [IO.File]::WriteAllText((Join-Path $taskPayload 'THIRD_PARTY.txt'),$taskNotice.ToString(),[Text.UTF8Encoding]::new($false))
    $taskIdentity|ConvertTo-Json -Compress
} finally {
    foreach($taskName in $taskNames){if($null -eq $taskBefore[$taskName]){Remove-Item -LiteralPath "Env:$taskName" -ErrorAction SilentlyContinue}else{[Environment]::SetEnvironmentVariable($taskName,$taskBefore[$taskName])}}
    Pop-Location
}
