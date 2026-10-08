param([string]$OwnerName='Global\YangLabInstrumentHost',[switch]$IgnoreProcesses)
$ErrorActionPreference='Stop'
if($OwnerName -ne 'Global\YangLabInstrumentHost' -and !$OwnerName.StartsWith('Global\YangLab-test-')) {throw 'Unsupported owner namespace'}
try {
    $taskOwner=[Threading.Mutex]::OpenExisting($OwnerName)
    $taskOwner.Dispose()
    throw 'Active owner: close instruments and stop the Host normally; no process was killed'
} catch [Threading.WaitHandleCannotBeOpenedException] {
    # Absent object is the only permitted result. Access denied is not absence.
}
if(!$IgnoreProcesses -and (Get-Process -Name 'yang-lab-host','yang-worker','sil-instrument-console' -ErrorAction SilentlyContinue)) {throw 'Active App/Host/Worker: ordinary verified shutdown required before replacement'}
[PSCustomObject]@{ReplacementAllowed=$true;KilledAnything=$false}|ConvertTo-Json -Compress
