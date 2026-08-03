---
name: pet
description: Show, hide, or switch the Pipsqueak desktop pet
argument-hint: "[on | off | toggle | status | byte | pip | ember]"
disable-model-invocation: true
---

The command below runs while this skill expands, so the pet reacts immediately
and no terminal window opens.

!`p="$LOCALAPPDATA/Pipsqueak/pipsqueak.exe"; [ -x "$p" ] || p="$HOME/.local/bin/pipsqueak"; [ -x "$p" ] || p="$(command -v pipsqueak)"; if [ -x "$p" ]; then "$p" control "$ARGUMENTS"; else echo "Pipsqueak is not installed. Get it from https://github.com/tristanmuzzu/pipsqueak/releases"; fi`

Report the line above to the user verbatim and stop. Do not run any other
command, and do not explain what the pet is unless asked.
