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
| `show_bubble` | show the status cards; off keeps the pet alone, and is remembered |
| `show_scratch` | include sessions rooted in a temp directory |
| `alert_on_waiting` | play a sound when a project starts waiting on you |
| `flash_on_finish` | blink the tray icon when a project finishes |
| `quiet` | do not disturb: no sound, no blink, the pet dozes |
| `hotkey` | chord that shows and hides the pet, or `off` |
| `update_check` | ask GitHub about new releases (off by default) |
| `update_dismissed` | a version you have already been told about |
| `welcomed` | whether the first-run panel has been dismissed |
| `autostart_initialised` | whether the start-with-Windows decision has been made; managed for you |
| `x` / `y` | window position, saved when you drag the pet |

A config written by a newer version of Pipsqueak is read but never written back
to, so running an older build cannot quietly delete settings it does not know
about.

## Keyboard shortcut

`Ctrl+Alt+P` shows and hides the pet from anywhere, so you never have to go
looking for the tray icon.

If pressing it appears to do nothing, open **Check my setup**. The keyboard
shortcut row says which chord registered and how many times it has actually
been pressed. Registering a chord and receiving one are separate things: a
program with a low-level keyboard hook can swallow the keypress on its way
past, and from inside Pipsqueak that looks exactly like nobody pressing it.

The chord belongs to the running app, which makes **Hide** and **Quit** two
different things. Hiding puts the pet away and leaves the process alive, so the
chord and the tray icon still work. Quitting stops it, and nothing on the
keyboard can bring back a program that is not running: start it from the Start
menu again, or run `pipsqueak control on`, which starts it if it has to.

Windows gives a chord to whichever program asks for it first, so if something
else already owns `Ctrl+Alt+P`, Pipsqueak takes the next one it can get and the
right-click menu shows which. Set `hotkey` in `config.json` to pick your own, or
to `off` to register nothing at all. Any combination of `Ctrl`, `Alt`, `Shift`
and `Win` plus a letter, digit or function key works. A chord with no modifier
is refused, since it would take that key away from everything else on the
machine.

## When it stops behaving

`~/.pipsqueak/log.txt` records what the overlay did: when it started and from
which path, which hotkey it registered, when the page had to be reloaded, and
when a session was retired. It is capped at a quarter of a megabyte and keeps
the newest half.

The failure worth knowing about is the quiet one. The window can be present,
visible, correctly positioned and painting nothing, which looks exactly like
the pet having vanished. Hiding and showing it does not help, because an empty
page is empty either way. The overlay now notices this itself: the frontend
reports in on every render, and 45 seconds of silence triggers a reload, logged.

The reload is a ping first: an occluded window has its timers throttled to
about one a minute by Windows, which is silence without being death, and
reloading a healthy page is itself a visible blank and repaint. **Reload the
overlay** in the tray menu forces one on demand.

## After a reboot

Pipsqueak registers itself to start with Windows on its first run, because an
overlay that is not running is indistinguishable from one that is broken —
and the shortcut that would bring it back belongs to the process that is not
there. **Check my setup** has a row for it, and:

```bash
pipsqueak autostart status   # also: on, off
```

Turning it off is remembered, so it stays off. If the program is installed to
a new location, the entry is corrected the next time it runs, since one
pointing at the old path is present, plausible, and starts nothing.

If the pet is running but Claude Code events never arrive, the program may have
been removed from disk while it was still running. Windows keeps a deleted
program running, so it looks healthy while every hook points at a path that no
longer exists. The overlay checks for this and says so, and **Check my setup**
reports it under "Hook program".

## CLI

```bash
pipsqueak                 # run the overlay
pipsqueak control on      # show it, starting it if needed (also: off, toggle, quit, status)
pipsqueak control byte    # switch pet
pipsqueak autostart on    # start with Windows (also: off, status)
pipsqueak sessions        # what the overlay would draw right now, as JSON
pipsqueak install         # register the Claude Code hooks
pipsqueak uninstall       # remove them (backs up settings.json first)
pipsqueak hook <Event>    # internal: consume one hook payload from stdin
```

`sessions` answers "why is the card saying that". A card is a session file
joined to two things that are not in it — the chat the desktop app knows
about, and what the transcript says is still running — so both were previously
only visible by looking at the pet.

If the hook payloads themselves are in question, `mkdir ~/.pipsqueak/payloads`
and every hook writes its raw payload there until the directory is removed.
They contain prompts and assistant text, which is why it is a directory you
have to make rather than a setting you might leave on.

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
