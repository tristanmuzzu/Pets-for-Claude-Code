# Findings ledger

Deep audit, 2026-08-06 (Fable 5, five parallel audit lenses + hook-API doc
verification). Each finding: mechanism at the line, status. Fixed items name
the commit subject; parked items say why.

## The symptom that started this

"Cards pop up and then disappear again immediately; strange transient visuals."
Root causes found, in order of contribution:

| # | Mechanism | Status |
| --- | --- | --- |
| S-1 | `Stop` with background tasks writes `state:"idle"` + 2s settle; `displayState` returns `idle` during the settle window, card leaves the DOM, returns 1–3s later as done | FIXED |
| H-2 | First `Stop` gets `settle_ms: 0`; a blocking stop hook (this user runs one) forces continuation and the just-shown Done card is wiped by the next event | FIXED |
| W-1 | Frontend-liveness watchdog trusts a throttleable JS timer; WebView2 clamps timers for occluded windows to ~1/min, so the overlay hard-reloads every 60s while occluded | FIXED |
| F-3 | Reordering a live card node (`prepend`/`after`) restarts the 160ms `rise` entrance animation from `opacity:0` — the card blinks when promoted | FIXED |
| F-5 | A notice injects a synthetic done-session that takes a slot: 4th slot flips dense mode, whole stack collapses for 6s, tray flashes | FIXED |
| H-3/I-4/S-2 | `SessionEnd` deletes the session file and its lock without holding the lock; an in-flight hook resurrects it as a zombie card that the sweep later kills | FIXED |
| S-5/F-2 | Sweep deletes a session file on a *transient read failure* (AV/indexer sharing violation), next hook recreates it bare — delete/recreate flap | FIXED |
| S-3/W-6 | Sweep does lock-free read-modify-write against files hooks are writing; can write back a stale snapshot ("Stopped responding" flash on a fresh turn) | FIXED |
| S-4 | `CreateToolhelp32Snapshot` transient failure reads as "every process is dead" → live session deleted, recreated on next hook | FIXED |
| W-2 | `pipsqueak control show` unconditionally reloads a healthy page — every `/pet` invocation blanks and repaints the overlay | FIXED |
| F-1 | Frontend tears down all card state (view, acknowledged, seen, slot) the instant a session misses one emit; combined with any transient dropout above, full flash + re-greet | FIXED (root causes above + read retry) |

## Fixed — backend truthfulness

- **S-1 / H-2** `hook.rs:503`, `derive.js:93` — every `Stop::Finished` now
  settles for 2000ms, and `displayState` reports `running` (not the stored
  `idle`) inside the settle window, so the card never leaves the DOM at turn
  end and a stop-hook-forced continuation cancels the outcome before it was
  ever shown.
- **S-7** `hook.rs:123` — `Stop`/`StopFailure` now clear `waiting_since`; a
  denied permission followed by turn end no longer leaves "Needs you" up for
  hours.
- **H-9** `hook.rs:671` — a `SubagentStop` when no subagents are outstanding is
  bookkeeping even when the outcome was already cleared by a new prompt.
- **H-5** `hook.rs:142` — a new permission prompt for the same tool but a
  different command refreshes detail, risk and the debounce clock instead of
  showing the previous command's text.
- **H-6** `hook.rs:280` — `SessionStart` reads `source`; `"compact"` no longer
  resets a working card to "Session started" mid-turn (doc-verified: the event
  fires mid-session after auto-compaction).
- **H-7** `hook.rs:354` — `Notification` without the undocumented
  `notification_type` field falls back to classifying by message text instead
  of being dropped (doc-verified: the field is not in the payload schema).
- **H-3/I-4/S-2** `hook.rs:49` — `SessionEnd` acquires the file lock before
  deleting and leaves the lock file to expire; events other than
  `SessionStart`/`UserPromptSubmit` no longer create a session file from
  nothing, so stragglers cannot resurrect an ended session.
