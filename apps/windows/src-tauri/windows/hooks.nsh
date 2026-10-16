; Since 0.1.178 qc-device-broker.exe is one process per user session that is
; the device broker, its notification icon and the background MCP host. As the
; resident host it outlives the GUI and may hold no client at all, so it never
; exits by itself during an upgrade. Tauri checks only its main executable
; before copying or deleting sidecars, so this installation's processes are
; asked to stop first. Settings and registry state are preserved, and every
; wait is bounded.
;
; Which helper runs depends on what is installed, because a broker from before
; 0.1.178 treats an unknown argument as its default live probe and would open
; the USB device:
; - 0.1.176/0.1.177 (and every release since the tray existed) ship
;   qc-remote-tray.exe. Its own --stop-for-install stops that old background
;   host and then signals the broker at the same path. The event name has not
;   changed, so this also stops a 0.1.178+ broker left beside an old tray by an
;   interrupted upgrade.
; - 0.1.178+ write qc-device-broker.resident-host in POSTINSTALL, after every
;   payload file, holding the broker's file time. It is trusted only while the
;   broker still carries that exact time, so neither an interrupted upgrade
;   (marker absent) nor a later downgrade (broker replaced) can make this run
;   an old broker. The broker stops its MCP host first and steps aside only
;   with no other client attached; the helper always exits 0 and
;   QC_CHECK_BROKER_UNLOCKED below is the verdict.
; - Anything else is a broker from before the tray, which never holds the
;   device past its five-second grace: nothing to signal.
!macro QC_STOP_INSTALLED_HOST_AT DIRECTORY
  ${If} ${FileExists} "${DIRECTORY}\qc-remote-tray.exe"
    ClearErrors
    ExecWait '"${DIRECTORY}\qc-remote-tray.exe" --stop-for-install' $R0
    ${If} ${Errors}
      SetErrorLevel 1
      Abort "Could not request QC Remote background host shutdown. Close QC Remote and retry."
    ${EndIf}
    ${If} $R0 != 0
      SetErrorLevel 1
      Abort "QC Remote background host did not stop. Close QC Remote and retry."
    ${EndIf}
    ; The helper has exited; a remaining running image must fail installation
    ; rather than allowing an old host to survive a partial replacement.
    ClearErrors
    FileOpen $R1 "${DIRECTORY}\qc-remote-tray.exe" a
    ${If} ${Errors}
      SetErrorLevel 1
      Abort "QC Remote background host is still in use. Close QC Remote and retry."
    ${EndIf}
    FileClose $R1
  ${ElseIf} ${FileExists} "${DIRECTORY}\qc-device-broker.resident-host"
    ClearErrors
    GetFileTime "${DIRECTORY}\qc-device-broker.exe" $R6 $R7
    FileOpen $R1 "${DIRECTORY}\qc-device-broker.resident-host" r
    FileRead $R1 $R8
    FileClose $R1
    ${IfNot} ${Errors}
    ${AndIf} $R8 == "$R6 $R7"
      ClearErrors
      ExecWait '"${DIRECTORY}\qc-device-broker.exe" --stop-for-install' $R0
      ${If} ${Errors}
        SetErrorLevel 1
        Abort "Could not request QC Remote broker shutdown. Close QC Remote and retry."
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend

!macro QC_STOP_INSTALLED_HOST
  !insertmacro QC_STOP_INSTALLED_HOST_AT "$INSTDIR"
!macroend

; A broker process can outlive its GUI while clients still own the shared
; session. Refuse an upgrade until the current installation's broker image can
; be opened for append. Retry briefly for a broker that is already exiting;
; never kill it. It runs after QC_STOP_INSTALLED_HOST, never before: the
; resident host holds its image for as long as it runs, so a check before the
; stop would refuse every upgrade.
!macro QC_CHECK_BROKER_UNLOCKED DIRECTORY
  ${If} ${FileExists} "${DIRECTORY}\qc-device-broker.exe"
    StrCpy $R4 0
    ${Do}
      ClearErrors
      FileOpen $R1 "${DIRECTORY}\qc-device-broker.exe" a
      ${IfNot} ${Errors}
        FileClose $R1
        ${ExitDo}
      ${EndIf}
      ${If} $R4 >= 14
        SetErrorLevel 1
        Abort "QC Remote broker is still in use. Close QC Remote and the AI apps using it, or release the Quad Cortex from the QC Remote icon in the notification area, then retry."
      ${EndIf}
      Sleep 500
      IntOp $R4 $R4 + 1
    ${Loop}
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro QC_STOP_INSTALLED_HOST
  !insertmacro QC_CHECK_BROKER_UNLOCKED "$INSTDIR"
