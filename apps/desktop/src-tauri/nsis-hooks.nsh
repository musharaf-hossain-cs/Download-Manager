# A damaged self-extracting installer must never bypass its embedded CRC,
# including when it is launched with NSIS's normally supported /NCRC switch.
CRCCheck force

!macro NSIS_HOOK_PREINSTALL
  # Tauri's stock current-user root omits the Programs segment. PU08 has one
  # fixed private install location, so reset both the variable and NSIS output
  # directory before the first application payload is copied.
  StrCpy $INSTDIR "$LOCALAPPDATA\Programs\${PRODUCTNAME}"
  SetOutPath $INSTDIR
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  # Silent uninstall is deliberately conservative. Interactive uninstallers
  # offer the explicit state choice requested by PU08.
  IfSilent dm_keep_state
  MessageBox MB_YESNO|MB_ICONQUESTION \
    "Remove Download Manager settings, job history, partial downloads, and logs? Completed downloads are never removed." \
    IDYES dm_remove_state IDNO dm_keep_state

dm_remove_state:
  # Delete only exact app-owned state. Never recursively remove the
  # DownloadManager root because a user may have saved a completed file there.
  Delete "$LOCALAPPDATA\DownloadManager\preferences.json"
  Delete "$LOCALAPPDATA\DownloadManager\State\downloads.sqlite3"
  Delete "$LOCALAPPDATA\DownloadManager\State\downloads.sqlite3-wal"
  Delete "$LOCALAPPDATA\DownloadManager\State\downloads.sqlite3-shm"
  Delete "$LOCALAPPDATA\DownloadManager\State\downloads.sqlite3.pre-migration.bak"
  RMDir /r "$LOCALAPPDATA\DownloadManager\State\partials"
  RMDir "$LOCALAPPDATA\DownloadManager\State"
  Delete "$LOCALAPPDATA\DownloadManager\Logs\backend.log"
  RMDir "$LOCALAPPDATA\DownloadManager\Logs"
  Delete "$LOCALAPPDATA\DownloadManager\ipc\cli-ipc-v1.json"
  RMDir "$LOCALAPPDATA\DownloadManager\ipc"
  RMDir "$LOCALAPPDATA\DownloadManager"

dm_keep_state:
!macroend
