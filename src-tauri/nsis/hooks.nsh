; Uncheck "Create Desktop Shortcut" by default on the finish page
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED

!macro NSIS_HOOK_POSTUNINSTALL
  RMDir /r "$APPDATA\PortusEchoes"
!macroend
