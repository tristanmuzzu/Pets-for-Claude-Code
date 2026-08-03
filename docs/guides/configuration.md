# Configuration and CLI

[← README](../../README.md)

Everything lives in `~/.pipsqueak/config.json` and is editable from the pet's
right-click menu. Editing the file by hand is fine. An out-of-range value is
pulled back to something usable and costs only itself, and a file that cannot be
parsed at all is moved to `config.json.bak` rather than overwritten.

| Key | Meaning |
| --- | --- |
| `version` | which build's idea of this file it is; managed for you |
| `pet` | id of the active pet (`byte`, `pip`, `ember`, or your own) |
| `scale` | pet size, 1 to 6 |
| `click_through` | make the pet itself non-interactive too |
| `show_bubble` | hide the status cards, keep the pet |
| `show_scratch` | include sessions rooted in a temp directory |
| `alert_on_waiting` | play a sound when a project starts waiting on you |
| `flash_on_finish` | blink the tray icon when a project finishes |
| `quiet` | do not disturb: no sound, no blink, the pet dozes |
| `hotkey` | chord that shows and hides the pet, or `off` |
| `update_check` | ask GitHub about new releases (off by default) |
| `update_dismissed` | a version you have already been told about |
| `welcomed` | whether the first-run panel has been dismissed |
| `x` / `y` | window position, saved when you drag the pet |

A config written by a newer version of Pipsqueak is read but never written back
to, so running an older build cannot quietly delete settings it does not know
about.

## Keyboard shortcut

`Ctrl+Alt+P` shows and hides the pet from anywhere, so you never have to go
looking for the tray icon.

Windows gives a chord to whichever program asks for it first, so if something
else already owns `Ctrl+Alt+P`, Pipsqueak takes the next one it can get and the
right-click menu shows which. Set `hotkey` in `config.json` to pick your own, or
to `off` to register nothing at all. Any combination of `Ctrl`, `Alt`, `Shift`
and `Win` plus a letter, digit or function key works. A chord with no modifier
is refused, since it would take that key away from everything else on the
machine.

## CLI

```bash
pipsqueak                 # run the overlay
pipsqueak control on      # show it, starting it if needed (also: off, toggle, quit, status)
pipsqueak control byte    # switch pet
pipsqueak install         # register the Claude Code hooks
pipsqueak uninstall       # remove them (backs up settings.json first)
pipsqueak hook <Event>    # internal: consume one hook payload from stdin
```

On Windows the binary is a GUI-subsystem app, so CLI output is also written to
`~/.pipsqueak/last-cli-result.txt`.

## From inside Claude Code

There is a small plugin so you never have to open a terminal:

```bash
/plugin marketplace add tristanmuzzu/pipsqueak
```

```bash
/plugin install pipsqueak@pipsqueak
```

| Command | Effect |
| --- | --- |
| `/pipsqueak:pet` | toggle the pet |
| `/pipsqueak:pet on` | show it, starting Pipsqueak if it isn't running |
| `/pipsqueak:pet off` | hide it |
| `/pipsqueak:pet byte` | switch pet |
| `/pipsqueak:pet status` | is it running? |
| `/pipsqueak:pet quit` | close it entirely |

The command runs while the skill expands, so it takes effect immediately
without spending a turn and without a console window appearing. Under the hood
it is `pipsqueak control <action>`, which writes a one-line instruction to
`~/.pipsqueak/command.json` for the running overlay to pick up, and starts the
overlay first if it isn't up.

## Uninstalling

1. Right-click the pet → **Quit**.
2. Uninstall the app. The uninstaller removes the hooks for you; `pipsqueak
   uninstall` does the same thing by hand.
3. Delete `~/.pipsqueak` if you want the config gone too.
