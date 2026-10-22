; Pirosman Studio NSIS hooks (wired in tauri.windows.conf.json).
; The bundled FFmpeg lives in $INSTDIR\ffmpeg. Clear that folder before files
; are copied, so an upgrade or reinstall never keeps binaries from an older
; FFmpeg build, and clear it again after uninstall.

!macro NSIS_HOOK_PREINSTALL
  ${If} $INSTDIR != ""
    RMDir /r "$INSTDIR\ffmpeg"
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $INSTDIR != ""
    RMDir /r "$INSTDIR\ffmpeg"
  ${EndIf}
!macroend
