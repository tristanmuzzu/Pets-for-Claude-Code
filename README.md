<p align="center">
  <img src="assets/portrait-byte.png" width="120" alt="Byte, a small pixel-art robot with a screen for a face" />
</p>

<h1 align="center">Pipsqueak</h1>

<p align="center">
  <strong>Your coding agent, in the corner of your eye.</strong>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="License: MIT"></a>
  <a href="https://github.com/tristanmuzzu/Pets-for-Claude-Code/releases/latest"><img src="https://img.shields.io/badge/Windows-installer-0a7bbb" alt="Windows installer"></a>
  <a href="https://github.com/tristanmuzzu/Pets-for-Claude-Code/releases/latest"><img src="https://img.shields.io/badge/Linux-deb%20%C2%B7%20AppImage%20%C2%B7%20rpm-e95420" alt="Linux packages"></a>
  <a href="#how-it-actually-works"><img src="https://img.shields.io/badge/Claude%20Code-hooks-d97757" alt="Driven by Claude Code hooks"></a>
  <a href="#privacy"><img src="https://img.shields.io/badge/telemetry-none-2ea44f" alt="No telemetry"></a>
  <a href="#the-pets"><img src="https://img.shields.io/badge/pets-3%20built--in-b26efa" alt="Three pets built in"></a>
</p>

<p align="center">
  <a href="#install">Install</a> &nbsp;•&nbsp;
  <a href="#what-you-actually-see">What you see</a> &nbsp;•&nbsp;
  <a href="#more-than-one-project">Many projects</a> &nbsp;•&nbsp;
  <a href="#the-pets">Pets</a> &nbsp;•&nbsp;
  <a href="#what-it-refuses-to-say">What it refuses to say</a>
</p>

---

Claude Code goes off and works for four minutes. You can sit there watching it
scroll, or you can do something else and miss the moment it needs you back.

Pipsqueak is a small pet that sits on top of everything and tells you what your
agent is doing right now. Not a spinner, and not the name of a tool it just
called. The actual sentence Claude wrote about what it's doing, lifted out of
the session while it happens.

![Three Claude Code projects stacked in the corner of a desktop. One is waiting on a permission prompt and has turned orange, with the pet below it showing an exclamation mark. The other two finish and turn green, each showing the answer it landed on.](assets/demo.png)

<sub>Recorded by `npm run demo`, which paints its own backdrop and drives the
real app with simulated sessions. Reproducible, and nobody's desktop in frame.</sub>

## Why this exists

I kept alt-tabbing to a terminal to find out whether Claude was still working,
had finished ten minutes ago, or had been sitting on a permission prompt the
whole time. That last one is the worst. An agent quietly waiting on you is dead
time you're paying for twice.

A status line doesn't fix it, because it lives in the window you're not looking
at. So the status moved out of the window. A pet on top of everything, one card
per chat, and a line that says what's happening, near enough to your cursor
that you read it without meaning to.

## What you actually see

```
● CLOCKWORK  Timezone test flakiness           3m  ↗  ×
It parses in local time and compares in UTC.
That is the bug. Rewriting the assertion.
Editing · 42 actions · 3m
```

| Line | Where it comes from | How often it changes |
| --- | --- | --- |
| **Project and chat** | the git repository, plus the chat's own title in the Claude Code desktop app | basically never |
| **The live line** | what Claude last said, or the last line of what it was thinking | every 20 seconds or so |
| **Status** | the *category* of work, plus counters | when the category does |

The live line gets the space because it's the one that moves. Hooks on their own
can only tell you "Editing render.js", which is the kind of thing happening and
never the point of it. The point is in the sentence Claude writes just before it
reaches for a tool, and Claude Code appends that to the session transcript as it
goes. So Pipsqueak follows the transcript. Only the bytes since the last poll,
only the newest line, never past a half-written one. Nothing leaves your machine.

Want it quieter? The menu offers silence, only what Claude says to you, or what
it says and thinks. The last one is the default and the one that feels alive.

- **↗ opens that chat.** Not some Claude window. That exact conversation, by id,
  in the desktop app.
- **Click a finished card and it's gone.** A card that says **Done** waits until
  you've seen it, however long that takes, and then one click anywhere on it
  swats it away completely. Not down to a chip, which would mean something is
  still going on. A card that's still working opens instead, showing the last
  two dozen things that project did.
- **× dismisses too**, and closes a working card down to a one-line chip.
- **`Ctrl+Alt+P`** shows and hides the whole thing from anywhere. If another
  program already owns that chord, Pipsqueak takes the next free one and tells
  you which. Windows only; on Linux you bind `pipsqueak control toggle` in your
  desktop's keyboard settings. See [Install](#install).

## More than one project

Run agents in four repos and you get four cards, not one bubble flickering
between them. Past three, the stack stops growing and collapses. The one you're
watching keeps its card, and everything else becomes a single line that still
says what it's doing and still closes. Click any line to hand it the card.
Anything that starts needing you takes the card back on its own.

