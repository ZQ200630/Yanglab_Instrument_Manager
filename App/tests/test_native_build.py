"""Native artifact selection and staged finite build tools; no hardware."""
import importlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
POWERSHELL = Path('C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe')

# Finite offline tool doubles. All files and artifacts stay inside a staged repo;
# no MSVC build, real Host binary or instrument is opened by these tests.
HARNESS = r'''
param([string]$Stage, [string]$FailMode)
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSHOME 'Modules/Microsoft.PowerShell.Utility/Microsoft.PowerShell.Utility.psd1')
$taskTools = Join-Path $Stage 'tools'
$taskShell = Join-Path $Stage 'dev shell.ps1'
@'
param([string]$Arch, [string]$HostArch, [switch]$SkipAutomaticLocation)
$env:PATH = $env:PATH + ';finite-msvc-path'
$env:INCLUDE = 'finite-include'
$env:LIB = $PSScriptRoot
$env:YANG_LAB_TEST_NEW_ENV = 'added'
Remove-Item Env:YANG_LAB_TEST_REMOVED_ENV
$env:YANG_LAB_TEST_EMPTY_ENV = 'changed'
'@ | Set-Content -LiteralPath $taskShell -Encoding UTF8
$taskCargo = Join-Path $Stage 'finite cargo.ps1'
@'
if ($env:YANG_LAB_TEST_BUILD_FAIL -eq '1') { $global:LASTEXITCODE = 1; return }
$taskName = if ($args -contains 'yang-lab-tlb') { 'yang-lab-tlb.exe' } else { 'yang-lab-host.exe' }
$taskOutput = Join-Path $env:CARGO_TARGET_DIR ('debug/' + $taskName)
New-Item -ItemType Directory -Path (Split-Path $taskOutput -Parent) -Force | Out-Null
[IO.File]::WriteAllBytes($taskOutput, [byte[]](1, 2, 3))
$global:LASTEXITCODE = 0
'@ | Set-Content -LiteralPath $taskCargo -Encoding UTF8
[Environment]::SetEnvironmentVariable('ProgramFiles(x86)', $taskTools, 'Process')
$env:YANG_LAB_TEST_DEV_SHELL = $taskShell
$env:YANG_LAB_TEST_BUILD_FAIL = $FailMode
$env:YANG_LAB_TEST_REMOVED_ENV = 'original'
Add-Type -TypeDefinition @'
public static class FiniteEnvironment {
    [System.Runtime.InteropServices.DllImport("kernel32.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode)]
    public static extern bool SetEnvironmentVariable(string name, string value);
}
'@
[FiniteEnvironment]::SetEnvironmentVariable('YANG_LAB_TEST_EMPTY_ENV', '') | Out-Null
$env:INCLUDE = 'original-include'
$taskOriginalLib = Join-Path $Stage 'original lib'
New-Item -ItemType Directory -Path $taskOriginalLib | Out-Null
$env:LIB = $taskOriginalLib
$env:TAURI_CONFIG = 'original-config'
$env:CARGO_TARGET_DIR = 'original-target'
$taskBefore = [Environment]::GetEnvironmentVariables('Process')
$taskFailure = $null
try {
    & (Join-Path $Stage 'App/scripts/build-host.ps1') -Offline -Profile debug `
        -Cargo $taskCargo -TargetDir 'target with spaces' | Out-Null
} catch { $taskFailure = $_.Exception.Message }
$taskAfter = [Environment]::GetEnvironmentVariables('Process')
$taskNames = @($taskBefore.Keys) + @($taskAfter.Keys) | Sort-Object -Unique
$taskChanged = @($taskNames | Where-Object { $taskBefore[$_] -cne $taskAfter[$_] })
@{changes=$taskChanged; error=$taskFailure;
  native=(Test-Path -LiteralPath (Join-Path $Stage 'App/src-tauri/binaries/yang-lab-tlb.exe'));
  artifact=(Test-Path -LiteralPath (Join-Path $Stage 'App/src-tauri/binaries/yang-lab-host-x86_64-pc-windows-msvc.exe'))
} | ConvertTo-Json -Compress
'''

