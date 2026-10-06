param([switch]$Offline)
$ErrorActionPreference = 'Stop'
$taskRoot = 'D:/Qian/Codex_Project/SIL_Experiments/tmp/tauri-build'
$taskCargo = "$taskRoot/rustup/toolchains/stable-x86_64-pc-windows-msvc/bin/cargo.exe"
if (!(Test-Path -LiteralPath $taskCargo)) { throw 'Existing offline Rust cache is missing; no download is permitted.' }
& 'D:/SoftwareInstaller/MicrosoftBuildTools2022/Common7/Tools/Launch-VsDevShell.ps1' -Arch amd64 -HostArch amd64 -SkipAutomaticLocation
$env:RUSTUP_HOME = "$taskRoot/rustup"
$env:CARGO_HOME = "$taskRoot/cargo"
$env:CARGO_TARGET_DIR = "$taskRoot/target"
$env:TEMP = "$taskRoot/temp"
$env:TMP = $env:TEMP
$env:CARGO_NET_OFFLINE = 'true'
$env:PATH = "$taskRoot/rustup/toolchains/stable-x86_64-pc-windows-msvc/bin;$env:CARGO_HOME/bin;$env:PATH"
$taskManifest = Join-Path $PSScriptRoot '../src-tauri/Cargo.toml'
$taskPreviousConfig = $env:TAURI_CONFIG
try {
    # Bootstrap only the Host target before its generated sidecar exists.
    $env:TAURI_CONFIG = '{"bundle":{"externalBin":[]}}'
    & $taskCargo build --offline --locked --release --manifest-path $taskManifest --features host-bin --bin yang-lab-host
    if ($LASTEXITCODE -ne 0) { throw 'Offline Host build failed. Do not substitute or download another binary.' }
} finally { $env:TAURI_CONFIG = $taskPreviousConfig }
$taskGenerated = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../src-tauri/binaries/yang-lab-host-x86_64-pc-windows-msvc.exe'))
Copy-Item -LiteralPath "$taskRoot/target/release/yang-lab-host.exe" -Destination $taskGenerated -Force
Get-FileHash -LiteralPath $taskGenerated -Algorithm SHA256
