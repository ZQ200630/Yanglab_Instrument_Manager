param([string]$SourceRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..')))
$ErrorActionPreference='Stop'
$taskScratch=Join-Path ([IO.Path]::GetTempPath()) ('Yang native 包 '+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $taskScratch | Out-Null
$taskFailures=0
function Assert-True($Condition,$Message) { if (!$Condition) {throw $Message} }
function Assert-Reject([scriptblock]$Action,[string]$Pattern='') { $taskRejected=$false;try {& $Action | Out-Null}catch{$taskRejected=$true;if($Pattern){Assert-True ("$_" -match $Pattern) "Wrong rejection: $_"}}; Assert-True $taskRejected 'Operation unexpectedly accepted'; }
function Run-Test($Name,[scriptblock]$Action) { try {& $Action;Write-Output "PASS $Name"}catch{$script:taskFailures++;Write-Output "FAIL $Name : $_"} }
function New-Package($Name) {
    $taskRoot=Join-Path $taskScratch $Name;New-Item -ItemType Directory -Path $taskRoot | Out-Null
    foreach($taskBin in @('sil-instrument-console.exe','yang-lab-host.exe','yang-worker.exe')) {[IO.File]::WriteAllBytes((Join-Path $taskRoot $taskBin),[byte[]](77,90,1,2,3))}
    New-Item -ItemType Directory -Path (Join-Path $taskRoot 'App/catalog'),(Join-Path $taskRoot 'Config') | Out-Null
    Copy-Item -LiteralPath (Join-Path $SourceRoot 'App/catalog/devices.json') -Destination (Join-Path $taskRoot 'App/catalog/devices.json')
    Copy-Item -LiteralPath (Join-Path $SourceRoot 'Config/fiber_coupling.json') -Destination (Join-Path $taskRoot 'Config/fiber_coupling.json')
    $taskIdentity=@{schema=1;source_revision='test-source';package_revision='0.1.0';protocol_version=3;startup_revision=1;worker_sha256=(Get-FileHash (Join-Path $taskRoot 'yang-worker.exe')).Hash.ToLowerInvariant()}
    [IO.File]::WriteAllText((Join-Path $taskRoot 'native-package.json'),($taskIdentity|ConvertTo-Json))
    [IO.File]::WriteAllText((Join-Path $taskRoot 'THIRD_PARTY.txt'),'Native notices')
    return $taskRoot
}
try {
    Run-Test 'PackageRejectsPythonAndFixtures' {
        $taskRoot=New-Package 'reject'; & (Join-Path $SourceRoot 'App/scripts/check-package.ps1') -Root $taskRoot -SourceRoot $SourceRoot | Out-Null
        foreach($taskForbidden in @('main.py','cache.pyc','python.exe','python313.dll','wheel.whl','requirements.txt','environment.yml','worker-fixture.exe','trust.dpapi','recording.bin','source-launcher.ps1')) {
            $taskBad=Join-Path $taskRoot $taskForbidden;[IO.File]::WriteAllText($taskBad,'forbidden');
            Assert-Reject {& (Join-Path $SourceRoot 'App/scripts/check-package.ps1') -Root $taskRoot -SourceRoot $SourceRoot};Remove-Item -LiteralPath $taskBad
        }
    }
    Run-Test 'BuildPreservesEnvironmentAndRefusesExistingOutput' {
        $taskOld=@{PATH=$env:PATH;TAURI_CONFIG=$env:TAURI_CONFIG;CARGO_TARGET_DIR=$env:CARGO_TARGET_DIR;YANG_PACKAGE_REVISION=$env:YANG_PACKAGE_REVISION}
        $taskExisting=Join-Path $taskScratch 'existing';New-Item -ItemType Directory -Path $taskExisting|Out-Null;[IO.File]::WriteAllText((Join-Path $taskExisting 'keep.txt'),'keep');
        Assert-Reject {& (Join-Path $SourceRoot 'App/scripts/build-package.ps1') -SourceRoot $SourceRoot -Cargo (Get-Command pwsh).Source -TargetDir (Join-Path $taskScratch 'target') -Output $taskExisting -Offline} 'Output already exists'
        Assert-True ([IO.File]::ReadAllText((Join-Path $taskExisting 'keep.txt')) -eq 'keep') 'Existing output changed'
        Assert-Reject {& (Join-Path $SourceRoot 'App/scripts/build-native.ps1') -SourceRoot $SourceRoot -Cargo (Get-Command pwsh).Source -TargetDir (Join-Path $taskScratch 'failed') -Offline} 'Not Cargo'
        foreach($taskKey in $taskOld.Keys) {Assert-True ([Environment]::GetEnvironmentVariable($taskKey) -ceq $taskOld[$taskKey]) "Environment changed: $taskKey"}
    }
    Run-Test 'UnicodeRootResolvesNativeSidecars' {
        $taskRoot=New-Package '空格 包';$taskResult=& (Join-Path $SourceRoot 'App/scripts/check-package.ps1') -Root $taskRoot -SourceRoot $SourceRoot | ConvertFrom-Json
        Assert-True ($taskResult.WorkerHash -eq (Get-FileHash (Join-Path $taskRoot 'yang-worker.exe')).Hash.ToLowerInvariant()) 'Wrong worker in Unicode package'
        Assert-True (!$taskResult.PythonPayload) 'Unexpected Python payload'
    }
    Run-Test 'SuccessfulBuildRestoresAbsentEnvironment' {
        $taskMini=Join-Path $taskScratch 'source'
        foreach($taskRelative in @('App/protocol/src','App/worker-rs/src','App/src-tauri/src','App/src-tauri/windows','Code/Utils/src','Code/Setups/src','App/web','App/catalog','Config','App/runtime')) {New-Item -ItemType Directory -Path (Join-Path $taskMini $taskRelative) -Force|Out-Null}
        foreach($taskRelative in @('Cargo.toml','Cargo.lock','App/src-tauri/tauri.conf.json','App/src-tauri/build.rs','App/runtime/NATIVE_THIRD_PARTY.txt')) {Copy-Item -LiteralPath (Join-Path $SourceRoot $taskRelative) -Destination (Join-Path $taskMini $taskRelative)}
        $taskTool=Join-Path $taskScratch 'finite-cargo.ps1'
        $taskStub=@'
$Items=$args
if($Items -contains '--version') {'cargo 1.0.0';exit 0}
if($Items -contains 'metadata') {'{"packages":[]}';exit 0}
$taskDirectory=Join-Path $env:CARGO_TARGET_DIR 'x86_64-pc-windows-msvc/release';New-Item -ItemType Directory -Path $taskDirectory -Force|Out-Null
foreach($taskImage in @('yang-worker.exe','yang-lab-host.exe')) {[IO.File]::WriteAllBytes((Join-Path $taskDirectory $taskImage),[byte[]](77,90,1))}
exit 0
'@
        [IO.File]::WriteAllText($taskTool,$taskStub)
        $taskOriginal=[Environment]::GetEnvironmentVariable('YANG_PACKAGE_REVISION')
        Remove-Item Env:YANG_PACKAGE_REVISION -ErrorAction SilentlyContinue
        try {
            & (Join-Path $SourceRoot 'App/scripts/build-native.ps1') -SourceRoot $taskMini -Cargo $taskTool -TargetDir (Join-Path $taskScratch 'finite-target') -Offline|Out-Null
            Assert-True ($null -eq [Environment]::GetEnvironmentVariable('YANG_PACKAGE_REVISION')) 'Successful build converted an absent environment variable into an empty variable'
        } finally {if($null -eq $taskOriginal){Remove-Item Env:YANG_PACKAGE_REVISION -ErrorAction SilentlyContinue}else{[Environment]::SetEnvironmentVariable('YANG_PACKAGE_REVISION',$taskOriginal)}}
    }
    Run-Test 'MissingVendorDependencyIsNotInstalled' {
        $taskMissing=Join-Path $taskScratch 'missing-vendor';New-Item -ItemType Directory -Path $taskMissing|Out-Null
        $taskResult=& (Join-Path $SourceRoot 'App/scripts/check-dependencies.ps1') -SystemRoot $taskMissing | ConvertFrom-Json
        Assert-True (!$taskResult.VisaPresent) 'Missing VISA claimed present'
        Assert-True ($taskResult.InstalledAnything -eq $false) 'Dependency check installed software'
        Assert-True ((Get-ChildItem -LiteralPath $taskMissing -Force).Count -eq 0) 'Dependency check changed target'
    }
    Run-Test 'UpgradeCannotReplaceActiveOwner' {
        $taskOwner='Global\YangLab-test-install-'+[guid]::NewGuid().ToString('N')
        $taskCreated=$false;$taskMutex=[Threading.Mutex]::new($false,$taskOwner,[ref]$taskCreated)
        try {Assert-Reject {& (Join-Path $SourceRoot 'App/scripts/check-upgrade.ps1') -OwnerName $taskOwner -IgnoreProcesses}} finally {$taskMutex.Dispose()}
        & (Join-Path $SourceRoot 'App/scripts/check-upgrade.ps1') -OwnerName $taskOwner -IgnoreProcesses | Out-Null
    }
} finally {
    # Delete only this newly created and checked test directory.
    $taskResolved=[IO.Path]::GetFullPath($taskScratch)
    if ((Split-Path -Parent $taskResolved) -ne [IO.Path]::GetTempPath().TrimEnd('\','/') -or !(Split-Path -Leaf $taskResolved).StartsWith('Yang native 包 ')) {throw 'Unsafe fixture cleanup target'}
    Remove-Item -LiteralPath $taskResolved -Recurse
}
if($taskFailures) {throw "$taskFailures native packaging tests failed"}