!macroend

; Remove a Run value only when it starts this installation's own executable.
!macro QC_REMOVE_OWNED_RUN_VALUE NAME EXECUTABLE
  ReadRegStr $R0 HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${NAME}"
  StrCpy $R1 '$\"$INSTDIR\${EXECUTABLE}$\"'
  StrLen $R2 $R1
  StrCpy $R3 $R0 $R2
  ${If} $R3 == $R1
    ClearErrors
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${NAME}"
    ${If} ${Errors}
      SetErrorLevel 1
      Abort "Could not remove this installation's QC Remote startup entry."
    ${EndIf}
  ${EndIf}
!macroend

; Written last by POSTINSTALL: see QC_STOP_INSTALLED_HOST_AT. A failure only
; costs the next upgrade its stop request (it then asks the user to release the
; device from the icon), so it never fails the installation.
!macro QC_WRITE_RESIDENT_HOST_MARKER
  ClearErrors
  GetFileTime "$INSTDIR\qc-device-broker.exe" $R6 $R7
  ${IfNot} ${Errors}
    FileOpen $R1 "$INSTDIR\qc-device-broker.resident-host" w
    ${IfNot} ${Errors}
      FileWrite $R1 "$R6 $R7"
      FileClose $R1
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Files this hook set wrote that Tauri's own uninstall list does not know:
  ; the marker, and a 0.1.176/0.1.177 tray an interrupted upgrade left behind.
  Delete "$INSTDIR\qc-device-broker.resident-host"
  Delete "$INSTDIR\qc-remote-tray.exe"
  ; Tauri sets UpdateMode for /UPDATE. Preserve the user's startup choice during
  ; upgrades; a full uninstall removes only commands owned by this directory:
  ; the resident host's value, and 0.1.176/0.1.177's tray value if one is left.
  ${If} $UpdateMode <> 1
    !insertmacro QC_REMOVE_OWNED_RUN_VALUE "QC Remote" "qc-device-broker.exe"
    !insertmacro QC_REMOVE_OWNED_RUN_VALUE "QC Remote Tray" "qc-remote-tray.exe"
  ${EndIf}
!macroend

; 0.1.176/0.1.177 started "$INSTDIR\qc-remote-tray.exe" --directory "<dir>" at
; sign-in under "QC Remote Tray". Carry that choice over to the resident host:
; write "QC Remote" with the same --directory, then delete the old value. Both
; steps are idempotent and the new value is written first, so an interrupted
; upgrade leaves at worst both values, and the next installer run (or the app's
; next start, which applies the same rule) finishes the move. Another
; installation's value is left to that installation's own uninstaller.
!macro QC_MIGRATE_LEGACY_RUN_VALUE
  ReadRegStr $R0 HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "QC Remote Tray"
  StrCpy $R1 '$\"$INSTDIR\qc-remote-tray.exe$\"'
  StrLen $R2 $R1
  StrCpy $R3 $R0 $R2
  ${If} $R3 == $R1
    StrCpy $R5 $R0 "" $R2
    ClearErrors
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "QC Remote" '$\"$INSTDIR\qc-device-broker.exe$\" --resident$R5'
    ${If} ${Errors}
      SetErrorLevel 1
      Abort "Could not move the QC Remote startup entry to the background host."
    ${EndIf}
    ClearErrors
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "QC Remote Tray"
    ${If} ${Errors}
      SetErrorLevel 1
      Abort "Could not remove the old QC Remote tray startup entry."
    ${EndIf}
  ${EndIf}
!macroend

; Delete only named product executables, and fail rather than leave a second
; launchable installation behind. Never use a recursive directory removal.
!macro QC_DELETE_LEGACY_EXECUTABLE DIRECTORY FILENAME
  ${If} ${FileExists} "${DIRECTORY}\${FILENAME}"
    Delete "${DIRECTORY}\${FILENAME}"
    ${If} ${FileExists} "${DIRECTORY}\${FILENAME}"
      SetErrorLevel 1
      Abort "An old QC Remote executable is still in use: ${DIRECTORY}\${FILENAME}. Close the old copy and retry."
    ${EndIf}
  ${EndIf}
