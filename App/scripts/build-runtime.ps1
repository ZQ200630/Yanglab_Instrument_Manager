param(
    [Parameter(Mandatory=$true)][string]$Python,
    [Parameter(Mandatory=$true)][string]$Git,
    [Parameter(Mandatory=$true)][string]$SourceRoot,
    [Parameter(Mandatory=$true)][string]$SourceCommit,
    [Parameter(Mandatory=$true)][string]$InputRoot,
    [Parameter(Mandatory=$true)][string]$Destination
)
$ErrorActionPreference = 'Stop'
# Build-time only. No downloads, installation, hardware, PATH mutation or fallback.
if (![IO.Path]::IsPathRooted($Python) -or !(Test-Path -LiteralPath $Python -PathType Leaf)) {
    throw 'Supply the absolute Python executable of the development VISA environment.'
}
if (![IO.Path]::IsPathRooted($Git) -or !(Test-Path -LiteralPath $Git -PathType Leaf)) {
    throw 'Supply the absolute Git executable for source-commit verification.'
}
$taskIdentity = & $Python -B -c 'import json,sys; from pathlib import Path; print(json.dumps(dict(version=list(sys.version_info[:3]),environment=Path(sys.prefix).name)))'
if ($LASTEXITCODE -ne 0) { throw 'Development Python identity check failed.' }
$taskIdentity = $taskIdentity | ConvertFrom-Json
if (($taskIdentity.version -join '.') -ne '3.10.16' -or $taskIdentity.environment -cne 'VISA') {
    throw 'Ordinary build commands require development VISA with Python 3.10.16.'
}
$taskSource = [IO.Path]::GetFullPath($SourceRoot)
$taskInputs = [IO.Path]::GetFullPath($InputRoot)
$taskDestination = [IO.Path]::GetFullPath($Destination)
$taskLock = Join-Path $taskSource 'App/runtime/requirements-win-x64.lock'
$taskArchive = Join-Path $taskInputs 'python-3.13.16-embed-amd64.zip'
$taskWheels = Join-Path $taskInputs 'wheels'
$taskModuleRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
Push-Location -LiteralPath $taskModuleRoot
try {
    & $Python -B -m App.scripts.runtime_assemble --archive $taskArchive --wheels $taskWheels `
        --lock $taskLock --destination $taskDestination --source-root $taskSource `
        --source-commit $SourceCommit --git $Git
    if ($LASTEXITCODE -ne 0) { throw 'Private runtime assembly rejected its inputs; no candidate was admitted.' }
} finally {
    Pop-Location
}
