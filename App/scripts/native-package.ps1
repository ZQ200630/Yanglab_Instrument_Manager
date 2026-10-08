# Shared closed native resource/provenance contract. PowerShell 7 only.
$script:NativeDriverPins = '[{"id":"newport","kind":"msi","entry":"newport/USBDriverSetup64.msi","version":"5.0.8","files":[{"path":"newport/USBDriverSetup64.msi","size":14447104,"sha256":"8f8fafedf3adb94f922e6a1bd7cd14e5f8aee2f3b0f501a8fc2348927adfdb19"}]},{"id":"ch340","kind":"inf","entry":"ch340/CH341SER.INF","version":"4.0.2026.02","files":[{"path":"ch340/CH341M64.sys","size":63792,"sha256":"d1f001401a28e43e20a3ed9ccfd5267a11fef4a9846183bab6a2cdb10acb4237"},{"path":"ch340/CH341PORTS.DLL","size":61096,"sha256":"9e79736e949897b8ff8e959e2b6b67ead2ad061a4c63f72bfa70995023ef7e05"},{"path":"ch340/CH341PORTSA64.DLL","size":72872,"sha256":"98707b112d079e05241298d1ab19bac83e0a18d8334a5bc096c359a169838a0b"},{"path":"ch340/CH341PT.DLL","size":49280,"sha256":"222f91aaeeda5d8348552f0edbb9125948dbfa38daccefab7ca001efc70ffc74"},{"path":"ch340/CH341PTA64.DLL","size":67704,"sha256":"cb0fb42b6b3ba62aff1999132df9a41f3993f7f79ede4b020048240934bf4f91"},{"path":"ch340/CH341S64.sys","size":74384,"sha256":"83078d87254ee4f52f08498c0458f27308c7ab857278eda59f8ec5bfd5e1d5ce"},{"path":"ch340/CH341SER.CAT","size":14510,"sha256":"b5f2ef9fcdfaac86243d47b26e77bc3378cabdef18eba3e04975c6403756bd28"},{"path":"ch340/CH341SER.INF","size":9390,"sha256":"1bbb1dfe5e311f53c2ae3928493dd884cabc12f992c7414a9509b28bf1f729df"},{"path":"ch340/CH341SER.sys","size":53904,"sha256":"bd849760a396ce7e9b0bf04ce49d528db63cae2056a0b3187363c602c15110cf"}]},{"id":"cp210x","kind":"inf","entry":"cp210x/silabser.inf","version":"11.3.0","files":[{"path":"cp210x/arm/silabser.sys","size":130560,"sha256":"746b9638f8331f924102caab1fa4201a0324b2533e4b9453d1a48bbd71edbdb1"},{"path":"cp210x/arm64/silabser.sys","size":151560,"sha256":"a1fd87fbcd10ea0b1719a2c4beaae75f8fead0247da319d1075cc47baa31672c"},{"path":"cp210x/CP210x_Universal_Windows_Driver_ReleaseNotes.txt","size":30389,"sha256":"efc9b602bfd864675ac364a10b33568b25ae5bad8f412010984dcafec5c2d17f"},{"path":"cp210x/silabser.cat","size":13719,"sha256":"71389a32fa3056c553e648ae81096817a558c77be06531953b891d933958fd90"},{"path":"cp210x/silabser.inf","size":13718,"sha256":"73faecf059e63150c42251d277611c86fdfede4562da752ae3b5ba28aef76a57"},{"path":"cp210x/SLAB_License_Agreement_VCP_Windows.txt","size":8371,"sha256":"d423c0202937441271dc0bc445ae3f9e024678cafd3070aaf104f345eef8885e"},{"path":"cp210x/x64/silabser.sys","size":153608,"sha256":"9ad2bea7c5a489891ff05d3d438020171857ab20b0ffaf6e13bef9b15b469fec"},{"path":"cp210x/x86/silabser.sys","size":135216,"sha256":"239d0cf5b7f1f5e7af8db79ddec4dd08bd24a7e7b70607361600a63c17fc7bad"}]}]' | ConvertFrom-Json
function Read-NativeJson([string]$Path) {
    $taskBytes = [IO.File]::ReadAllBytes($Path)
    if ($taskBytes.Length -gt 65536) { throw 'JSON exceeds native manifest bound' }
    $taskDocument = [System.Text.Json.JsonDocument]::Parse([Text.Encoding]::UTF8.GetString($taskBytes))
    try {
        $taskStack = [Collections.Generic.Stack[System.Text.Json.JsonElement]]::new()
        $taskStack.Push($taskDocument.RootElement)
        while ($taskStack.Count) {
            $taskElement = $taskStack.Pop()
            if ($taskElement.ValueKind -eq [System.Text.Json.JsonValueKind]::Object) {
                $taskNames = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
                foreach ($taskProperty in $taskElement.EnumerateObject()) {
                    if (!$taskNames.Add($taskProperty.Name)) { throw 'Duplicate native manifest key' }
                    $taskStack.Push($taskProperty.Value)
                }
            } elseif ($taskElement.ValueKind -eq [System.Text.Json.JsonValueKind]::Array) {
                foreach ($taskValue in $taskElement.EnumerateArray()) { $taskStack.Push($taskValue) }
            }
        }
    } finally { $taskDocument.Dispose() }
    [Text.Encoding]::UTF8.GetString($taskBytes) | ConvertFrom-Json
}
function Assert-NativePath([string]$Path) {
    if (![IO.Path]::IsPathRooted($Path)) { throw 'Absolute native path required' }
    $taskPath = [IO.Path]::GetFullPath($Path)
    while ($taskPath) {
        $taskItem = Get-Item -LiteralPath $taskPath -Force -ErrorAction Stop
        if ($taskItem.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Native reparse point refused' }
        $taskParent = [IO.Path]::GetDirectoryName($taskPath.TrimEnd('\','/'))
        if (!$taskParent -or $taskParent -eq $taskPath) { break }
        $taskPath = $taskParent
    }
}
function Get-NativeResources {
    $taskMap = [ordered]@{
        'native-package.json' = 'App/runtime/native-payload/native-package.json'
        'THIRD_PARTY.txt' = 'App/runtime/native-payload/THIRD_PARTY.txt'
        'App/catalog/devices.json' = 'App/catalog/devices.json'
        'Config/fiber_coupling.json' = 'Config/fiber_coupling.json'
        'drivers/packages.json' = 'App/drivers/packages.json'
    }
    foreach ($taskPackage in $script:NativeDriverPins) {
        foreach ($taskFile in $taskPackage.files) { $taskMap['drivers/' + $taskFile.path] = 'App/drivers/' + $taskFile.path }
        $taskMap['drivers/' + $taskPackage.id + '/README.md'] = 'App/drivers/' + $taskPackage.id + '/README.md'
    }
    return $taskMap
}
function Assert-NativeDrivers([string]$SourceRoot, [string]$DriverRoot) {
    $taskSourceManifest = Join-Path $SourceRoot 'App/drivers/packages.json'
    $taskApproved = $script:NativeDriverPins | ConvertTo-Json -Depth 12 -Compress
    foreach ($taskManifestPath in @($taskSourceManifest, (Join-Path $DriverRoot 'packages.json'))) {
        Assert-NativePath $taskManifestPath
        $taskManifest = Read-NativeJson $taskManifestPath
        if (($taskManifest | ConvertTo-Json -Depth 12 -Compress) -cne $taskApproved) { throw 'Fixed driver manifest differs from approved 3-package/18-file pins' }
    }
    if ((Get-FileHash -LiteralPath $taskSourceManifest -Algorithm SHA256).Hash -cne (Get-FileHash -LiteralPath (Join-Path $DriverRoot 'packages.json') -Algorithm SHA256).Hash) { throw 'Driver manifest bytes differ from source' }
    foreach ($taskPackage in $script:NativeDriverPins) {
        foreach ($taskPin in $taskPackage.files) {
            $taskPath = Join-Path $DriverRoot $taskPin.path
            Assert-NativePath $taskPath
            $taskFile = Get-Item -LiteralPath $taskPath -Force
            if ($taskFile.PSIsContainer -or $taskFile.Length -ne $taskPin.size -or (Get-FileHash -LiteralPath $taskPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne $taskPin.sha256) { throw "Modified fixed driver resource: $($taskPin.path)" }
        }
    }
}
function Get-NativeFiles([string]$Root) {
    Assert-NativePath $Root
    $taskPending = [Collections.Generic.Queue[string]]::new()
    $taskPending.Enqueue($Root)
    while ($taskPending.Count) {
        foreach ($taskItem in Get-ChildItem -LiteralPath $taskPending.Dequeue() -Force) {
            if ($taskItem.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Native reparse point refused' }
            if ($taskItem.PSIsContainer) { $taskPending.Enqueue($taskItem.FullName) } else { $taskItem }
        }
    }
}
function Get-NativeSourceFingerprint([string]$SourceRoot) {
    $taskSource = [IO.Path]::GetFullPath($SourceRoot).TrimEnd('\','/')
    Assert-NativePath $taskSource
    $taskInputs = @()
    foreach ($taskFolder in @('App/protocol/src','App/worker-rs/src','App/src-tauri/src','Code/Utils/src','Code/Setups/src','Code/Utils/tlb_native/src','App/web','App/catalog','Config','App/src-tauri/windows')) {
        $taskInputs += @(Get-NativeFiles (Join-Path $taskSource $taskFolder))
    }
    foreach ($taskFile in @('Cargo.toml','Cargo.lock','App/src-tauri/tauri.conf.json','App/src-tauri/build.rs','App/runtime/NATIVE_THIRD_PARTY.txt','Code/Utils/tlb_native/Cargo.toml','Code/Utils/tlb_models.json','App/drivers/packages.json','App/scripts/native-package.ps1','App/scripts/build-native.ps1','App/scripts/build-package.ps1','App/scripts/build-host.ps1','App/scripts/check-package.ps1')) {
        $taskInputs += Get-Item -LiteralPath (Join-Path $taskSource $taskFile) -Force
    }
    $taskInputs += Get-ChildItem -Path (Join-Path $taskSource 'App/*/Cargo.toml'),(Join-Path $taskSource 'Code/*/Cargo.toml') -File
    $taskInputs += @(Get-NativeFiles (Join-Path $taskSource 'App/drivers'))
    $taskLines = @($taskInputs | Sort-Object FullName -Unique | ForEach-Object {
        Assert-NativePath $_.FullName
        $taskRelative = $_.FullName.Substring($taskSource.Length).Replace('\','/')
        "$taskRelative $((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash)"
    })
    $taskBytes = [Text.Encoding]::UTF8.GetBytes($taskLines -join "`n")
    [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($taskBytes)).ToLowerInvariant()
}
function Assert-NativeBundleResources([string]$SourceRoot) {
    $taskConfig = Read-NativeJson (Join-Path $SourceRoot 'App/src-tauri/tauri.conf.json')
    $taskExpected = Get-NativeResources
    $taskActual = @{}
    foreach ($taskProperty in $taskConfig.bundle.resources.PSObject.Properties) {
        if ($taskActual.ContainsKey($taskProperty.Value)) { throw 'Duplicate bundle resource destination' }
        $taskActual[$taskProperty.Value] = [IO.Path]::GetFullPath((Join-Path (Join-Path $SourceRoot 'App/src-tauri') $taskProperty.Name))
    }
    if ($taskActual.Count -ne $taskExpected.Count) { throw 'Native bundle resource count mismatch' }
    foreach ($taskDestination in $taskExpected.Keys) {
        if ($taskActual[$taskDestination] -cne [IO.Path]::GetFullPath((Join-Path $SourceRoot $taskExpected[$taskDestination]))) { throw "Native bundle resource mismatch: $taskDestination" }
    }
    if (($taskConfig.bundle.externalBin -join ',') -cne 'binaries/yang-lab-host,binaries/yang-worker') { throw 'Only native Host/Worker sidecars allowed' }
}