- **H-8** `narration.rs` — transcript entries with `isSidechain: true` are
  skipped; a subagent's inner monologue no longer flickers onto the parent
  card.

## Fixed — sweep and liveness

- **S-3/S-5/W-6/F-2** `state.rs:542` — the sweep takes the per-file lock for
  its read-modify-write; an IO read failure skips the file (retry next sweep)
  instead of deleting it; only a successfully read but unparseable file is
  removed.
- **S-4** `process.rs` — a failed process snapshot now reads as "unknown, treat
  as alive" instead of "everything is dead".
- **S-11** `state.rs:470` — a lock file whose age cannot be read is no longer
  stolen instantly; stealing requires a provably expired lock.
- **F-1** `state.rs:503` — `read_sessions` retries a failed read once before
  dropping a session from the emit.

## Fixed — window layer

- **W-1** `app.rs`, `main.js` — liveness no longer trusts a throttleable
  timer: when the page has been silent the backend emits a ping and only
  reloads if the page fails to answer it; a throttled-but-alive page answers
  events immediately.
- **W-2** `app.rs:820` — `control show` reloads only a page that is actually
  silent.
- **W-3** `app.rs` — the heartbeat is written at setup, before the first poll
  tick, and a second instance that finds a live heartbeat at startup defers to
  it (shows the running overlay and exits) instead of stacking a duplicate pet.
- **W-5/F-14/W-11** `app.rs`, `main.js` — the frontend calls `frontend_ready`
  after its listeners attach; the poller re-emits its state, so changes that
  land in the boot/reload gap (including pet-switch commands) are not lost.
- **W-8** `control.rs:27` — `pipsqueak control stop` quits; only
  `off`/`hide` hide (matches the "put it away" vs "stop it" split).
- **W-4** `app.rs`, `main.js` — tray toggles emit the full config, the
  frontend listens and merges, and the tray checkboxes are updated on every
  config change; the two sides no longer overwrite each other's settings.
- **W-12** `app.rs:488` — default first-launch placement uses the monitor work
  area instead of a hard-coded taskbar guess.

## Fixed — frontend

- **F-3/F-12** `main.js`, `style.css` — the entrance animation is a one-shot
  class applied only to freshly built cards (removed on `animationend`, hit
  rects re-synced then); moving a card no longer replays it.
- **F-4** `main.js:498` — the chip row is only re-appended when out of place.
- **F-5** `main.js` — a notice always shows at the front of the stack but no
  longer counts toward the density flip, flashes the tray, or marks itself
  unread; with three chats up, one chat temporarily becomes a chip instead of
  every card collapsing for six seconds.
- **F-9** `main.js:776` — the doctor's countdown and completion timers are
  cancelled when the panel closes or rebuilds.
- **F-10** `main.js` — chips are only offered for live chats; the chip for an
  idle chat restored nothing and its only behavior was to vanish when clicked.
- **F-11** `main.js:215` — `acknowledge` only reports success when it recorded
  one, so clicks during a hold window expand/close instead of doing nothing.
- **F-8** `main.js` — the update-check toggle starts and stops the scheduler
  at the moment it is flipped.
- **F-13** `pet.js` — one-shots start at frame 0 (not the previous row's
  frame) and a state change during a one-shot crossfades from the state row.

## Fixed — installer, doctor, log, misc

- **I-1** `Cargo.toml`, `install.rs` — serde_json `preserve_order`; the
  installer keeps key order, skips the write when nothing changed, and no
  longer plants an empty `hooks` object on uninstall.
- **I-2/I-13** `log.rs` — rotation cuts at a char boundary (no more abort on a
  multibyte midpoint under `panic = "abort"`) and reads lossily so one bad
  byte cannot disable rotation forever.
- **I-5** `install.rs` — settings backups are pruned to the newest five.
- **I-7** `install.rs` — install strips our hooks from every event key (not
  just the current list) before re-inserting, so renamed events cannot orphan.