!macroend

!macro QC_DELETE_LEGACY_EXECUTABLES DIRECTORY
  !insertmacro QC_DELETE_LEGACY_EXECUTABLE "${DIRECTORY}" "qc-remote-tray.exe"
  !insertmacro QC_DELETE_LEGACY_EXECUTABLE "${DIRECTORY}" "qc-device-broker.exe"
  !insertmacro QC_DELETE_LEGACY_EXECUTABLE "${DIRECTORY}" "qc-remote.exe"
  !insertmacro QC_DELETE_LEGACY_EXECUTABLE "${DIRECTORY}" "qc-device-gateway.exe"
!macroend

; The old product name used a separate uninstall key, so Windows does not
; consider it an upgrade of QC Remote. Retire it before copying the new app:
; its uninstaller must never run after the new background host could have
; launched.
; Never remove app-data folders: they can hold settings and integrations.
!macro QC_RETIRE_LEGACY_INSTALLATION
  ${If} "$INSTDIR" != "$LOCALAPPDATA\QC Control"
    !insertmacro QC_STOP_INSTALLED_HOST_AT "$LOCALAPPDATA\QC Control"
    ${If} ${FileExists} "$LOCALAPPDATA\QC Control\uninstall.exe"
      ClearErrors
      ExecWait '"$LOCALAPPDATA\QC Control\uninstall.exe" /S _?=$LOCALAPPDATA\QC Control' $R0
      ${If} ${Errors}
        SetErrorLevel 1
        Abort "Could not retire the old QC Control installation. Uninstall QC Control and retry."
      ${EndIf}
      ${If} $R0 != 0
        SetErrorLevel 1
        Abort "QC Control could not be uninstalled. Close it and retry."
      ${EndIf}
    ${EndIf}
    !insertmacro QC_DELETE_LEGACY_EXECUTABLES "$LOCALAPPDATA\QC Control"
  ${EndIf}
  ${If} "$INSTDIR" != "$LOCALAPPDATA\QC Remote"
    !insertmacro QC_STOP_INSTALLED_HOST_AT "$LOCALAPPDATA\QC Remote"
    ; A legacy mixed installation's uninstaller must not survive relocation:
    ; it could later remove the new installation's shared registry entries.
    ; Only retire it when this is demonstrably an app installation, not data.
    ${If} ${FileExists} "$LOCALAPPDATA\QC Remote\qc-remote.exe"
      !insertmacro QC_DELETE_LEGACY_EXECUTABLES "$LOCALAPPDATA\QC Remote"
      !insertmacro QC_DELETE_LEGACY_EXECUTABLE "$LOCALAPPDATA\QC Remote" "uninstall.exe"
      Delete "$LOCALAPPDATA\QC Remote\qc-device-broker.resident-host"
    ${Else}
      !insertmacro QC_DELETE_LEGACY_EXECUTABLES "$LOCALAPPDATA\QC Remote"
    ${EndIf}
  ${EndIf}
!macroend

