<p align="center">
  <img src="assets/portrait-byte.png" width="120" alt="Byte, a small pixel-art robot with a screen for a face" />
</p>

<h1 align="center">Pipsqueak</h1>

<p align="center">
  <strong>Your coding agent, in the corner of your eye.</strong>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="License: MIT"></a>
  <a href="https://github.com/tristanmuzzu/pipsqueak/releases"><img src="https://img.shields.io/badge/Windows-installer-0a7bbb" alt="Windows installer"></a>
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
- **× dismisses.** A finished card says **Done** and waits until you've seen it,
  then goes away completely.
- **Click a card** for the last two dozen things that project did.
- **`Ctrl+Alt+P`** shows and hides the whole thing from anywhere. If another
  program already owns that chord, Pipsqueak takes the next free one and tells
  you which.

## More than one project

Run agents in four repos and you get four cards, not one bubble flickering
between them. Past three, the stack stops growing and collapses. The one you're
watching keeps its card, and everything else becomes a single line that still
says what it's doing and still closes. Click any line to hand it the card.
Anything that starts needing you takes the card back on its own.

![Five projects. Four are collapsed to one line each, and the one waiting on a permission prompt keeps the full card with its question spelled out.](assets/stack.png)

## Install

**Windows 10 or 11.** Download the installer from
[Releases](https://github.com/tristanmuzzu/pipsqueak/releases) and run it. It
needs the Edge WebView2 runtime, which ships with Windows 11 and installs itself
if it's missing.

On first launch a panel offers to register the Claude Code hooks. That's the only
step that touches your config, and it says exactly what it edits. Then restart
Claude Code, because it reads its hooks at startup. That's the whole setup.

There's a plugin too, if you'd rather drive the pet from inside a session:

```bash
/plugin marketplace add tristanmuzzu/pipsqueak
```

> macOS and Linux are untested. The code is portable and the bundle targets
> exist, so if you build it there, a report either way is welcome.

### What it does to your settings

`~/.claude/settings.json` is usually hand-tuned, so the installer copies it
first, refuses to run if it isn't valid JSON, and only ever removes entries whose
command path contains `pipsqueak`. Uninstalling takes the hooks with it.

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

- **"Done" only when the turn is really over.** `Stop` fires whenever the
  assistant yields the floor, including when it's about to carry straight on.
  Pipsqueak checks `stop_hook_active`, `background_tasks` and `session_crons`
  first. A subagent reporting in afterwards is bookkeeping, not new work.
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