- **I-9** `doctor.rs:174` — path comparison is case-insensitive and
  separator-insensitive on Windows.
- **I-10** `doctor.rs` — stale traffic (no events for over an hour) warns
  instead of reading green.
- **S-8** `chats.rs` — duplicate chat records for one session resolve to the
  most recently modified, via one shared function, so the title cannot flap
  between rescans and the open button opens the chat the title names.
- **S-9** `chats.rs:243` — surrogate-pair `\u` escapes decode; an emoji title
  no longer silently drops the whole title.
- **S-10** `project.rs` — scratch detection anchors at real OS temp roots
  (`D:\Temp\project` is a project); an unparseable `.git` file continues the
  ancestor walk instead of aborting it.
- **S-12** `risk.rs` — `git branch -D` and `-d` are distinguished (flag case
  is preserved); `bash -c` / `powershell -Command` / `cmd /c` are unwrapped
  one level so a dangerous inner command is still seen.
- **I-17** `.gitignore` — `.harness/` ignored.

## After v0.7.2 — the counting the last release introduced

- **N-1** `narration.rs:245` — only `<status>completed</status>` drained the
  outstanding set, and a real transcript ends work four ways. Across this
  machine's transcripts: 1156 `completed`, 65 `failed`, 22 `stopped`, 10
  `killed`. One failed subagent therefore left the count permanently one too
  high, so every later turn in that session read **Finishing · 1 running**,
  never went green, and never blinked the tray — the five-minute write-off only
  starts once the session goes *entirely* quiet, which a working session never
  does. All four terminal statuses now drain; `running` deliberately does not.
  FIXED, with a test per status.
- **N-2** `main.js:1588` — the browser demo still populated `subagents`, which
  nothing has read since the count moved to the transcript, so the "3 running"
  chip was missing from every demo run and from anything recorded off it.
  FIXED (`outstanding`).
- **N-3** the red chip counted `PostToolUseFailure` and read "N retried". A
  tool that errored is not something anyone can act on, and Claude usually
  just tries something else. It now counts `PermissionDenied` — documented as
  "denied by the auto mode classifier" — and reads "N blocked". FIXED.

## After v0.7.3 — the claim the whole overlay is for

- **N-4** `state.rs:630` — the sweep downgraded any running session silent for
  five minutes to `stalled`, *including one blocked on a human*. Being asked a
  question produces no further hook events by definition, so a real permission
  prompt is indistinguishable from death by that rule. Five minutes after
  walking away from a prompt, the card read **Stopped responding** with the
  question still on it, and `how-it-works.md` had been claiming the opposite
  ("a session genuinely waiting on you is never retired by age") since it was
  written. Reproduced live against the installed 0.7.3 build: a session with
  `waiting_since` six minutes old came back `state: idle, stalled: true`.
  FIXED — the decision is now `has_stopped_responding`, a pure function with
  tests, and a session whose agent really died is still retired by the process
  check that runs before it.
- **N-5** `main.js:439` — `kindLabel` checked `stalled` before `waiting`, so
  even with the backend fixed, any session flagged by an older build showed
  "Stopped responding" over its own question. The checkable claim now wins.
- **N-6** `.github/workflows/release.yml` — the release workflow ran `npm test`
  and `cargo test` but not `clippy -D warnings` or `cargo fmt --check`, the two
  gates `ci` enforces. That is how v0.7.2 shipped green from a commit whose
  `ci` run was red. Both added. The release body also said "See
  docs/releases/vX.Y.Z.md" instead of containing the notes; it now takes the
  file.

## After v0.7.4 — the numbers around the sentence

Reported as "the pet is bullshitting": the quoted line is what Claude really
said, but everything around it — **Finishing · 7 running · 1 action · 4m** on a
turn that was over and had nothing running — was not. Three parallel read-only
audits plus a replay of the real matcher over 250 transcripts and 376 captured
hook payloads.

