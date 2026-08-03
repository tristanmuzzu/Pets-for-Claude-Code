; Uninstalling has to take the hooks with it. Upgrading must not.
;
; The hooks live in ~/.claude/settings.json and name this executable by
; absolute path. Deleting the program without removing them leaves fifteen
; entries pointing at a file that no longer exists, in a config file the user
; hand-tunes, so every Claude Code session afterwards is quietly trying to run
; a deleted program.
;
; An upgrade runs the old uninstaller first, and there the hooks are correct:
; they name a path the new build is about to occupy. Stripping them would
; silently deafen the pet on every update. Tauri passes /UPDATE in that case,
; which is the only thing separating the two.
;
; `uninstall` is marker-scoped: it only removes entries whose command contains
; "pipsqueak", backs the file up first, and refuses to touch a settings.json it
; cannot parse. Failure here must never block the uninstall, so the result is
; ignored on purpose.

!include FileFunc.nsh
!include LogicLib.nsh

!macro NSIS_HOOK_PREUNINSTALL
  ClearErrors
  ${GetOptions} $CMDLINE "/UPDATE" $R9
  ${If} ${Errors}
    ClearErrors
    IfFileExists "$INSTDIR\pipsqueak.exe" 0 +2
      ExecWait '"$INSTDIR\pipsqueak.exe" uninstall' $0
  ${EndIf}
!macroend
