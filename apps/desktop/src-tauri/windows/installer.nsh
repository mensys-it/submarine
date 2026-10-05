; Hooks of the NSIS installer made by Tauri for Windows.
;
; They register submarine-daemon as a Windows service, ask on the first
; installation who may use it, and put the `submarine` CLI on the machine PATH.
; The daemon, the CLI, wintun.dll and the driver are installed side by side in
; $INSTDIR.
;
; NB: the Tauri template already includes StrFunc.nsh and declares ${StrLoc};
; ${UnStrRep} is declared here, since the uninstaller needs it.

!include nsDialogs.nsh

; registry key of the machine environment variables
!define ENV_KEY "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"
${UnStrRep}

; Access page: who may use the service, chosen with two radio buttons right before
; the installation starts. On an upgrade the current choice is preselected, and
; access.json is rewritten ONLY if it changes, so that a list of users set with
; `submarine-daemon access only ...` survives a plain "Next".
;
; Tauri has hooks for the install and uninstall sections, but NOT for the pages, so
; MUI_PAGE_INSTFILES is redefined to declare the access page in front of the
; installation page. The rest of the body is the same as in MUI2
; (Pages/InstallFiles.nsh); the uninstaller pages are left alone.
!macroundef MUI_PAGE_INSTFILES
!macro MUI_PAGE_INSTFILES
  !insertmacro SUBMARINE_ACCESS_PAGE

  !verbose push
  !verbose ${MUI_VERBOSE}
  !insertmacro MUI_PAGE_INIT
  !insertmacro MUI_PAGEDECLARATION_INSTFILES
  !verbose pop
!macroend

; Declaration of the access page and of its functions.
;
; NB: expanded where the template inserts the installation page, after its `Var`
; declarations: $PassiveMode is NOT declared yet when this file is included.
!macro SUBMARINE_ACCESS_PAGE
  ; "all" or "only", empty when the page was skipped
  Var AccessChoice
  ; "all" or "only" as found in access.json, empty without a readable file
  Var AccessCurrent
  ; user of this desktop session as DOMAIN\user, empty when unknown
  Var AccessUser
  ; whether $AccessUser and $AccessCurrent were already looked up
  Var AccessProbed
  Var AccessRadioAll
  Var AccessRadioOnly

  Page custom AccessPageShow AccessPageLeave

  Function AccessPageShow
    ; skipped in passive mode, which keeps the current choice
    ${IfThen} $PassiveMode = 1 ${|} Abort ${|}

    ; lookups done once, even if the page is shown again
    ${If} $AccessProbed != 1
      StrCpy $AccessProbed 1

      ; the current choice: every user when the file stores `"allowed_users": null`
      ; NB: the data directory is open to the Administrators, so the elevated
      ; installer can read it
      ExpandEnvStrings $0 "%ProgramData%\Submarine\access.json"
      ClearErrors
      FileOpen $0 "$0" r
      ${IfNot} ${Errors}
        StrCpy $AccessCurrent "only"
        ${Do}
          FileRead $0 $1
          ${IfThen} ${Errors} ${|} ${ExitDo} ${|}
          ${StrLoc} $2 "$1" '"allowed_users": null' ">"
          ${If} $2 != ""
            StrCpy $AccessCurrent "all"
          ${EndIf}
        ${Loop}
        FileClose $0
      ${EndIf}
      StrCpy $AccessChoice $AccessCurrent

      ; the user of this desktop session
      ; NB: NOT the account the installer runs as, which is another administrator
      ; when the UAC prompt asked for different credentials
      nsExec::ExecToStack `powershell -NoProfile -NonInteractive -Command "$$s = (Get-Process -Id $$PID).SessionId; $$p = Get-CimInstance Win32_Process -Filter 'Name=''explorer.exe''' | Where-Object SessionId -eq $$s | Select-Object -First 1; if ($$p) { $$o = Invoke-CimMethod -InputObject $$p -MethodName GetOwner; [Console]::Out.Write($$o.Domain + '\' + $$o.User) }"`
      Pop $0
      Pop $1
      ${If} $0 == "0"
        StrCpy $AccessUser $1
      ${EndIf}
    ${EndIf}
    ; without a known user there is no "only me" to offer
    ${IfThen} $AccessUser == "" ${|} Abort ${|}

    !insertmacro MUI_HEADER_TEXT "Choose Users" "Choose who may use Submarine on this computer."
    nsDialogs::Create 1018
    Pop $0
    ${IfThen} $(^RTL) = 1 ${|} nsDialogs::SetRTL $(^RTL) ${|}

    ${NSD_CreateLabel} 0 0 100% 24u "Submarine is installed for the whole computer. Choose who may connect, disconnect and manage the tunnels:"
    Pop $0
    ${NSD_CreateRadioButton} 10u 34u -10u 12u "&Anyone who uses this computer"
    Pop $AccessRadioAll
    ${NSD_AddStyle} $AccessRadioAll ${WS_GROUP}
    ${NSD_CreateRadioButton} 10u 50u -10u 12u "Only for &me ($AccessUser)"
    Pop $AccessRadioOnly
    ${NSD_CreateLabel} 0 76u 100% 24u "The administrators of this computer may always use Submarine."
    Pop $0

    ; the choice made so far or found in access.json, every user otherwise
    ${If} $AccessChoice == "only"
      ${NSD_Check} $AccessRadioOnly
    ${Else}
      ${NSD_Check} $AccessRadioAll
    ${EndIf}

    nsDialogs::Show
  FunctionEnd

  Function AccessPageLeave
    ${NSD_GetState} $AccessRadioOnly $0
    ${If} $0 = ${BST_CHECKED}
      StrCpy $AccessChoice "only"
    ${Else}
      StrCpy $AccessChoice "all"
    ${EndIf}
  FunctionEnd
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; upgrade: stop and remove the old service, so its files can be replaced
  IfFileExists "$INSTDIR\submarine-daemon.exe" 0 +2
    nsExec::ExecToLog '"$INSTDIR\submarine-daemon.exe" service uninstall'
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; who may use the service, as chosen on the access page; without the page
  ; (passive mode, unknown user) an upgrade keeps access.json and a first
  ; installation allows every user
  StrCmp $AccessChoice "" 0 access_chosen
    ExpandEnvStrings $2 "%ProgramData%\Submarine\access.json"
    IfFileExists "$2" access_done access_all
  access_chosen:
    StrCmp $AccessChoice $AccessCurrent access_done
    StrCmp $AccessChoice "only" 0 access_all
      nsExec::ExecToLog '"$INSTDIR\submarine-daemon.exe" access only "$AccessUser"'
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