![Five projects. Four are collapsed to one line each, and the one waiting on a permission prompt keeps the full card with its question spelled out.](assets/stack.png)

## Install

Everything is on the
[Releases page](https://github.com/tristanmuzzu/Pets-for-Claude-Code/releases/latest).
Pick your platform; the rest of the setup is identical.

### Windows 10 or 11

Download **`Pipsqueak_<version>_x64-setup.exe`** and run it. It needs the Edge
WebView2 runtime, which ships with Windows 11 and installs itself if it's
missing. There's an `.msi` next to it if your organisation prefers one.

### Linux (Ubuntu, Debian, Fedora, or anything else)

The `.deb` is the one to take on Ubuntu and Debian. Download it, then:

```bash
sudo apt install ./Pipsqueak_*_amd64.deb
```

Fedora, RHEL and openSUSE take the `.rpm`:

```bash
sudo dnf install ./Pipsqueak-*.x86_64.rpm
```

Anywhere else (Arch, NixOS, or a distribution whose package manager you'd
rather not involve) takes the **`.AppImage`**, which needs no install at all:

```bash
chmod +x Pipsqueak_*_amd64.AppImage
./Pipsqueak_*_amd64.AppImage
```

Keep the AppImage somewhere permanent before you run it. It registers *itself*
as the hook program, so moving or deleting the file later leaves Claude Code
calling a program that isn't there. `~/Applications` is a good home for it.

Built on Ubuntu 22.04. The highest glibc symbol the binary asks for is
`GLIBC_2.34`, so Ubuntu 22.04, Debian 12, Fedora 35 and anything newer are fine.
The packages pull in what they need. The AppImage expects GTK 3 and WebKitGTK
4.1, which any current desktop already has; if it refuses to start, install
them:

```bash
sudo apt install libwebkit2gtk-4.1-0 libayatana-appindicator3-1
```

**Developed and tested on Wayland** (GNOME 50 on Ubuntu 26.04), which is the
harder of the two. Clicks land on the pet and pass through everywhere else
because the overlay sets a real input shape and lets the compositor do the
routing, instead of polling for a cursor position Wayland will not give it. That
mechanism started life on X11, so X11 should work too. I haven't run it there,
so I'm not going to tell you it does.

Clicking the pet does not take your keyboard. The window asks the window
manager not to focus it at all, so a click still reaches the card you aimed at
and the keys keep going wherever they were already going: poke a card while a
video is full screen and space still pauses the video. The request has to be
made in an X11 window to mean anything: under XWayland GNOME honours it, and a
Wayland-native client is simply given the keyboard anyway. Measured on GNOME
50.1: that run also ignored the pet's saved position and put the window in the
middle of the screen. Run it with `GDK_BACKEND=x11` and both work.

Two things Linux doesn't get:

- **The pet doesn't follow your cursor with its eyes.** A Wayland client is not
  allowed to know where the pointer is unless it is over the window, and a
  click-through window never is. Everything else about the pet is the same.
- **No global hotkey.** `Ctrl+Alt+P` is a Windows registration, and Wayland has
  no equivalent a normal application can make. The tray icon does the same job,
  and your desktop's own keyboard settings do it properly. Bind a custom
  shortcut to:

  ```bash
  pipsqueak control toggle
  ```

  which works whether or not the pet is already running. On GNOME that's
  Settings → Keyboard → View and Customise Shortcuts → Custom Shortcuts.

### Then, on both

On first launch a panel offers to register the Claude Code hooks. That's the only
step that touches your config, and it says exactly what it edits. Then restart
Claude Code, because it reads its hooks at startup. That's the whole setup.

It also registers itself to start with the machine, because a status overlay
that doesn't survive a reboot isn't much of one. That's the Run key on Windows
and an XDG autostart entry on Linux. The welcome panel says so and turns it off
in one click, as does `pipsqueak autostart off`.

There's a plugin too, if you'd rather drive the pet from inside a session:

```bash
/plugin marketplace add tristanmuzzu/Pets-for-Claude-Code
```

> macOS is untested. It builds and the bundle target exists, but I haven't run
> it, and autostart isn't implemented there. A report either way is welcome.

### Building it yourself

Both platforms, from a clone. Prerequisites and the rest are in
[building](docs/project/building.md):

```bash
npm install && npm run app:build
```

### What it does to your settings

`~/.claude/settings.json` is usually hand-tuned, so the installer copies it
first, refuses to run if it isn't valid JSON, and only ever removes entries whose
command path contains `pipsqueak`.

The Windows uninstaller takes the hooks with it. `apt remove`, `dnf remove` and
a deleted AppImage do not, because a package manager doesn't know about a file
in your home directory. So on Linux, run `pipsqueak uninstall` before you remove
the program. Leaving them behind costs a few milliseconds per hook and nothing
else, since a hook whose command is missing simply fails to run.

The hooks are `async` and never write to stdout. That second part is
load-bearing: a hook that prints on `PermissionRequest` can approve or deny the
tool call. This one writes a file and exits, so Claude Code's prompts are exactly
what they'd be without it, and the pet crashing can't affect a session.

## The pets

![Byte idle, working, waiting, failed, reviewing, and mid-hop](assets/states-byte.png)

| | State | When |
| --- | --- | --- |
| 💤 grey | `idle` | nothing running |
| 🔵 blue | `thinking` / `running` | prompt submitted, tool running |
| 🟠 orange | `waiting` | a prompt a human is actually looking at |
| 🔴 red | `failed` | the turn ended badly |
| 🟢 green | `done` | Claude finished |
| 🟣 purple | `compacting` | context compaction |

**They watch your cursor.** Every built-in pet is drawn facing sixteen
directions and turns to look at your pointer while it's resting, then gets back
to work when there's work. Poke one and it jumps.

![Byte drawn facing sixteen directions, clockwise from straight up](assets/look-byte.png)

Three ship in the box: Byte, Pip and Ember. A pet is only a folder with a sprite
sheet and a small JSON file, so bring your own. Pets built for the Codex app work
here unchanged, both versions of that atlas, including any already sitting in
`~/.codex/pets`. It works the other way round too.

## What it refuses to say

The whole thing is worthless if you can't trust one glance at it, so most of the
work went into not claiming things:

- **"Done" only when everything is done.** `Stop` fires whenever the assistant
  yields the floor, including when it yields *because* it is waiting, with two
  subagents still reviewing and a release pipeline it started still running. No
  hook fires when either of those finishes. So the card counts the work the turn
  started, taken from the list Claude Code hands the `Stop` hook and corrected
  between turns by the launches and completions in the transcript. It reads
  **Finishing · 3 running** until the last one reports back. Green means
  finished, the way the sidebar's blue dot does.
- **The counters belong to the turn, and stop when it does.** "18 actions · 3m"
  is the turn's own work and the turn's own length, so a subagent's tool calls
  are not counted as the main agent's, and a finished card's clock does not keep
  climbing over an answer that arrived four minutes ago.
- **The tray only blinks for news that held.** The card is in front of you and
  can correct itself; a tray blink is a tap on the shoulder of somebody looking
  elsewhere, so it waits until the completion has survived with nothing still
  running. A stop hook vetoing a stop never reaches you as a false finish.
- **"Needs you" only after a prompt has gone unanswered for 800ms.**
  `PermissionRequest` fires before anyone is asked, and auto-mode settles most of
  them in a couple of hundred milliseconds. Crying wolf teaches you to ignore the
  one that matters.
- **Nothing at all once the agent is gone.** A sweep retires sessions whose
  process has exited, and drops silent work back to idle after five minutes. It
  says "stopped responding", not "done".
- **A dangerous command says so.** A force push, an `rm -rf`, a `DROP TABLE`,
  all get a warning on the card that's asking you to approve them.

## Privacy

No telemetry, no analytics, no account, no server. The one network call in the
whole app is an update check. It's off by default, it asks GitHub for the latest
release tag, and it downloads nothing.

Session files live in `~/.pipsqueak`. They hold the project name, the current
tool, and the line the card is showing, and they're deleted when the session
ends. The transcript that line comes from is a file Claude Code already wrote to
your disk. Pipsqueak reads it and puts one line of it two inches from your
cursor.

## How it actually works

Claude Code fires [hooks](https://code.claude.com/docs/en/hooks) at defined
moments. Pipsqueak registers a command hook on each relevant event pointing at
its own binary. Each invocation reads the payload, turns it into a state plus a
line, and writes `~/.pipsqueak/sessions/<id>.json`. The overlay polls that
folder, joins in the transcript and the desktop app's own chat records, and
draws. No daemon, no port, no server. If the pet isn't running, the hooks cost a
few milliseconds of file write and nothing else.

There's more of that in [how it works](docs/guides/how-it-works.md): why `Stop`
isn't "finished", how a session gets matched to a repository, what the sweep
does.

## Docs

- [How it works](docs/guides/how-it-works.md), hooks and state and why each
  claim is delayed or withheld
- [Configuration and CLI](docs/guides/configuration.md)
- [Custom pets](docs/guides/custom-pets.md), the atlas format, timing, look
  directions, Codex compatibility
- [Building and releasing](docs/project/building.md)
- [Release notes](docs/releases/) · [Contributing](CONTRIBUTING.md)

## Star it

If Pipsqueak saves you one alt-tab, a star helps the next person find it. ⭐

## Credits

Not affiliated with Anthropic or OpenAI. The pet-on-your-desktop idea is
borrowed from the Codex app's companions. Everything here is written from
scratch against the documented Claude Code hook API and the published pet atlas
contract.

MIT. Take it, fork it, make it yours.
