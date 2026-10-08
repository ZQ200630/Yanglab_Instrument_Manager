; No process termination, instrument commands or vendor installation here.
!macro YANG_CHECK_OWNER
  System::Call 'kernel32::OpenMutexW(i 0x100000, i 0, w "Global\YangLabInstrumentHost") p.r0 ?e'
  Pop $1
  ${If} $0 != 0
    System::Call 'kernel32::CloseHandle(p r0)'
    MessageBox MB_OK|MB_ICONSTOP "An instrument Host or diagnostic is active. Disconnect instruments and stop it normally before upgrading. No process was stopped."
    Abort
  ${EndIf}
  ${If} $1 != 2
    MessageBox MB_OK|MB_ICONSTOP "Cannot verify instrument ownership. Close the App, Host and Worker normally, then retry."
    Abort
  ${EndIf}
  ; Catch a surviving Worker or network-only GUI even after Host crash. The
  ; 32-bit NSIS process uses the 556-byte PROCESSENTRY32W structure.
  System::Call 'kernel32::CreateToolhelp32Snapshot(i 2, i 0) p.r0'
  ${If} $0 == -1
    MessageBox MB_OK|MB_ICONSTOP "Cannot inspect active App processes. Upgrade refused."
    Abort
  ${EndIf}
  System::Alloc 556
  Pop $1
  System::Call '*$1(i 556)'
  System::Call 'kernel32::Process32FirstW(p r0, p r1) i.r2 ?e'
  Pop $5
  ${DoWhile} $2 != 0
    IntOp $3 $1 + 36
    System::Call '*$3(&w260 .r4)'
    ${If} $4 == "yang-worker.exe"
    ${OrIf} $4 == "yang-lab-host.exe"
    ${OrIf} $4 == "sil-instrument-console.exe"
      System::Free $1
      System::Call 'kernel32::CloseHandle(p r0)'
      MessageBox MB_OK|MB_ICONSTOP "Yang Lab App/Host/Worker is still running. Use ordinary verified shutdown before upgrading."
      Abort
    ${EndIf}
  System::Call 'kernel32::Process32NextW(p r0, p r1) i.r2 ?e'
  Pop $5
  ${Loop}
  System::Free $1
  System::Call 'kernel32::CloseHandle(p r0)'
  ${If} $5 != 18
    MessageBox MB_OK|MB_ICONSTOP "Process inspection did not finish. Upgrade refused."
    Abort
  ${EndIf}
!macroend
Var YangInstallOwner
!macro YANG_HOLD_OWNER
  !insertmacro YANG_CHECK_OWNER
  System::Call 'kernel32::CreateMutexW(p 0, i 0, w "Global\YangLabInstrumentHost") p.r0 ?e'
  Pop $1
  ${If} $0 == 0
  ${OrIf} $1 == 183
    ${If} $0 != 0
      System::Call 'kernel32::CloseHandle(p r0)'
    ${EndIf}
    MessageBox MB_OK|MB_ICONSTOP "An owner started during the check. Upgrade refused."
    Abort
  ${EndIf}
  StrCpy $YangInstallOwner $0
!macroend
!macro NSIS_HOOK_PREINSTALL
  !insertmacro YANG_HOLD_OWNER
  IfFileExists "$INSTDIR\App\worker\main.py" 0 +3
    MessageBox MB_OK|MB_ICONSTOP "A legacy Python installation is present. After ordinary shutdown, select a new empty installation folder; old files are not automatically deleted or migrated."
    Abort
  ReadRegStr $0 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  ${If} $0 == ""
    SetRegView 32
    ReadRegStr $0 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
    SetRegView 64
    ${If} $0 == ""
      ReadRegStr $0 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
    ${EndIf}
  ${EndIf}
  ${If} $0 == ""
  ${OrIf} $0 == "0.0.0.0"
    MessageBox MB_OK|MB_ICONSTOP "Microsoft Edge WebView2 Runtime is required. Install it from Microsoft, then retry. This installer does not install VISA or serial vendor drivers."
    Abort
  ${EndIf}
!macroend
!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro YANG_HOLD_OWNER
!macroend
!macro YANG_RELEASE_OWNER
  ${If} $YangInstallOwner != 0
    System::Call 'kernel32::CloseHandle(p $YangInstallOwner)'
    StrCpy $YangInstallOwner 0
  ${EndIf}
!macroend
!macro NSIS_HOOK_POSTINSTALL
  !insertmacro YANG_RELEASE_OWNER
!macroend
!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro YANG_RELEASE_OWNER
!macroend
