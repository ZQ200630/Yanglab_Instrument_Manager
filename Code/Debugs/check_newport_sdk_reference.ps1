<#
Bounded comparison against the installed manufacturer's managed SDK. This is
a diagnostic, not an application backend. It sends only the fixed identity
and status getters below, and never changes output or front-panel settings.
#>
param(
    [Parameter(Mandatory=$true)][ValidatePattern('^6700 SN[0-9]{1,16}$')][string]$DeviceKey,
    [switch]$ConfirmReadonly
)
$ErrorActionPreference='Stop'
if(!$ConfirmReadonly){throw 'Separate read-only authorization required: -ConfirmReadonly'}
$taskResult=@{kind='official-sdk-readonly-reference';status='failed';identity=@{};queries=@();samples=@();close_called=$false;physical_state='not_measured'}
$taskSdk=$null
try {
    Add-Type -TypeDefinition @"
public static class SDKLibrarySearch {
    [System.Runtime.InteropServices.DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool SetDefaultDllDirectories(uint flags);
    [System.Runtime.InteropServices.DllImport("kernel32.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode, SetLastError=true)]
    public static extern System.IntPtr AddDllDirectory(string path);
}
"@
    if(![SDKLibrarySearch]::SetDefaultDllDirectories(4096)){throw 'Cannot restrict native DLL search'}
    $taskDirectory=[SDKLibrarySearch]::AddDllDirectory('C:/Program Files/Newport/Newport USB Driver/Bin')
    if($taskDirectory -eq [IntPtr]::Zero){throw 'Cannot register installed SDK directory'}
    [void][Reflection.Assembly]::LoadFrom('C:/Program Files/Newport/Newport USB Driver/Samples/UsbDllWrap.dll')
    $taskSdk=New-Object Newport.USBComm.USB -ArgumentList $true
    # The managed SDK's boolean enables key addressing. Its C++ wrapper
    # inverts that value before calling the native index-addressing API.
    if(!$taskSdk.OpenDevices(4106,$true)){throw 'SDK OpenDevices failed'}
    $taskKeys=@($taskSdk.GetDeviceTable().Keys | ForEach-Object {$_})
    $taskResult.keys=$taskKeys
    if($DeviceKey -notin $taskKeys){throw 'Exact controller key absent'}
    function QueryReadonly([string]$taskCommand) {
        if($taskCommand -notin @('*IDN?','SYSTem:LASer:MODEL?','SYSTem:LASer:SN?','OUTPut:STATe?','OUTPut:TRACk?','SYSTem:MCONT?','SOURce:CPOWer?','SENSe:WAVElength','SOURce:WAVElength?','SENSe:POWer:DIODe','SOURce:POWer:DIODe?','SENSe:CURRent:DIODe','SOURce:CURRent:DIODe?','SOURce:VOLTage:PIEZo?','*OPC?','*STB?')){throw 'Unreviewed query'}
        Start-Sleep -Milliseconds 200
        $taskAnswer=New-Object System.Text.StringBuilder 64
        $taskReturn=$taskSdk.Query($DeviceKey,$taskCommand,$taskAnswer)
        $taskText=($taskAnswer.ToString() -split "`r`n")[0].Trim()
        $taskResult.queries+=@(@{command=$taskCommand;code=$taskReturn;reply=$taskText})
        if($taskReturn -ne 0){throw ('SDK Query failed: '+$taskCommand+', code '+$taskReturn)}
        if(!$taskText -or $taskText -match 'COMMAND NOT VALID|NO PARAMETER SPECIFIED'){throw ('Bad SDK response: '+$taskCommand+' -> '+$taskText)}
        return $taskText
    }
    $taskId=QueryReadonly '*IDN?'
    if($taskId -notmatch ('^New_Focus\s+6700\s+v\S+\s+\S+\s+SN'+[regex]::Escape($DeviceKey.Substring(7))+'$')){throw 'Controller identity mismatch'}
    $taskResult.identity.controller=$taskId
    $taskResult.identity.head_model=QueryReadonly 'SYSTem:LASer:MODEL?'
    $taskResult.identity.head_serial=QueryReadonly 'SYSTem:LASer:SN?'
    for($taskSample=0;$taskSample -lt 3;$taskSample++) {
        $taskReadback=@{}
        foreach($taskCommand in @('OUTPut:STATe?','OUTPut:TRACk?','SYSTem:MCONT?','SOURce:CPOWer?','SENSe:WAVElength','SOURce:WAVElength?','SENSe:POWer:DIODe','SOURce:POWer:DIODe?','SENSe:CURRent:DIODe','SOURce:CURRent:DIODe?','SOURce:VOLTage:PIEZo?','*OPC?','*STB?')){
            $taskReadback[$taskCommand]=QueryReadonly $taskCommand
        }
        $taskResult.samples+=@($taskReadback)
    }
    $taskResult.status='passed'
} catch {$taskResult.error=$_.Exception.Message}
finally {
    if($taskSdk){try{$taskSdk.CloseDevices();$taskResult.close_called=$true}catch{$taskResult.close_error=$_.Exception.Message}}
    $taskResult | ConvertTo-Json -Depth 6
}
if($taskResult.status -ne 'passed'){exit 1}
