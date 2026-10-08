param([switch]$Offline)
$ErrorActionPreference = 'Stop'
$taskSource = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$taskCargo = (Get-Command cargo -ErrorAction Stop).Source
$taskNode = (Get-Command node -ErrorAction Stop).Source
$taskOldPath = $env:PATH
$taskOldConfig = $env:TAURI_CONFIG
$taskCargoDir = Split-Path -Parent $taskCargo
$taskNodeDir = Split-Path -Parent $taskNode
$taskRustupDir = if (Get-Command rustup -ErrorAction SilentlyContinue) { Split-Path -Parent (Get-Command rustup).Source } else { $taskCargoDir }
$taskBuildDirs = @($taskOldPath.Split(';') | Where-Object { $_ -match 'Microsoft|Windows Kits|System32|Git' -and $_ -notmatch 'Python|Anaconda|Miniconda|WindowsApps' })
$taskFlags = @('--locked')
if ($Offline) { $taskFlags += '--offline' }
Push-Location $taskSource
try {
    $env:PATH = (@($taskCargoDir,$taskRustupDir,$taskNodeDir,"$env:SystemRoot/System32") + $taskBuildDirs | Select-Object -Unique) -join ';'
    $env:TAURI_CONFIG = '{"bundle":{"externalBin":[],"resources":[]}}'
    if (Get-Command python,python3,conda -ErrorAction SilentlyContinue) { throw 'Python/conda still appears on the native test PATH' }
    & $taskCargo build @taskFlags -p yang-worker -p yang-debug --bins
    if ($LASTEXITCODE -ne 0) { throw 'Native Worker/diagnostic fixture build failed' }
    & $taskCargo test @taskFlags --workspace --features host-bin -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'Native regression failed' }
    $taskTests = @(Get-ChildItem -LiteralPath (Join-Path $taskSource 'App/tests') -Filter '*.test.mjs' -File | ForEach-Object { $_.FullName })
    & $taskNode --test @taskTests
    if ($LASTEXITCODE -ne 0) { throw 'Frontend regression failed' }
    & (Join-Path $PSScriptRoot 'tests/native.Tests.ps1') -SourceRoot $taskSource
} finally {
    $env:PATH = $taskOldPath
    $env:TAURI_CONFIG = $taskOldConfig
    Pop-Location
}