- **P-1** `narration.rs:230` — `"taskId"` is overwhelmingly the *to-do list's*
  field, not an async one: 694 of 723 occurrences across 233 transcripts were
  `TaskUpdate` ticking a checkbox, ids `"1"`, `"2"`, `"3"`. Each added a
  background task that could never finish, because no completion notification
  for a checkbox exists. Measured on 250 real transcripts: **38 sessions ended
  holding 298 phantom runners**; the screenshot's own session held 14, of which
  7 had accumulated by the moment it was taken. FIXED — `toolUseResult` is read
  as an object, only the four shapes Claude Code actually launches work with
  count, and all-numeric ids are rejected. Same 250 transcripts after: 7
  leftovers in 7 sessions, every one a background shell still alive at session
  end.
- **P-2** `narration.rs:262` — a resumed session emits one notification listing
  every task the previous session abandoned, up to ten `<task-id>` tags under a
  single `<status>stopped</status>`. `split_once` read the first and discarded
  the other nine, which then sat in the count for the rest of the session.
  FIXED — every id in the notification is retired, `__orphan_summary__` markers
  skipped.
- **P-3** `narration.rs:234` — an `attachment` echoing a finished task's id on a
  later line *re-added* it, by the very line saying it had completed. FIXED — a
  result or attachment carrying a finished status retires its id instead.
- **P-4** `main.js:423` — the elapsed was `duration(turn_started_ms)` against a
  live clock, and `turn_started_ms` is cleared only by the next prompt. A
  two-minute turn that finished two minutes ago read **4m** and went on
  climbing for as long as anyone looked at it. FIXED — the hook stamps
  `turn_ended_ms` and `turnElapsed` freezes there; the sweep stamps it too, so
  a "Stopped responding" card stops counting as well.
- **P-5** `hook.rs:133` — a subagent's tool calls arrive on the *parent's*
  session id with `agent_id` set, and were counted as the turn's own actions
  and allowed to rename its status line. Verified live: of 131 tool events in
  one session, 21 were subagents'. The card said "Editing spiralplan.py" while
  the main agent sat waiting for three agents to report. FIXED — `turn_tools`
  counts the main chain, and a delegated call reads "Delegating".
- **P-6** `hook.rs:137` — `UserPromptSubmit` was the only turn boundary, so a
  turn begun by a stop hook sending one back to work, or by a resume, inherited
  the previous turn's counters and clock. FIXED — `prompt_id`, which is in
  every payload, is the boundary; stragglers cannot rotate it back.