# Use Windows PowerShell's framework compiler for a self-contained finite exe;
# the subsequent build harness can then exercise either PowerShell runtime.
SHIM_BUILDER = r'''
param([string]$Stage)
$ErrorActionPreference = 'Stop'
$taskOutput = Join-Path $Stage 'tools/Microsoft Visual Studio/Installer/vswhere.exe'
New-Item -ItemType Directory -Path (Split-Path $taskOutput -Parent) -Force | Out-Null
Add-Type -TypeDefinition @'
using System;
public class FiniteVswhere {
    public static void Main(string[] args) {
        Console.WriteLine(Environment.GetEnvironmentVariable("YANG_LAB_TEST_DEV_SHELL"));
    }
}
'@ -OutputAssembly $taskOutput -OutputType ConsoleApplication
'''


class NativeBuildTests(unittest.TestCase):
    def shells(self):
        shells = [POWERSHELL]
        if os.environ.get('YANG_LAB_TEST_POWERSHELL'):
            explicit = Path(os.environ['YANG_LAB_TEST_POWERSHELL'])
            if not explicit.is_absolute() or not explicit.is_file():
                raise ValueError('YANG_LAB_TEST_POWERSHELL must be an existing absolute executable')
            if explicit != POWERSHELL:
                shells.append(explicit)
        return shells

    def build_environment(self, failure, shell):
        with tempfile.TemporaryDirectory(prefix='finite native build ') as directory:
            stage = Path(directory)
            scripts = stage / 'App/scripts'
            scripts.mkdir(parents=True)
            shutil.copyfile(ROOT / 'App/scripts/build-host.ps1', scripts / 'build-host.ps1')
            builder = stage / 'finite shim.ps1'
            builder.write_text(SHIM_BUILDER, encoding='utf-8-sig')
            subprocess.run([str(POWERSHELL), '-NoProfile', '-ExecutionPolicy', 'Bypass',
                            '-File', str(builder), '-Stage', str(stage)],
                           capture_output=True, text=True, timeout=30, check=True)
            harness = stage / 'harness.ps1'
            harness.write_text(HARNESS, encoding='utf-8-sig')
            result = subprocess.run([str(shell), '-NoProfile', '-ExecutionPolicy', 'Bypass',
                                     '-File', str(harness), '-Stage', str(stage),
                                     '-FailMode', '1' if failure else '0'],
                                    capture_output=True, text=True, timeout=30, check=True)
            return json.loads(result.stdout.strip())

    def test_successful_build_preserves_complete_callers_environment(self):
        for shell in self.shells():
            with self.subTest(shell=str(shell)):
                result = self.build_environment(False, shell)
                self.assertIsNone(result['error'])
                self.assertTrue(result['artifact'])
                self.assertTrue(result['native'])
                self.assertEqual(result['changes'], [])

    def test_failed_build_preserves_complete_callers_environment(self):
        for shell in self.shells():
            with self.subTest(shell=str(shell)):
                result = self.build_environment(True, shell)
                self.assertIn('build failed', result['error'])
                self.assertFalse(result['artifact'])
                self.assertEqual(result['changes'], [])

    def binary(self, environ):
        return importlib.import_module('App.tests.host_build_fixture').native_host_binary(environ, root=ROOT)

    def test_default_is_this_repositories_debug_host(self):
        self.assertEqual(self.binary({}), ROOT / 'App/src-tauri/target/debug/yang-lab-host.exe')

    def test_relative_cargo_target_is_relative_to_repository(self):
        self.assertEqual(self.binary({'CARGO_TARGET_DIR':'Result/native target'}),
                         ROOT / 'Result/native target/debug/yang-lab-host.exe')

    def test_explicit_absolute_artifact_overrides_target_directory(self):
        artifact = ROOT / 'Result/verified build/yang-lab-host.exe'
        self.assertEqual(self.binary({'YANG_LAB_TEST_HOST':str(artifact),'CARGO_TARGET_DIR':'unused'}), artifact)

    def test_relative_explicit_artifact_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'absolute'):
            self.binary({'YANG_LAB_TEST_HOST':'other/yang-lab-host.exe'})


if __name__ == '__main__':
    unittest.main()
