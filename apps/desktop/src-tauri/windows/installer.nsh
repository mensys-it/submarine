; Hooks of the NSIS installer made by Tauri for Windows.
;
; They register submarine-daemon as a Windows service and put the `submarine`
; CLI on the machine PATH. The daemon, the CLI, wintun.dll and the driver are
; installed side by side in $INSTDIR.
;
; NB: the Tauri template already includes StrFunc.nsh and declares ${StrLoc};
; ${UnStrRep} is declared here, since the uninstaller needs it.

; registry key of the machine environment variables
!define ENV_KEY "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"
${UnStrRep}

!macro NSIS_HOOK_PREINSTALL
  ; upgrade: stop and remove the old service, so its files can be replaced
  IfFileExists "$INSTDIR\submarine-daemon.exe" 0 +2
    nsExec::ExecToLog '"$INSTDIR\submarine-daemon.exe" service uninstall'
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; registration and start of the service
  nsExec::ExecToLog '"$INSTDIR\submarine-daemon.exe" service install'

  ; $INSTDIR added to the machine PATH once, then running programs notified
  ReadRegStr $0 HKLM "${ENV_KEY}" "Path"
  ${StrLoc} $1 "$0" "$INSTDIR" ">"
  StrCmp $1 "" 0 path_done
    WriteRegExpandStr HKLM "${ENV_KEY}" "Path" "$0;$INSTDIR"
    SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
  path_done:
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; removal of the service, which also removes the kill switch rules and the
  ; split tunnel driver
  nsExec::ExecToLog '"$INSTDIR\submarine-daemon.exe" service uninstall'

  ; $INSTDIR removed from the machine PATH, then running programs notified
  ReadRegStr $0 HKLM "${ENV_KEY}" "Path"
  ${UnStrRep} $1 "$0" ";$INSTDIR" ""
  WriteRegExpandStr HKLM "${ENV_KEY}" "Path" "$1"
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
!macroend