- **P-7** `hook.rs:227` — `event_ms` was documented as the race guard ("the
  older one must lose") and was written by one line and read by none. Hooks are
  installed `async: true`, so a `PreToolUse` that started before a `Stop` and
  landed after it cleared the outcome and set the card back to "running" with
  nothing running. FIXED — the stamp is taken before the lock (taking it after
  ordered the events backwards) and an out-of-order event may still count its
  tool call but may not un-finish a turn.
- **P-8** `hook.rs:166` — "anything other than the prompt itself means the
  prompt is over" was written as an exclusion list of two, so `SubagentStop`,
  `PreCompact` and `SessionStart` cleared a permission prompt nobody had
  answered — and a subagent finishing while the main agent waits at a prompt is
  routine. "Needs you" vanished while Claude Code was still asking, and the
  sweep, which reads a cleared `pending_since` as silence, was then free to
  call the session dead. FIXED — only an event proving the tool ran, was
  refused, or that the turn ended may clear it, and never a subagent's.
- **P-9** `hook.rs:383`/`hook.rs:530` — a `Task` call added a subagent and the
  `SubagentStart` for that same subagent added another, against one
  `SubagentStop` removing one. Measured live: three agents launched, count read
  six, and it never returned to zero — which is the value `is_trailing_subagent`
  tests. FIXED — a set of the `agent_id`s in the payload.
- **P-10** `main.js:397` — the "N running" chip read the raw number while the
  status word used the five-minute write-off, so a session silent for an hour
  carried "7 running" next to the word "Done". `kindLabel` could also print
  "Finishing · 0 running" when the work drained inside the state's display
  hold. FIXED — both go through `runningCount`, and zero prints as "Finishing".
- **P-11** `main.js:212` — `holdLine` returned the previous line whenever the
  new one was empty, and the sweep clears `activity` precisely to stop the card
  claiming work is in progress. So the biggest text on a "Stopped responding"
  card was a sentence from the turn before. FIXED.
- **P-12** `main.js:120` — a new view seeded `lastStable: 'running'`, so a
  session whose first event is a permission prompt spent the debounce claiming
  to be working, with a card, a slot and the pet animating as busy, on no
  evidence. FIXED — seeded `idle`.

## After v0.8.0 — the click that took the keyboard

Reported from the desktop it runs on (GNOME 50.1, Ubuntu 26.04, pet under
XWayland): clicking a card while a video was full screen left space and escape
going nowhere until the video was clicked again. The overlay had become the
focused window, which is the one thing it should never be.

- **L-1** `tauri.conf.json` — the window was created focusable, so mutter gave
  it the keyboard on the first click. `focus: false` only covers the *initial
  map*, and setting the GTK input hint from `setup()` does not survive either:
  tao restores `accept_focus(true)` on the first draw of a window built
  `focusable && !focused`
  (`tao-0.35.3/src/platform_impl/linux/window.rs:221`). FIXED — `focusable:
  false` on the window, which tao turns into the ICCCM input hint on X11,
  `WS_EX_NOACTIVATE` on Windows and `canBecomeKeyWindow: NO` on macOS, and
  which also stops the draw-time restore from running. Measured before and
  after with a click driven onto the pet sprite: before, focus moved from
  Chrome to Pipsqueak; after, the pet collapsed its cards (so the click landed)
  while a separate window kept receiving every keystroke typed either side of
  the click.
- **L-2** — a Wayland-native run drops the request entirely: the window takes
  focus on map and ignores its saved position. Not a regression and not
  fixable from the client side (xdg-shell has no way to refuse keyboard focus);
  the README now says so and says to use `GDK_BACKEND=x11`.
- **L-3** `app.rs:1033`, `desktop.rs:124` — found while testing L-1, three
  times over: `ensure_autostart` rewrote the XDG entry whenever its `Exec` was
  not literally this binary, and "not this binary" is not "dead". An entry that
  launches the pet through a wrapper script — to set the `GDK_BACKEND=x11` L-2
  needs, or to wait for the shell's tray support — was replaced by a bare path
  on every single start, silently undoing both, and the doctor told the owner
  of that entry to reinstall and finish the job. FIXED — an entry is corrected
  only when the program it names is gone (`program_is_present`: a file that
  exists and can be executed, a bare name resolved against `PATH`), and the
  doctor says the entry is being left alone rather than advising a reinstall.
  Verified on both branches: a wrapper entry came through a start byte for byte
  identical, and an entry pointing at a path that does not exist was still
  corrected.

## Parked (recorded, not fixed — reasons given)

- **P-13 (counter increments rest on a best-effort lock)** `hook.rs:134`,
  `state.rs:477` — `FileLock::acquire` gives up after ~200ms and writes
  unlocked, and it will steal a lock older than 2s from a holder that is still
  alive, whose `Drop` then deletes the thief's lock. N parallel `PreToolUse`
  processes can therefore collapse N increments into one. Not seen in the
  captured payloads (376 events, no lost `tool_use_id`), and the fix — refusing
  to write rather than writing unlocked — trades a wrong count for a missing
  event, which needs its own measurement first.
- **P-14 (the hook does not retry a transient read)** `hook.rs:74` — `sweep`
  retries once for exactly this reason and the hook does not, so a file briefly
  held by an indexer loses that event's increment, or its whole outcome.
  One-line fix, deliberately separated from this pass so it can be measured.
- **P-15 (`PermissionDenied` is labelled "Auto-mode blocked")** `hook.rs:439` —
  the payload carries a `reason`, observed as `"Blocked by classifier"`. Since
  it is present, the label should quote it rather than assume the classifier;
  as it stands a denial from a `deny` rule or a permission hook would be
  attributed to auto-mode. Needs a sample of a non-classifier denial first.
- **P-16 (`Notification` fallback misses wordings)** `hook.rs:469` — with
  `notification_type` absent, only "permission", "waiting for your" and "input"
  are matched; "approval", "approve" and "confirm" are not recorded at all.

- **H-1/S-6 (general event-ordering guard)** — hook payloads carry no fire
  timestamp and process-spawn skew means arrival time cannot reconstruct fire
  order. The concrete harms are individually fixed (settle absorption, strict
  file creation, waiting cleared on Stop). A correct general guard needs a
  timestamp in the payload, which Claude Code does not provide. Residual: a
  straggler `PostToolUse` landing >2s after `Stop` briefly revives "running";
  self-limiting via the sweep.
- **H-4 (permission-prompt straggler)** — same missing-timestamp problem;
  bounded by the 800ms debounce plus clear-on-next-event. Rare, cosmetic.
- **W-3b (named mutex single-instance)** — heartbeat-at-setup plus the startup
  check covers the observed races; a named-mutex/plugin approach is the
  durable fix but adds a dependency and touches startup semantics. Do with the
  next dependency bump.
- **W-7 (show before first paint)** — plausible one-frame flash on some GPU
  paths only; deferring `show()` interacts with the watchdog's visibility
  check. Revisit if reported.
- **W-9 (hotkey late-success after 3s timeout)** — cosmetic mismatch between
  log and reality on very slow machines.
- **W-10 (`ensure_on_screen` vs live drag; config write races)** — needs a
  drag-state signal or a config-writer mutex; small blast radius.
- **F-7 (dense-mode hysteresis)** — main triggers (F-1 flap, F-5 notice) are
  fixed; add hysteresis only if the 3↔4 boundary still convulses in practice.
- **F-6 (`acknowledged` keyed on `outcome_ms`)** — with settle and ordering
  fixes `outcome_ms` is stable per completion; a genuine second completion is
  a new event and should reappear.
- **H-12 (grapheme-cluster truncation)** — needs a segmentation dependency for
  a cosmetic edge (emoji cut before the ellipsis).
- **H-13 (missing `session_id` funnels to `unknown.json`)** — version-skew
  only; harmless bucket.
- **S-13 (sanitize collisions)** — adversarial ids only; suffixing a hash
  changes on-disk filenames mid-upgrade.
- **I-3 (undocumented Stop extras)** — `background_tasks`/`session_crons`/
  `last_assistant_message` are not in the docs; the code already degrades
  gracefully when absent. Re-verify each Claude Code release.
- **I-6 (`is_ours` substring match)** — changing the match rule strands hooks
  installed by older builds; needs a migration story.
- **I-8 (settings.json read-modify-write race with Claude Code)** — needs
  retry-compare; rare, and the backup limits the damage.
- **I-11/I-12 (doctor wording nits)** — cosmetic.
- **I-14 (main.js has zero tests)** — the render/reconciliation layer needs a
  DOM test harness; out of scope for this pass, first candidate for the next.
- **I-15 (simulate.mjs payload drift)** — capture-and-replay real hook
  payloads; needs a recording session.
- **F-15 (browser-demo fallback)** — if `__TAURI_INTERNALS__` is ever absent
  the app silently runs the demo loop; worth an explicit banner eventually.