; A per-user installation directory must not be writable by anyone else. The
; default under %LOCALAPPDATA% already inherits that, but a directory the user
; picked (C:\QC Remote, a shared drive) inherits whatever its parent grants,
; and C:\ lets every authenticated user create and modify subfolders. Anyone
; who can write here can replace qc-remote.exe, the broker, or a DLL they load,
; and run as this user on the next launch. So before any payload is copied the
; directory gets a protected DACL: the installing user (also made its owner),
; SYSTEM and Administrators, full control, nothing inherited from the parent.
; Existing children are reset to inherit only that. Fails closed: an
; installation that cannot be protected is refused, not completed open.
;
; That rewrite is irreversible, so it is applied only to a folder that is this
; product's to rewrite: one that does not exist yet, one that is empty, or one
; that already holds qc-remote.exe (an earlier installation being upgraded or
; repaired). Any other folder (D:\Apps, a tools folder, a shared drive) would
; have every file beneath it re-owned and stripped of its own permissions, so
; it is refused before anything is created or changed.
!macro QC_ENUMERATE_INSTALL_DIRECTORY ERROR_MESSAGE
  ; NSIS FindNext sets only its own error flag. Use the native API through the
  ; System plug-in so ?e captures GetLastError immediately after each call.
  ; WIN32_FIND_DATAW is 592 bytes; cFileName starts at byte 44.
  StrCpy $R8 0
  System::Alloc 592
  Pop $R0
  ${If} $R0 == 0
    SetErrorLevel 1
    Abort "${ERROR_MESSAGE}"
  ${EndIf}
  System::Call 'kernel32::FindFirstFileW(w "$INSTDIR\*", p R0) p.R6 ?e'
  Pop $R7
  ${If} $R6 == -1
    System::Free $R0
    SetErrorLevel 1
    Abort "${ERROR_MESSAGE}"
  ${EndIf}
  System::Call '*$R0(&v44, &w260 .r9)'
  ${If} $9 != "."
  ${AndIf} $9 != ".."
    StrCpy $R8 1
  ${EndIf}
  StrCpy $R1 1
  StrCpy $R2 18
  ${DoWhile} $R8 == 0
    System::Call 'kernel32::FindNextFileW(p R6, p R0) i.R1 ?e'
    Pop $R2
    ${If} $R1 == 0
      ${ExitDo}
    ${EndIf}
    System::Call '*$R0(&v44, &w260 .r9)'
    ${If} $9 != "."
    ${AndIf} $9 != ".."
      StrCpy $R8 1
    ${EndIf}
  ${Loop}
  FindClose $R6
  System::Free $R0
  ${If} $R1 == 0
  ${AndIf} $R2 != 18
    SetErrorLevel 1
    Abort "${ERROR_MESSAGE}"
  ${EndIf}
!macroend

; Refuse root/descendant reparse points before following payload paths or
; rewriting ACLs. Ancestor redirects are allowed (redirected profiles/mounts).
; This preflight does not defend against a hostile concurrent path replacement.
!macro QC_VERIFY_INSTALL_TREE ALLOW_MISSING
  System::Call 'kernel32::GetFileAttributesW(w "$INSTDIR") i.R4 ?e'
  Pop $R7
  ${If} $R4 == -1
    StrCpy $R9 -1
    ${If} $R7 == 2
    ${OrIf} $R7 == 3
      ${If} ${ALLOW_MISSING} == 1
        StrCpy $R9 0
      ${EndIf}
    ${EndIf}
    ${If} $R9 == 0
      StrCpy $R9 0
    ${Else}
      SetErrorLevel 1
      Abort "Could not inspect the QC Remote installation folder. Choose an ordinary folder and retry."
    ${EndIf}
  ${Else}
    Push "$INSTDIR"
    StrCpy $R9 1
  ${EndIf}
  ${DoWhile} $R9 > 0
    Pop $R3
    IntOp $R9 $R9 - 1
    System::Call 'kernel32::GetFileAttributesW(w "$R3") i.R4 ?e'
    Pop $R7
    IntOp $R5 $R4 & 0x400
    IntOp $R2 $R4 & 0x10
    ${If} $R4 == -1
    ${OrIf} $R5 != 0
    ${OrIf} $R2 == 0
      SetErrorLevel 1
      Abort "QC Remote cannot install through links or reparse points. Choose an ordinary folder and retry."
    ${EndIf}
    System::Alloc 592
    Pop $R0
    ${If} $R0 == 0
      SetErrorLevel 1
      Abort "Could not inspect the QC Remote installation folder. Retry the installation."
    ${EndIf}
    System::Call 'kernel32::FindFirstFileW(w "$R3\*", p R0) p.R6 ?e'
    Pop $R7
    ${If} $R6 == -1
      System::Free $R0
      SetErrorLevel 1
      Abort "Could not inspect the QC Remote installation files. Retry the installation."
    ${EndIf}
    ${Do}
      System::Call '*$R0(i .R4, &v40, &w260 .r9)'
      ${If} $9 != "."
      ${AndIf} $9 != ".."
        IntOp $R5 $R4 & 0x400
        ${If} $R5 != 0
          FindClose $R6
          System::Free $R0
          SetErrorLevel 1
          Abort "QC Remote installation files contain links or reparse points. Move them aside and retry."
        ${EndIf}
        IntOp $R5 $R4 & 0x10
        ${If} $R5 != 0
          Push "$R3\$9"
          IntOp $R9 $R9 + 1
        ${EndIf}
      ${EndIf}
      System::Call 'kernel32::FindNextFileW(p R6, p R0) i.R1 ?e'
      Pop $R2
      ${If} $R1 == 0
        ${ExitDo}
      ${EndIf}
    ${Loop}
    FindClose $R6
    System::Free $R0
    ${If} $R2 != 18
      SetErrorLevel 1
      Abort "Could not inspect all QC Remote installation files. Retry the installation."
    ${EndIf}
  ${Loop}
