; Extra steps for the MediaForge installer (see "installerHooks" in tauri.conf.json).
;
; MediaForge offers itself in Explorer's "Open with" menu for audio and video files, as the Mac version does
; in Finder. It registers as an application that supports those types, and never as the default app for any of
; them, so nothing the user has set up changes.

!macro MEDIAFORGE_SUPPORTS EXT
  WriteRegStr SHCTX "Software\Classes\Applications\MediaForge.exe\SupportedTypes" "${EXT}" ""
!macroend

!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr SHCTX "Software\Classes\Applications\MediaForge.exe" "FriendlyAppName" "MediaForge"
  WriteRegStr SHCTX "Software\Classes\Applications\MediaForge.exe\shell\open" "FriendlyAppName" "MediaForge"
  WriteRegStr SHCTX "Software\Classes\Applications\MediaForge.exe\shell\open\command" "" '"$INSTDIR\MediaForge.exe" "%1"'
  !insertmacro MEDIAFORGE_SUPPORTS ".mp3"
  !insertmacro MEDIAFORGE_SUPPORTS ".wav"
  !insertmacro MEDIAFORGE_SUPPORTS ".flac"
  !insertmacro MEDIAFORGE_SUPPORTS ".m4a"
  !insertmacro MEDIAFORGE_SUPPORTS ".aac"
  !insertmacro MEDIAFORGE_SUPPORTS ".aiff"
  !insertmacro MEDIAFORGE_SUPPORTS ".aif"
  !insertmacro MEDIAFORGE_SUPPORTS ".ogg"
  !insertmacro MEDIAFORGE_SUPPORTS ".opus"
  !insertmacro MEDIAFORGE_SUPPORTS ".wma"
  !insertmacro MEDIAFORGE_SUPPORTS ".mp4"
  !insertmacro MEDIAFORGE_SUPPORTS ".m4v"
  !insertmacro MEDIAFORGE_SUPPORTS ".mov"
  !insertmacro MEDIAFORGE_SUPPORTS ".mkv"
  !insertmacro MEDIAFORGE_SUPPORTS ".webm"
  !insertmacro MEDIAFORGE_SUPPORTS ".avi"
  !insertmacro MEDIAFORGE_SUPPORTS ".wmv"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegKey SHCTX "Software\Classes\Applications\MediaForge.exe"
!macroend
