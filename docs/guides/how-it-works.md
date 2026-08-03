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
| none of the above | finished | done |

## Why "needs you" is delayed

`PermissionRequest` fires *before* anyone is asked, and auto-mode or a
permission hook answers most of them within a few hundred milliseconds. A pet
that reacts to the event itself cries wolf constantly.

So the prompt is recorded, and only becomes a visible "needs you" if it is still
unanswered 800ms later, by which point a human really is being looked at. The
same debounce applies to `Notification`/`permission_prompt`, `idle_prompt` and
`agent_needs_input`. Auto-mode declining a call is shown as ordinary progress,
not a failure.

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

Right-click the pet → **Check my setup**. It verifies that the hooks are
registered, that they point at a program that still exists, and that the session
folder is writable, then watches for ten seconds while you go and run
something, which is the only way to tell "nothing happened" apart from "the
hooks are not firing".

The most common cause is the least interesting one: Claude Code reads its hooks
at startup, so it needs restarting after they are installed.
