; Firewol installer hooks (Tauri NSIS).
;
; Firewol keeps its data in %LOCALAPPDATA%\Firewol and its program (the copy Windows is
; pointed at for folders, Win+E and file windows) in %LOCALAPPDATA%\Programs\Firewol.
; Tauri's per-user default is %LOCALAPPDATA%\Firewol, so the default is moved.

!define MUI_CUSTOMFUNCTION_GUIINIT FirewolGuiInit

; This file is included before PRODUCTNAME is defined, so the function spells the name out.
Function FirewolGuiInit
  ${If} $INSTDIR == "$LOCALAPPDATA\Firewol"
    StrCpy $INSTDIR "$LOCALAPPDATA\Programs\Firewol"
  ${EndIf}
FunctionEnd

; Silent installs skip the GUI, so the install step checks again.
!macro NSIS_HOOK_PREINSTALL
  ${If} $INSTDIR == "$LOCALAPPDATA\${PRODUCTNAME}"
    StrCpy $INSTDIR "$LOCALAPPDATA\Programs\${PRODUCTNAME}"
    SetOutPath $INSTDIR
  ${EndIf}
!macroend

; A real uninstall (not an update) gives folders, Win+E and file windows back to Windows.
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --restore-explorer'
  ${EndIf}
!macroend