!macroend

!macro QC_PROTECT_INSTALL_DIRECTORY
  ; Establish that the target exists and is a directory before enumeration.
  ; CreateDirectory succeeds without changing an existing directory.
  ClearErrors
  CreateDirectory "$INSTDIR"
  ${If} ${Errors}
    SetErrorLevel 1
    Abort "Could not inspect $INSTDIR. Choose a folder that can be accessed and retry."
  ${EndIf}
  !insertmacro QC_VERIFY_INSTALL_TREE 0
  !insertmacro QC_ENUMERATE_INSTALL_DIRECTORY "Could not inspect $INSTDIR. Choose a folder that can be accessed and retry."
  ${If} $R8 == 1
  ${AndIfNot} ${FileExists} "$INSTDIR\qc-remote.exe"
    SetErrorLevel 1
    Abort "$INSTDIR already contains other files. Choose an empty folder for QC Remote and retry."
  ${EndIf}
  ; The installing user's SID, from the token rather than the environment.
  nsExec::ExecToStack '"$SYSDIR\whoami.exe" /user /fo csv /nh'
  Pop $R0
  Pop $R1
  ${If} $R0 != 0
    SetErrorLevel 1
    Abort "Could not identify the installing user to protect $INSTDIR."
  ${EndIf}
  ; whoami prints "domain\user","S-1-5-..." - take what follows the quote
  ; that opens the SID, up to the quote that closes it.
  StrCpy $R5 ""
  StrLen $R3 $R1
  StrCpy $R2 0
  ${Do}
    ${If} $R2 >= $R3
      ${ExitDo}
    ${EndIf}
    StrCpy $R4 $R1 5 $R2
    ${If} $R4 S== '"S-1-'
      IntOp $R2 $R2 + 1
      StrCpy $R5 $R1 "" $R2
      ${ExitDo}
    ${EndIf}
    IntOp $R2 $R2 + 1
  ${Loop}
  StrLen $R3 $R5
  StrCpy $R2 0
  ${Do}
    ${If} $R2 >= $R3
      ${ExitDo}
    ${EndIf}
    StrCpy $R4 $R5 1 $R2
    ${If} $R4 == '"'
      StrCpy $R5 $R5 $R2
      ${ExitDo}
    ${EndIf}
    IntOp $R2 $R2 + 1
  ${Loop}
  ; Any well-formed SID names the user: "S-1-", an identifier authority, then
  ; one to fifteen sub-authorities, all decimal and dash-separated. That is
  ; S-1-5-21-... for a local or Active Directory account and S-1-12-1-... for
  ; a Microsoft Entra ID account. Requiring S-1-5- refused every Entra ID
  ; installation, upgrades included. Anything else - an empty component, a
  ; stray character, a missing closing quote - is refused: the value goes
  ; into the icacls command lines below.
  StrCpy $R4 $R5 4
  ${If} $R4 S!= "S-1-"
    SetErrorLevel 1
    Abort "Could not identify the installing user to protect $INSTDIR."
  ${EndIf}
  ; $R6 counts the digits of the current component, $R7 the components ended
  ; so far, and $R7 = -1 marks a malformed SID.
  StrLen $R3 $R5
  StrCpy $R2 4
  StrCpy $R6 0
  StrCpy $R7 0
  ${Do}
    ${If} $R2 >= $R3
      ${ExitDo}
    ${EndIf}
    StrCpy $R4 $R5 1 $R2
    ${If} $R4 == "-"
      ${If} $R6 == 0
        StrCpy $R7 -1
        ${ExitDo}
      ${EndIf}
      IntOp $R7 $R7 + 1
      StrCpy $R6 0
    ${Else}
      ; A single character survives IntOp unchanged only if it is a digit.
      IntOp $R1 $R4 + 0
      ${If} $R1 S!= $R4
        StrCpy $R7 -1
        ${ExitDo}
      ${EndIf}
      IntOp $R6 $R6 + 1
    ${EndIf}
    IntOp $R2 $R2 + 1
  ${Loop}
  ${If} $R7 >= 0
  ${AndIf} $R6 > 0
    IntOp $R7 $R7 + 1
  ${Else}
    StrCpy $R7 -1
  ${EndIf}
  ${If} $R7 < 2
  ${OrIf} $R7 > 16
    SetErrorLevel 1
    Abort "Could not identify the installing user to protect $INSTDIR."
  ${EndIf}
  ; Drop every explicit entry first: /grant:r below replaces only the three
  ; it names, and a folder made by someone else can carry its own grants.
  nsExec::ExecToStack '"$SYSDIR\icacls.exe" "$INSTDIR" /reset /Q'
  Pop $R0
  Pop $R1
  ${If} $R0 != 0
    SetErrorLevel 1
    Abort "Could not restrict access to $INSTDIR. Choose a folder on an NTFS drive that you own and retry."
  ${EndIf}
  nsExec::ExecToStack '"$SYSDIR\icacls.exe" "$INSTDIR" /inheritance:r /grant:r "*$R5:(OI)(CI)F" "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-544:(OI)(CI)F" /Q'
  Pop $R0
  Pop $R1
  ${If} $R0 != 0
    SetErrorLevel 1
    Abort "Could not restrict access to $INSTDIR. Choose a folder on an NTFS drive that you own and retry."
  ${EndIf}
  nsExec::ExecToStack '"$SYSDIR\icacls.exe" "$INSTDIR" /setowner "*$R5" /Q'
  Pop $R0
  Pop $R1
  ${If} $R0 != 0
    SetErrorLevel 1
    Abort "Could not take ownership of $INSTDIR. Choose a folder that you own and retry."
  ${EndIf}
  ; Earlier files keep no explicit grants of their own: each inherits only the
  ; protected entries above. A fresh directory has no children to reset.
  !insertmacro QC_ENUMERATE_INSTALL_DIRECTORY "Could not inspect the files in $INSTDIR. Close QC Remote and retry."
  ${If} $R8 == 1
    nsExec::ExecToStack '"$SYSDIR\icacls.exe" "$INSTDIR\*" /reset /T /C /Q'
    Pop $R0
    Pop $R1
    ${If} $R0 != 0
      SetErrorLevel 1
      Abort "Could not restrict access to the files in $INSTDIR. Close QC Remote and retry."
    ${EndIf}
  ${EndIf}
