param([string]$SourceRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..')))
$ErrorActionPreference='Stop'
. (Join-Path $SourceRoot 'App/scripts/native-package.ps1')
$taskScratch=Join-Path ([IO.Path]::GetTempPath()) ('Yang native 包 '+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $taskScratch | Out-Null
$taskFailures=0
function Assert-True($Condition,$Message) { if (!$Condition) {throw $Message} }
function Assert-Reject([scriptblock]$Action,[string]$Pattern='') {
    $taskRejected=$false
    try {& $Action | Out-Null} catch {$taskRejected=$true;if($Pattern){Assert-True ("$_" -match $Pattern) "Wrong rejection: $_"}}
    Assert-True $taskRejected 'Operation unexpectedly accepted'
}
function Run-Test($Name,[scriptblock]$Action) { try {& $Action;Write-Output "PASS $Name"}catch{$script:taskFailures++;Write-Output "FAIL $Name : $_"} }
function Check-Package([string]$Root) { & (Join-Path $SourceRoot 'App/scripts/check-package.ps1') -Root $Root -SourceRoot $SourceRoot }
function New-Package($Name) {
    $taskRoot=Join-Path $taskScratch $Name;New-Item -ItemType Directory -Path $taskRoot | Out-Null
    # Structural fixtures deliberately cannot qualify a running PE/runtime.
    foreach($taskBin in @('sil-instrument-console.exe','yang-lab-host.exe','yang-worker.exe')) {[IO.File]::WriteAllBytes((Join-Path $taskRoot $taskBin),[byte[]](77,90,1,2,3))}
    $taskResources=Get-NativeResources
    foreach($taskRelative in $taskResources.Keys | Where-Object {$_ -notin @('native-package.json','THIRD_PARTY.txt')}) {
        $taskDestination=Join-Path $taskRoot $taskRelative
        New-Item -ItemType Directory -Path (Split-Path -Parent $taskDestination) -Force|Out-Null
        Copy-Item -LiteralPath (Join-Path $SourceRoot $taskResources[$taskRelative]) -Destination $taskDestination
    }
    $taskIdentity=@{schema=1;source_revision='structural-fixture';package_revision='structural-fixture';protocol_version=3;startup_revision=1;worker_sha256=(Get-FileHash -LiteralPath (Join-Path $taskRoot 'yang-worker.exe') -Algorithm SHA256).Hash.ToLowerInvariant()}
    [IO.File]::WriteAllText((Join-Path $taskRoot 'native-package.json'),($taskIdentity|ConvertTo-Json))
    [IO.File]::WriteAllText((Join-Path $taskRoot 'THIRD_PARTY.txt'),'Structural notice fixture')
    return $taskRoot
}
function New-Source($Name) {
    $taskMini=Join-Path $taskScratch $Name
    foreach($taskFolder in @('App/protocol/src','App/worker-rs/src','App/src-tauri/src','App/src-tauri/windows','Code/Utils/src','Code/Setups/src','Code/Utils/tlb_native/src','App/web','App/catalog','Config','App/runtime','App/scripts')) {New-Item -ItemType Directory -Path (Join-Path $taskMini $taskFolder) -Force|Out-Null}
    foreach($taskRelative in @('Cargo.toml','Cargo.lock','App/src-tauri/tauri.conf.json','App/src-tauri/build.rs','App/runtime/NATIVE_THIRD_PARTY.txt','Code/Utils/tlb_native/Cargo.toml','Code/Utils/tlb_models.json','App/catalog/devices.json','Config/fiber_coupling.json','App/scripts/native-package.ps1','App/scripts/build-native.ps1','App/scripts/build-package.ps1','App/scripts/build-host.ps1','App/scripts/check-package.ps1')) {Copy-Item -LiteralPath (Join-Path $SourceRoot $taskRelative) -Destination (Join-Path $taskMini $taskRelative)}
    Copy-Item -LiteralPath (Join-Path $SourceRoot 'App/drivers') -Destination (Join-Path $taskMini 'App/drivers') -Recurse
    [IO.File]::WriteAllText((Join-Path $taskMini 'Code/Utils/tlb_native/src/lib.rs'),'finite TLB source')
    return $taskMini
}
$taskTool=Join-Path $taskScratch 'finite-cargo.ps1'
$taskStub=@'
if($args -contains '--version') {'cargo 1.0.0';exit 0}
if($args -contains 'metadata') {'{"packages":[]}';exit 0}
$taskDirectory=Join-Path $env:CARGO_TARGET_DIR 'x86_64-pc-windows-msvc/release';New-Item -ItemType Directory -Path $taskDirectory -Force|Out-Null
foreach($taskImage in @('yang-worker.exe','yang-lab-host.exe','sil-instrument-console.exe')) {[IO.File]::WriteAllBytes((Join-Path $taskDirectory $taskImage),[byte[]](77,90,1))}
if($env:YANG_FINITE_ADD_SOURCE) {[IO.File]::WriteAllText($env:YANG_FINITE_ADD_SOURCE,'source added during finite build')}
if($env:YANG_FINITE_BUILD_FAIL -eq '1') {exit 19}
exit 0
'@
[IO.File]::WriteAllText($taskTool,$taskStub)
try {
    Run-Test 'Closed29FileContractAndBundleAgreement' {
        $taskRoot=New-Package 'complete';$taskResult=Check-Package $taskRoot | ConvertFrom-Json
        Assert-True ($taskResult.Files -eq 29 -and $taskResult.DriverPins -eq 18 -and $taskResult.DriverPackages -eq 3) 'Wrong closed contract'
        Assert-True (!$taskResult.PythonPayload -and !$taskResult.Fixtures) 'Forbidden production payload'
    }
    Run-Test 'EveryPinnedDriverMissingOrModifiedIsRejected' {
        $taskRoot=New-Package 'driver-tamper'
        foreach($taskPackage in $script:NativeDriverPins) {foreach($taskPin in $taskPackage.files) {
            $taskFile=Join-Path $taskRoot ('drivers/'+$taskPin.path)
            $taskOriginal=[IO.File]::ReadAllBytes($taskFile)
            Remove-Item -LiteralPath $taskFile
            Assert-Reject {Check-Package $taskRoot} 'Missing package file'
            [IO.File]::WriteAllBytes($taskFile,$taskOriginal)
            $taskChanged=[byte[]]$taskOriginal.Clone();$taskChanged[0]=$taskChanged[0] -bxor 1
            [IO.File]::WriteAllBytes($taskFile,$taskChanged)
            Assert-Reject {Check-Package $taskRoot} 'Modified fixed driver resource'
            [IO.File]::WriteAllBytes($taskFile,$taskOriginal)
        }}
    }
    Run-Test 'ManifestUnknownPackagesDuplicateKeysAndTamperedNoticesFail' {
        $taskRoot=New-Package 'manifest';$taskFile=Join-Path $taskRoot 'drivers/packages.json';$taskOriginal=[IO.File]::ReadAllBytes($taskFile)
        foreach($taskBad in @('[]','[{"id":"unknown"}]','[{"id":"newport","id":"ch340"}]')) {
            [IO.File]::WriteAllText($taskFile,$taskBad)
            Assert-Reject {Check-Package $taskRoot}
        }
        [IO.File]::WriteAllBytes($taskFile,$taskOriginal)
        [IO.File]::AppendAllText((Join-Path $taskRoot 'drivers/cp210x/README.md'),'tampered attestation')
        Assert-Reject {Check-Package $taskRoot} 'Mismatched approved data'
        Copy-Item -LiteralPath (Join-Path $SourceRoot 'App/drivers/cp210x/README.md') -Destination (Join-Path $taskRoot 'drivers/cp210x/README.md') -Force
        $taskManifest=Join-Path $taskRoot 'native-package.json'
        [IO.File]::WriteAllText($taskManifest,'{"schema":1,"schema":1}')
        Assert-Reject {Check-Package $taskRoot} 'Duplicate'
    }
    Run-Test 'PackageRejectsPythonFixturesExtraPackagesAndDirectories' {
        $taskRoot=New-Package 'extras'
        foreach($taskForbidden in @('main.py','cache.pyc','python.exe','python313.dll','wheel.whl','requirements.txt','environment.yml','worker-fixture.exe','trust.dpapi','recording.bin','source-launcher.ps1','yang-lab-tlb.exe')) {
            $taskBad=Join-Path $taskRoot $taskForbidden;[IO.File]::WriteAllText($taskBad,'forbidden')
            Assert-Reject {Check-Package $taskRoot} 'Unexpected package file'
            Remove-Item -LiteralPath $taskBad
        }
        $taskBad=Join-Path $taskRoot 'drivers/unknown';New-Item -ItemType Directory -Path $taskBad|Out-Null
        Assert-Reject {Check-Package $taskRoot} 'Unexpected package directory';Remove-Item -LiteralPath $taskBad
    }
    Run-Test 'PackageRootAndDescendantReparsePointsAreRejected' {
        $taskRoot=New-Package 'reparse'
        $taskAlias=Join-Path $taskScratch 'junction'
        & cmd.exe /c mklink /J $taskAlias $taskRoot | Out-Null
        if($LASTEXITCODE -ne 0){throw 'Could not create finite junction'}
        try {Assert-Reject {Check-Package $taskAlias} 'reparse'} finally {[IO.Directory]::Delete($taskAlias)}
        $taskChild=Join-Path $taskRoot 'drivers/linked'
        & cmd.exe /c mklink /J $taskChild (Join-Path $taskRoot 'drivers/cp210x') | Out-Null
        if($LASTEXITCODE -ne 0){throw 'Could not create finite descendant junction'}
        try {Assert-Reject {Check-Package $taskRoot} 'reparse'} finally {[IO.Directory]::Delete($taskChild)}
    }
    Run-Test 'MissingFixedDriverPayloadIsRejected' {
        $taskRoot=New-Package 'missing-drivers'
        Remove-Item -LiteralPath (Join-Path $taskRoot 'drivers/packages.json')
        Assert-Reject {Check-Package $taskRoot} 'Missing package file'
    }
    Run-Test 'BuildPreservesEnvironmentAndRefusesExistingOutput' {
        $taskOld=@{};foreach($taskKey in @('PATH','TAURI_CONFIG','CARGO_TARGET_DIR','YANG_PACKAGE_REVISION','RUSTFLAGS','RUSTC_ENCODED_RUSTFLAGS')){$taskOld[$taskKey]=[Environment]::GetEnvironmentVariable($taskKey)}
        $taskExisting=Join-Path $taskScratch 'existing';New-Item -ItemType Directory -Path $taskExisting|Out-Null;[IO.File]::WriteAllText((Join-Path $taskExisting 'keep.txt'),'keep')
        Assert-Reject {& (Join-Path $SourceRoot 'App/scripts/build-package.ps1') -SourceRoot $SourceRoot -Cargo $taskTool -TargetDir (Join-Path $taskScratch 'target') -Output $taskExisting -Offline -PortableOnly} 'Output already exists'
        Assert-True ([IO.File]::ReadAllText((Join-Path $taskExisting 'keep.txt')) -eq 'keep') 'Existing output changed'
        Assert-Reject {& (Join-Path $SourceRoot 'App/scripts/build-native.ps1') -SourceRoot $SourceRoot -Cargo (Join-Path $PSHOME 'pwsh.exe') -TargetDir (Join-Path $taskScratch 'failed') -Offline} 'Not Cargo'
        foreach($taskKey in $taskOld.Keys){Assert-True ([Environment]::GetEnvironmentVariable($taskKey) -ceq $taskOld[$taskKey]) "Environment changed: $taskKey"}
    }
    Run-Test 'TlbModelDriverAndNewSourceChangesAlterFingerprint' {
        $taskMini=New-Source 'fingerprint'
        foreach($taskRelative in @('Code/Utils/tlb_native/src/lib.rs','Code/Utils/tlb_models.json','App/drivers/ch340/CH341SER.INF')) {
            $taskBefore=Get-NativeSourceFingerprint $taskMini
            [IO.File]::AppendAllText((Join-Path $taskMini $taskRelative),'finite fingerprint change')
            Assert-True ((Get-NativeSourceFingerprint $taskMini) -cne $taskBefore) "Input omitted: $taskRelative"
        }
        $taskBefore=Get-NativeSourceFingerprint $taskMini
        [IO.File]::WriteAllText((Join-Path $taskMini 'Code/Utils/tlb_native/src/added.rs'),'new input')
        Assert-True ((Get-NativeSourceFingerprint $taskMini) -cne $taskBefore) 'New source file omitted'
    }
    Run-Test 'SourceAddedDuringBuildIsRejected' {
        $taskMini=New-Source 'racing'
        $taskPrevious=[Environment]::GetEnvironmentVariable('YANG_FINITE_ADD_SOURCE')
        try {
            $env:YANG_FINITE_ADD_SOURCE=Join-Path $taskMini 'Code/Utils/tlb_native/src/added.rs'
            Assert-Reject {& (Join-Path $taskMini 'App/scripts/build-native.ps1') -SourceRoot $taskMini -Cargo $taskTool -TargetDir (Join-Path $taskScratch 'racing-target') -Offline} 'Source changed during native build'
        } finally {if($null -eq $taskPrevious){Remove-Item Env:YANG_FINITE_ADD_SOURCE -ErrorAction SilentlyContinue}else{$env:YANG_FINITE_ADD_SOURCE=$taskPrevious}}
    }
    Run-Test 'PortableOnlyFiniteBuildUsesSame29FilesAndRestoresEnvironment' {
        $taskMini=New-Source 'portable-source'
        $taskBefore=@{};foreach($taskKey in @('TAURI_CONFIG','CARGO_TARGET_DIR','YANG_PACKAGE_REVISION','RUSTFLAGS','RUSTC_ENCODED_RUSTFLAGS')){$taskBefore[$taskKey]=[Environment]::GetEnvironmentVariable($taskKey)}
        $taskOutput=Join-Path $taskScratch 'finite-portable'
        & (Join-Path $taskMini 'App/scripts/build-package.ps1') -SourceRoot $taskMini -Cargo $taskTool -TargetDir (Join-Path $taskScratch 'finite-target') -Output $taskOutput -Offline -PortableOnly | Out-Null
        $taskReport=Get-Content -LiteralPath (Join-Path $taskOutput 'qualification.json') -Raw|ConvertFrom-Json
        Assert-True ($taskReport.portable_only -and !$taskReport.installer_built -and $null -eq $taskReport.installer_sha256 -and $taskReport.portable_files -eq 29) 'False installer/portable report'
        foreach($taskKey in $taskBefore.Keys){Assert-True ([Environment]::GetEnvironmentVariable($taskKey) -ceq $taskBefore[$taskKey]) "Successful build environment changed: $taskKey"}
        $env:YANG_FINITE_BUILD_FAIL='1'
        try {Assert-Reject {& (Join-Path $taskMini 'App/scripts/build-package.ps1') -SourceRoot $taskMini -Cargo $taskTool -TargetDir (Join-Path $taskScratch 'failed-build') -Output (Join-Path $taskScratch 'failed-output') -Offline -PortableOnly} 'Native worker build failed'} finally {Remove-Item Env:YANG_FINITE_BUILD_FAIL}
        foreach($taskKey in $taskBefore.Keys){Assert-True ([Environment]::GetEnvironmentVariable($taskKey) -ceq $taskBefore[$taskKey]) "Failed build environment changed: $taskKey"}
    }
    Run-Test 'UnicodeRootResolvesNativeSidecars' {
        $taskRoot=New-Package '空格 包';$taskResult=Check-Package $taskRoot|ConvertFrom-Json
        Assert-True ($taskResult.WorkerHash -eq (Get-FileHash -LiteralPath (Join-Path $taskRoot 'yang-worker.exe') -Algorithm SHA256).Hash.ToLowerInvariant()) 'Wrong worker in Unicode package'
    }
    Run-Test 'MissingVendorDependencyIsNotInstalled' {
        $taskMissing=Join-Path $taskScratch 'missing-vendor';New-Item -ItemType Directory -Path $taskMissing|Out-Null
        $taskResult=& (Join-Path $SourceRoot 'App/scripts/check-dependencies.ps1') -SystemRoot $taskMissing|ConvertFrom-Json
        Assert-True (!$taskResult.VisaPresent -and !$taskResult.InstalledAnything) 'Missing dependency installed/claimed ready'
    }
    Run-Test 'UpgradeCannotReplaceActiveIsolatedOwner' {
        $taskOwner='Global\YangLab-test-install-'+[guid]::NewGuid().ToString('N')
        $taskCreated=$false;$taskMutex=[Threading.Mutex]::new($false,$taskOwner,[ref]$taskCreated)
        try {Assert-Reject {& (Join-Path $SourceRoot 'App/scripts/check-upgrade.ps1') -OwnerName $taskOwner -IgnoreProcesses}} finally {$taskMutex.Dispose()}
        & (Join-Path $SourceRoot 'App/scripts/check-upgrade.ps1') -OwnerName $taskOwner -IgnoreProcesses|Out-Null
    }
} finally {
    $taskResolved=[IO.Path]::GetFullPath($taskScratch)
    if((Split-Path -Parent $taskResolved) -ne [IO.Path]::GetTempPath().TrimEnd('\','/') -or !(Split-Path -Leaf $taskResolved).StartsWith('Yang native 包 ')){throw 'Unsafe fixture cleanup target'}
    Remove-Item -LiteralPath $taskResolved -Recurse
}
if($taskFailures){throw "$taskFailures native packaging tests failed"}
