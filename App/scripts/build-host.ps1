param(
    [string]$SourceRoot=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..')),
    [string]$Cargo=(Get-Command cargo -ErrorAction Stop).Source,
    [string]$TargetDir=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../target')),
    [switch]$Offline
)
# Compatibility entry point delegates to native builds; no interpreter bootstrap.
& (Join-Path $PSScriptRoot 'build-native.ps1') -SourceRoot $SourceRoot -Cargo $Cargo -TargetDir $TargetDir -Offline:$Offline
