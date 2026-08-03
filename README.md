# Pipsqueak

A tiny desktop pet that shows what Claude Code is doing, without switching to it.

Pipsqueak sits in the corner of your screen on top of everything else. While
Claude Code works, the pet mirrors its state: thinking, running a tool, blocked
on a permission prompt, failed, or done. Watch a video, read a PR, do anything
else, and still know at a glance whether the agent is busy, stuck, or finished.

![Three Claude Code projects stacked in the corner of a desktop. Two are working, one calling a Vercel MCP tool and one running an eval suite, while the third has turned orange and says "Needs you · Allow Bash(npm test)?". The pet below them shows an exclamation mark on its screen.](assets/demo.png)

<sub>Recorded by `npm run demo`, which paints its own backdrop and drives the app
with simulated sessions, so it is reproducible and contains nobody's desktop.</sub>

---

## Why

Long agent turns are dead time you can only spend elsewhere if you can tell,
without looking at the terminal, when the agent needs you back. A status line in
a window you're not looking at can't do that. A pet on top of everything can.

Which puts most of the design pressure on one thing: **the pet must not say
anything that isn't true.** A card that claims a turn finished when it hasn't,
or claims work is happening after the agent crashed, is worse than no card.
You stop trusting the one glance the whole thing exists for.

## Features

- **Transparent always-on-top overlay.** No taskbar entry, no window chrome.
- **Click-through everywhere except the pet.** The overlay does not eat clicks
  meant for whatever is underneath it. Most pet overlays get this wrong.
- **One card per project, stacked.** Run Claude Code in four repos and you get
  four cards, not one bubble flickering between them. Press **×** to collapse a
  project into a chip; it reopens itself if that project gets blocked or fails.
- **Readable at speed.** Each card has a *headline* (what the turn is
  about) that changes once per turn, and a dimmer live line for the current tool. Every
  state has a floor on how long it stays up, so nothing flashes past.
- **It says what it is blocked on**, with a warning when the command can't be
  undone: a force push, an `rm -rf`, a `DROP TABLE`.
- **Completions survive not being seen.** A turn that finishes while the overlay
  is behind a full-screen window leaves a mark until you look, and blinks the
  tray icon if the overlay is covered entirely.
- **Do Not Disturb** silences both, and the pet visibly dozes so you can see it
  won't interrupt you.
- **A setup check** that watches for real hook traffic, so "nothing is
  happening" and "the hooks aren't firing" stop looking identical.
- **`Ctrl+Alt+P` shows and hides it** from anywhere, so you never have to go
  hunting for the tray icon. If something else already owns that chord,
  Pipsqueak takes the next free one and tells you which.
- **An event log.** Click a card for the last two dozen things that project did.
- **Jump to the window.** **↗** on a card brings that Claude Code window
  forward.
- **Three pets built in**, and you can drop your own sprite folder in.
  **Codex pets work as-is**, including any already in `~/.codex/pets`.

### Why it stays readable

An agent can fire five tools in three seconds. Showing each one is unreadable;
showing none of it is useless. A card is three lines that move at three
different speeds:

```
● CLOCKWORK                              3m  ↗  ×
Fix the flaky timezone test
Editing · 42 actions · 3m
```

| Line | Source | Changes |
| --- | --- | --- |
| Project | the git repository the session belongs to | never |
| Headline | your prompt, then Claude's answer when the turn ends | once per turn |
| Status | the *category* of work, plus counters | on category change |

Nothing on the card strobes. Several tools share one status word on
purpose. `Read`, `Glob` and `Grep` are all `Reading`, so the only thing moving
during a burst is the counter. The exact call is in the tooltip, and the full log is one
click away.

### Which project a session belongs to

Agents work in git worktrees and scripted runs work in temp directories with
generated names, so `basename(cwd)` produces card titles like
`design-quality__nocaveman__r3`. Pipsqueak resolves the **repository** instead,
following `.git` worktree pointers back to the repo that owns them, and falls
back to `CLAUDE_PROJECT_DIR` and then the working directory. A session running
somewhere other than the repo root gets a small badge showing where.

