; Hooks of the NSIS installer made by Tauri for Windows.
;
; They register submarine-daemon as a Windows service, ask on the first
; installation who may use it, and put the `submarine` CLI on the machine PATH.
; The daemon, the CLI, wintun.dll and the driver are installed side by side in
; $INSTDIR.
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
  ; who may use the service, asked on the first installation only: an upgrade
  ; keeps the choice in access.json, and a silent installation allows every user
  ExpandEnvStrings $2 "%ProgramData%\Submarine\access.json"
  IfFileExists "$2" access_done
    ; the user of this desktop session, as DOMAIN\user
    ; NB: NOT the account the installer runs as, which is another administrator
    ; when the UAC prompt asked for different credentials
    nsExec::ExecToStack `powershell -NoProfile -NonInteractive -Command "$$s = (Get-Process -Id $$PID).SessionId; $$p = Get-CimInstance Win32_Process -Filter 'Name=''explorer.exe''' | Where-Object SessionId -eq $$s | Select-Object -First 1; if ($$p) { $$o = Invoke-CimMethod -InputObject $$p -MethodName GetOwner; [Console]::Out.Write($$o.Domain + '\' + $$o.User) }"`
    Pop $3
    Pop $4
    ; without a known user there is no "only me" to offer
    StrCmp $3 "0" 0 access_all
    StrCmp $4 "" access_all
    MessageBox MB_YESNO|MB_ICONQUESTION "Install Submarine for all the users of this computer?$\r$\n$\r$\nYes: every user may use it.$\r$\nNo: only $4 and the administrators may use it." /SD IDYES IDYES access_all
      nsExec::ExecToLog '"$INSTDIR\submarine-daemon.exe" access only "$4"'
      Goto access_done
    access_all:
      nsExec::ExecToLog '"$INSTDIR\submarine-daemon.exe" access all'
  access_done:

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
