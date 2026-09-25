; Reject a shortcut slot owned by something other than this installation before
; Tauri's stock CreateShortcut calls can replace it. The stock uninstaller already
; checks targets before removal; this hook closes the corresponding install gap.
!macro AURORA_GUARD_SHORTCUT SLOT LABEL
  IfFileExists "${SLOT}" 0 ${LABEL}
  !insertmacro IsShortcutTarget "${SLOT}" "$INSTDIR\${MAINBINARYNAME}.exe"
  Pop $0
  ${If} $0 != 1
    ${If} $OldMainBinaryName != ""
      !insertmacro IsShortcutTarget "${SLOT}" "$INSTDIR\$OldMainBinaryName"
      Pop $0
    ${EndIf}
    ${If} $0 != 1
      Abort "An Aurora Launcher shortcut name is already in use by another application. Move that shortcut before installing."
    ${EndIf}
  ${EndIf}
  ${LABEL}:
!macroend

!macro NSIS_HOOK_PREINSTALL
  ReadRegStr $OldMainBinaryName SHCTX "${UNINSTKEY}" "MainBinaryName"
  !insertmacro MUI_STARTMENU_GETFOLDER Application $AppStartMenuFolder
  !insertmacro AURORA_GUARD_SHORTCUT "$SMPROGRAMS\${PRODUCTNAME}.lnk" aurora_start_root_clear
  !insertmacro AURORA_GUARD_SHORTCUT "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" aurora_start_folder_clear
  !insertmacro AURORA_GUARD_SHORTCUT "$DESKTOP\${PRODUCTNAME}.lnk" aurora_desktop_clear
!macroend