Sessions rooted in a temp directory are treated as scratch and hidden, since
they are runs rather than projects. Turn them on in the menu.

### What it refuses to claim

- **"Done" only when the turn is actually over.** `Stop` fires whenever the
  assistant yields the floor, including when it is about to carry straight on.
  Pipsqueak reads `stop_hook_active`, `background_tasks` and `session_crons`
  before believing it.
- **"Needs you" only after a prompt has gone unanswered for 800ms.**
  `PermissionRequest` fires before anyone is asked, and auto-mode settles most
  of them in a couple of hundred milliseconds.
- **Nothing at all once the agent is gone.** A sweep retires sessions whose
  process has exited, and drops work that has produced no event for five minutes
  back to idle. It says "stopped responding", not "done".

The details, and why each one is the way it is, are in
[how it works](docs/guides/how-it-works.md).

## States

![Byte idle, working, waiting, failed, reviewing, and mid-hop](assets/states-byte.png)

| Pet | State | Fires on |
| --- | --- | --- |
| 💤 grey | `idle` | session started, nothing running |
| 🔵 blue | `thinking` / `running` | prompt submitted, tool running |
| 🟠 orange | `waiting` | a permission prompt a human is actually looking at |
| 🔴 red | `failed` | the turn ended badly |
| 🟢 green | `done` | Claude finished its turn |
| 🟣 purple | `compacting` | context compaction |

## Install

**Windows 10/11.** Download the installer from
[Releases](https://github.com/tristanmuzzu/pipsqueak/releases) and run it.
Requires the Microsoft Edge WebView2 runtime, which ships with Windows 11 and is
installed automatically by the setup if missing.

On first launch a panel offers to register the Claude Code hooks. That is the
only step that touches your configuration, and it says exactly what it edits. Then
**restart Claude Code**, because it reads its hooks at startup.

There is also a plugin so you can control the pet from inside a session:

```bash
/plugin marketplace add tristanmuzzu/pipsqueak
```

See [configuration and CLI](docs/guides/configuration.md) for the commands.

> macOS and Linux are not tested. The code is portable and the Tauri bundle
> targets exist; if you build it there, a report either way is welcome.

## What it does to your settings

`~/.claude/settings.json` is usually hand-tuned, so the installer is
careful with it: it copies the file first, refuses to run if it is not valid
JSON, and only ever removes entries whose command path contains `pipsqueak`.
Uninstalling the app takes the hooks with it.

The hooks are `async` and **never write to stdout**, which is the load-bearing
part: a hook that prints on `PermissionRequest` can approve or deny the tool
call. This one writes a file and exits, so Claude Code's own prompt is exactly
what it would be without the pet installed, and the pet crashing cannot affect a
session.

## Privacy

No telemetry, no analytics, no account. The one network request in the app is
the update check, which is **off by default**, asks GitHub for the latest
release tag and nothing else, and never downloads anything.

Session files hold the project folder name, the current tool and its target, and
a truncated copy of the last assistant message, which is what the card shows
anyway. They live in `~/.pipsqueak` and are deleted when the session ends. The
setup check's copyable report contains no paths, prompts, or file contents.

## Docs

- [How it works](docs/guides/how-it-works.md): hooks, state, and why each claim
  is delayed or withheld
- [Configuration and CLI](docs/guides/configuration.md)
- [Custom pets](docs/guides/custom-pets.md): the atlas format, timing, and
  Codex compatibility
- [Building and releasing](docs/project/building.md)
- [Release notes](docs/releases/)
- [Contributing](CONTRIBUTING.md)

## Credits

Not affiliated with Anthropic or OpenAI. Inspired by the Codex app's pets;
implemented from scratch against the documented Claude Code hook API and the
community-documented pet atlas layout.

MIT licensed.
