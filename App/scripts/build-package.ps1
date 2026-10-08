param(
    [Parameter(Mandatory=$true)][string]$SourceRoot,
    [Parameter(Mandatory=$true)][string]$Cargo,
    [Parameter(Mandatory=$true)][string]$TargetDir,
    [Parameter(Mandatory=$true)][string]$Output,
    [switch]$Offline,
    [switch]$PortableOnly
)
$ErrorActionPreference='Stop'
foreach($taskPath in @($SourceRoot,$Cargo,$TargetDir,$Output)){if(![IO.Path]::IsPathRooted($taskPath)){throw 'Package paths must be absolute'}}
$taskOutput=[IO.Path]::GetFullPath($Output)
if(Test-Path -LiteralPath $taskOutput){throw 'Output already exists; choose a new candidate directory'}
if(!(Test-Path -LiteralPath (Split-Path -Parent $taskOutput) -PathType Container)){throw 'Candidate parent directory must exist'}
. (Join-Path $PSScriptRoot 'native-package.ps1')
$taskSource=[IO.Path]::GetFullPath($SourceRoot)
Assert-NativePath (Split-Path -Parent $taskOutput)
Assert-NativeBundleResources $taskSource
# Preflight before building; the bundler must not bootstrap CLI or NSIS tools.
if(!$PortableOnly) {
    $taskCli=Join-Path (Split-Path -Parent $Cargo) 'cargo-tauri.exe'
    if(!(Test-Path -LiteralPath $taskCli -PathType Leaf)) { $taskCommand=Get-Command cargo-tauri -ErrorAction SilentlyContinue; if(!$taskCommand){throw 'Installed Tauri CLI 2 unavailable; use -PortableOnly or prepare tooling explicitly'}; $taskCli=$taskCommand.Source }
    $taskCliVersion=& $taskCli --version
    if($LASTEXITCODE -ne 0 -or "$taskCliVersion" -notmatch '^tauri-cli 2\.') {throw 'Installed Tauri CLI 2 required; no automatic installation'}
    $taskTools=if($env:TAURI_BUNDLER_TOOLS_PATH){$env:TAURI_BUNDLER_TOOLS_PATH}else{Join-Path $env:LOCALAPPDATA 'tauri'}
    if(!(Test-Path -LiteralPath (Join-Path $taskTools 'NSIS/makensis.exe') -PathType Leaf) -and !(Test-Path -LiteralPath (Join-Path $taskTools 'NSIS/Bin/makensis.exe') -PathType Leaf)) {throw 'Prepared NSIS tooling unavailable; use -PortableOnly. No tools were installed'}
}
$taskBefore=@{};foreach($taskName in @('CARGO_TARGET_DIR','TAURI_CONFIG','YANG_PACKAGE_REVISION','RUSTFLAGS','RUSTC_ENCODED_RUSTFLAGS')){$taskBefore[$taskName]=[Environment]::GetEnvironmentVariable($taskName)}
Push-Location $taskSource
try {
    $taskIdentity=& (Join-Path $PSScriptRoot 'build-native.ps1') -SourceRoot $taskSource -Cargo $Cargo -TargetDir $TargetDir -Offline:$Offline | Select-Object -Last 1 | ConvertFrom-Json
    $env:CARGO_TARGET_DIR=[IO.Path]::GetFullPath($TargetDir)
    Remove-Item Env:TAURI_CONFIG -ErrorAction SilentlyContinue
    $env:YANG_PACKAGE_REVISION=$taskIdentity.package_revision
    if($env:RUSTC_ENCODED_RUSTFLAGS) {$env:RUSTC_ENCODED_RUSTFLAGS += [char]31+'-C'+[char]31+'target-feature=+crt-static'} else {$env:RUSTFLAGS=($env:RUSTFLAGS+' -C target-feature=+crt-static').Trim()}
    if($PortableOnly) {
        $taskFlags=@('build','--locked','--release','--target','x86_64-pc-windows-msvc','-p','sil-instrument-console','--bin','sil-instrument-console');if($Offline){$taskFlags+='--offline'}
        & $Cargo @taskFlags
        if($LASTEXITCODE -ne 0){throw 'Native portable GUI build failed'}
    } else {
        Push-Location (Join-Path $taskSource 'App/src-tauri')
        try {
            $taskFlags=@('build','--target','x86_64-pc-windows-msvc','--bundles','nsis','--','--locked');if($Offline){$taskFlags+='--offline'}
            & $taskCli @taskFlags
            if($LASTEXITCODE -ne 0){throw 'Native GUI/NSIS build failed; no installer was installed'}
        } finally {Pop-Location}
    }
    if(('tree-'+(Get-NativeSourceFingerprint $taskSource)) -cne $taskIdentity.source_revision){throw 'Source changed during GUI build; discard candidate'}
    New-Item -ItemType Directory -Path $taskOutput|Out-Null
    $taskPortable=Join-Path $taskOutput 'portable';New-Item -ItemType Directory -Path $taskPortable|Out-Null
    $taskRelease=Join-Path $TargetDir 'x86_64-pc-windows-msvc/release'
    foreach($taskBin in @('sil-instrument-console','yang-lab-host','yang-worker')) {Copy-Item -LiteralPath (Join-Path $taskRelease "$taskBin.exe") -Destination (Join-Path $taskPortable "$taskBin.exe")}
    $taskResources=Get-NativeResources
    foreach($taskRelative in $taskResources.Keys) {
        $taskDestination=Join-Path $taskPortable $taskRelative
        New-Item -ItemType Directory -Path (Split-Path -Parent $taskDestination) -Force|Out-Null
        Copy-Item -LiteralPath (Join-Path $taskSource $taskResources[$taskRelative]) -Destination $taskDestination
    }
    $taskCheck=& (Join-Path $PSScriptRoot 'check-package.ps1') -Root $taskPortable -SourceRoot $taskSource | ConvertFrom-Json
    $taskInstallerHash=$null
    if(!$PortableOnly) {
        $taskInstallers=@(Get-ChildItem -LiteralPath (Join-Path $taskRelease 'bundle/nsis') -Filter '*-setup.exe' -File)
        if($taskInstallers.Count -ne 1){throw 'Expected exactly one candidate NSIS installer'}
        Copy-Item -LiteralPath $taskInstallers[0].FullName -Destination (Join-Path $taskOutput $taskInstallers[0].Name)
        $taskInstallerHash=(Get-FileHash -LiteralPath $taskInstallers[0].FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    if(('tree-'+(Get-NativeSourceFingerprint $taskSource)) -cne $taskIdentity.source_revision){throw 'Source changed during package staging; discard candidate'}
    $taskReport=[ordered]@{schema=1;architecture='x86_64-pc-windows-msvc';source_revision=$taskIdentity.source_revision;package_revision=$taskIdentity.package_revision;protocol_version=3;startup_revision=1;gui_sha256=$taskCheck.GuiHash;host_sha256=$taskCheck.HostHash;worker_sha256=$taskCheck.WorkerHash;portable_only=[bool]$PortableOnly;installer_built=(!$PortableOnly);installer_sha256=$taskInstallerHash;driver_packages=3;driver_pins=18;portable_files=$taskCheck.Files;python_payload=$false;physical_validated=$false;clean_windows_validated=$false;installed=$false}
    [IO.File]::WriteAllText((Join-Path $taskOutput 'qualification.json'),($taskReport|ConvertTo-Json),[Text.UTF8Encoding]::new($false))
    $taskReport|ConvertTo-Json -Compress
} finally {
    foreach($taskName in $taskBefore.Keys){if($null -eq $taskBefore[$taskName]){Remove-Item -LiteralPath "Env:$taskName" -ErrorAction SilentlyContinue}else{[Environment]::SetEnvironmentVariable($taskName,$taskBefore[$taskName])}}
    Pop-Location
}