!macroend

!macro QC_REJECT_DATA_DIRECTORY
  ; Runtime data is never an installation target, even with an explicit /D.
  GetFullPathName $R7 "$INSTDIR"
  StrLen $R8 "$LOCALAPPDATA\QC Remote\"
  StrCpy $R9 "$R7\" $R8
  ${If} $R9 == "$LOCALAPPDATA\QC Remote\"
    SetErrorLevel 1
    Abort "This folder stores QC Remote user data. Install in $LOCALAPPDATA\Programs\QC Remote instead. Your data has not been changed."
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro QC_REJECT_DATA_DIRECTORY
  ; Never execute/delete through a linked selected root before the ACL guard.
  !insertmacro QC_VERIFY_INSTALL_TREE 1
  ; Stop first, then judge: the resident host steps aside here only when no
  ; client is attached, and the unlock check catches one that is still busy
  ; or that started in the interval between the two.
  !insertmacro QC_STOP_INSTALLED_HOST
  !insertmacro QC_CHECK_BROKER_UNLOCKED "$INSTDIR"
  !insertmacro QC_RETIRE_LEGACY_INSTALLATION
  ; Last, so nothing above can loosen it, and before any payload is copied.
  !insertmacro QC_PROTECT_INSTALL_DIRECTORY
!macroend

; Remove the Python gateway left by QC Remote releases before 0.1.67, and the
; separate background host 0.1.176/0.1.177 shipped as qc-remote-tray.exe: its
; work now runs inside qc-device-broker.exe. It was stopped in PREINSTALL, so a
; file still in use here is an old copy someone restarted; the delete then
; fails loudly rather than leaving a second launchable host. The Run value is
; moved after the payload, so it never names a broker that is not there yet.
!macro NSIS_HOOK_POSTINSTALL
  !insertmacro QC_DELETE_LEGACY_EXECUTABLE "$INSTDIR" "qc-device-gateway.exe"
  !insertmacro QC_DELETE_LEGACY_EXECUTABLE "$INSTDIR" "qc-remote-tray.exe"
  !insertmacro QC_MIGRATE_LEGACY_RUN_VALUE
  !insertmacro QC_WRITE_RESIDENT_HOST_MARKER
!macroend
