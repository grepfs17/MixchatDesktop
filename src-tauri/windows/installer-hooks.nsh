!macro NSIS_HOOK_PREINSTALL
  ; The Mixchat sidecar can outlive the shell. The Windows updater exits with
  ; std::process::exit, which skips the app's shutdown path, so mixchat-node.exe
  ; keeps its own file locked and the installer fails with "Error opening file
  ; for writing". Stop it before any files are copied.
  nsExec::Exec 'taskkill /F /IM mixchat-node.exe /T'
  Pop $0
!macroend
