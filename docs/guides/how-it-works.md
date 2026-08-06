# How it works

[← README](../../README.md)

Claude Code fires [hooks](https://code.claude.com/docs/en/hooks) at defined
moments. Pipsqueak registers a command hook on each relevant event pointing at
its own binary:

```json
{
  "type": "command",
  "command": "C:\\Users\\you\\AppData\\Local\\Pipsqueak\\pipsqueak.exe",
  "args": ["hook", "PreToolUse"],
  "timeout": 5,
  "async": true
}
```

Each invocation reads the hook payload from stdin, turns it into a state plus
one human-readable line, and writes `~/.pipsqueak/sessions/<session-id>.json`.
The overlay polls that directory and animates.

There is no daemon and no port. If the pet is not running, the hooks are a few
milliseconds of file write and nothing else, and the state is still on disk
when it next starts.

Events used: `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`,
`PostToolUseFailure`, `PermissionRequest`, `PermissionDenied`, `Notification`,
`Stop`, `StopFailure`, `SubagentStart`, `SubagentStop`, `PreCompact`,
`PostCompact`, `SessionEnd`.

## What the hooks can and cannot do

Every hook is `async`, so none of them adds latency to a turn, and **none of
them writes to stdout**. That is the important one: a hook that prints on
`PermissionRequest` can approve or deny the tool call. Pipsqueak's writes a file
and exits 0 with nothing on stdout, so Claude Code's own prompt appears exactly
as it would if the pet were not installed. It cannot approve anything, deny
anything, or change what Claude Code decides.

It also means the pet crashing, being killed, or never having been started
cannot affect a session.

## What it does to your settings

`~/.claude/settings.json` is usually hand-tuned, so the installer is careful
with it:

- It **copies the file first** to `settings.json.pipsqueak-backup-<timestamp>`.
- It **refuses to run** if the file is not valid JSON.
- It only ever removes entries whose command path contains `pipsqueak`. Your own
  hooks on the same events are left exactly where they are.
- Uninstalling the app removes them, and `pipsqueak uninstall` does the same by
  hand.

## Three questions, three fields

A session file keeps *what the session is doing* apart from *how the last turn
ended* and *whether a human is blocking it*. Collapsing those into one state
field is what used to leave a card stuck on "Needs you" when a prompt was
answered somewhere the hooks could not see.

- `state`: `idle`, `thinking`, `running`, `compacting`. Only forward progress
  moves it.
- `outcome`: `done` or `failed`, cleared the moment the next turn starts.
- `waiting_since` / `pending_since`: non-zero while something is blocking.

## Where the card's title comes from

A hook runs *inside* a session and knows nothing about the window showing it, so
it can only offer the first line of the prompt. The Claude Code desktop app
knows more: it keeps one record per chat under
`%APPDATA%/Claude/claude-code-sessions/<account>/<org>/local_<id>.json`, and
that record has both the chat's title and the id the app routes by, keyed by the
same session id the hooks write.

The overlay reads those records (head of the file only, skipping anything it
has already seen unchanged) and joins them on. So a card shows the title you
see in the app's own sidebar, and **↗** asks the app to open that exact chat
(`claude://resume?session=<id>`, which for an id the app already holds is a
navigation, not an import).

None of it is required. No app, an older layout, a session started in a
terminal: the card falls back to the prompt's first line, with dropped-in file
paths shortened to filenames, and **↗** falls back to matching a window title.

## What it is doing, in its own words

Hooks fire at tool boundaries, so on their own they can only report a category:
"Editing render.js" says what kind of thing is happening, never the point of it.
The point is in the sentence Claude writes just before it reaches for the tool,
and Claude Code appends that to the session transcript as it happens. The path
comes in on every hook payload as `transcript_path`.

So the overlay follows the transcript: only the bytes appended since the last
poll, only the newest line worth showing, only for sessions that are on screen,
and never past a half-written line. Nothing is sent anywhere. The file is
already on the disk and the line travels as far as a card two inches away.

Three settings, from the right-click menu:

| Setting | What the card says |
| --- | --- |
| Say nothing while working | the tool line, as before |
| Say what Claude tells you | the last thing it said to you, roughly every 80s |
| Say what Claude is thinking | that, plus the last line of each thought, roughly every 20s. The one that feels alive |

Greetings and one-word acknowledgements are skipped, so a reply that opens
"Tristan," narrates the line after it.

## Why `Stop` is not "finished"

Claude Code fires `Stop` whenever the assistant yields the floor, which includes
cases where it is about to carry straight on. Three fields in the payload say
so, and Pipsqueak reads all three:

| Payload | Meaning | Card |
| --- | --- | --- |
| `session_crons` non-empty | scheduled work still running | keeps working |
| `stop_hook_active` | a stop hook asked it to continue | keeps working |
| `background_tasks` non-empty, no final message | still finishing | keeps working |
| `background_tasks` non-empty, with a final message | done, but something trails | settles after 2s |
| none of the above | finished | done, after a 2s hold |

The hold on the last row is not hedging, it is how blocking stop hooks look
from the outside: a review gate or completion loop can veto the stop and have
the turn carry straight on, and `stop_hook_active` only admits that on the
*second* `Stop`. Two seconds is long enough for the continuation to cancel a
premature "Done" before it was ever shown.

## Why the turn ending is not the work ending

None of the above helps with the case that matters most: the assistant yields
the floor *because* it is waiting. Two subagents are still reviewing, or a
release pipeline it started in the background has not reported back, and its
final sentence says exactly that. `Stop` fires anyway, and **no hook of any
kind fires when a background command or a subagent finishes**. From the hooks
alone, that turn is indistinguishable from a finished one.

The transcript is where the answer lives. Claude Code gives every asynchronous
thing an id when it starts it — a background command, a monitor, a subagent —
and names that same id again in the notification when it completes. So the
work still outstanding is simply the launches minus the completions, and the
overlay is already reading that file for the live line.

| The card says | When |
| --- | --- |
| **Finishing · N running** | the turn ended and N things it started have not reported back |
| **Done** | the turn ended and nothing it started is still running |

Which is the distinction the Claude Code sidebar draws with a blinking dot, a
hollow one, and a blue one. A count that never drains — because the overlay
started watching after the launch and missed the completion — is written off
after five minutes of total silence, the same cutoff that retires a session
which stopped producing events.

Two things are deliberately not done here. The pet does not read the
assistant's prose looking for phrases like "waiting for" — that is guessing
dressed as knowing, and the ids are already an exact answer. And it does not
treat the *tray blink* as the same claim as the card: the card can say Done
and take it back a second later, because it is in front of you and it corrects
itself, while a tray blink is a tap on the shoulder of somebody looking
somewhere else. So the blink waits until the completion has held for eight
seconds with nothing still running.

A `SubagentStop` for work that outlived the answer is bookkeeping: it adjusts
the count and touches nothing else. Treated as progress, it cleared the outcome
and put a finished card back to "Delegating" with nothing running.

## Why "needs you" is delayed

`PermissionRequest` fires *before* anyone is asked, and auto-mode or a
permission hook answers most of them within a few hundred milliseconds. A pet
that reacts to the event itself cries wolf constantly.

So the prompt is recorded, and only becomes a visible "needs you" if it is still
unanswered 800ms later, by which point a human really is being looked at. The
same debounce applies to `Notification`/`permission_prompt`, `idle_prompt` and
`agent_needs_input`. Auto-mode declining a call is shown as ordinary progress,
not a failure.

The one place a refusal is counted is the card's red **N blocked** chip, off
`PermissionDenied` — the event Claude Code fires when the auto-mode classifier
refuses a call, and the only signal here that distinguishes "policy said no"
from "the command errored". The count is per turn, so it clears on the next
prompt. Tool calls that merely *failed* are not counted at all: Claude tries
something else, and a tally of that reads as "the agent keeps getting things
wrong" while telling you nothing you can act on. They still show up in the
expanded log, in red, as they happen.

## When a session dies

Nothing writes a file to say Claude Code crashed. A sweep runs every ten
seconds:

- The agent process is gone → the session is retired immediately. Identity is
  `(pid, process creation time)`, never the pid alone, because Windows reuses
  pids and a reused one would report a dead session as alive.
- Running with no events for five minutes → dropped to idle. Claude Code fires a
  hook per tool call, so that silence means the turn died without a `Stop`. Not
  a completion: it says "stopped responding", because that is what happened.
- Nothing at all for twelve hours → deleted.

A session that is genuinely *waiting on you* is never retired by age. That claim
stays true no matter how long you take.

## When it isn't working

Right-click the pet, then **Check my setup**. It verifies that the hooks are
registered, that they point at a program that still exists, and that the session
folder is writable, then watches for ten seconds while you go and run
something, which is the only way to tell "nothing happened" apart from "the
hooks are not firing".

The most common cause is the least interesting one: Claude Code reads its hooks
at startup, so it needs restarting after they are installed.
