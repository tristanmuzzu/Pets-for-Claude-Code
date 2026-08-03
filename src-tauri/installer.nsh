; Uninstalling has to take the hooks with it.
;
; The hooks live in ~/.claude/settings.json and name this executable by
; absolute path. Deleting the program without removing them leaves fifteen
; entries pointing at a file that no longer exists, in a config file the user
; hand-tunes — so every Claude Code session afterwards is quietly trying to run
; a deleted program.
;
; `uninstall` is marker-scoped: it only removes entries whose command contains
; "pipsqueak", backs the file up first, and refuses to touch a settings.json it
; cannot parse. Failure here must never block the uninstall, so the result is
; ignored deliberately.

!macro NSIS_HOOK_PREUNINSTALL
  IfFileExists "$INSTDIR\pipsqueak.exe" 0 +2
    ExecWait '"$INSTDIR\pipsqueak.exe" uninstall' $0
!macroend
