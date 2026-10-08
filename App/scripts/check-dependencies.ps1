param([string]$SystemRoot=$env:SystemRoot)
$ErrorActionPreference='Stop'
if (![IO.Path]::IsPathRooted($SystemRoot)) {throw 'Absolute Windows directory required'}
$taskVisa=Test-Path -LiteralPath (Join-Path $SystemRoot 'System32/visa64.dll') -PathType Leaf
$taskWebView=$false
if ($SystemRoot -eq $env:SystemRoot) {
    foreach ($taskHive in @('HKCU','HKLM')) {
        foreach ($taskSuffix in @('','WOW6432Node/')) {
            $taskKey="${taskHive}:/SOFTWARE/${taskSuffix}Microsoft/EdgeUpdate/Clients/{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
            $taskVersion=(Get-ItemProperty -LiteralPath $taskKey -Name pv -ErrorAction SilentlyContinue).pv
            if ($taskVersion -and $taskVersion -ne '0.0.0.0') {$taskWebView=$true}
        }
    }
}
[PSCustomObject]@{VisaPresent=$taskVisa;WebView2Present=$taskWebView;SerialDriverCheck='Windows device-specific; see Device setup status';InstalledAnything=$false;MissingVisaEffect='VISA devices unavailable; serial and archive viewing remain usable'}|ConvertTo-Json -Compress
