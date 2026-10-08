# Historical private Python runtime assembly is retired.
# Its reference sources remain excluded from every native build and package.
param([Parameter(ValueFromRemainingArguments=$true)][object[]]$LegacyArguments)
$ErrorActionPreference = 'Stop'
throw 'The private Python runtime builder is retired. Use App/scripts/build-native.ps1 or build-package.ps1 for the native Rust application.'
