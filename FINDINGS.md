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

## Parked (recorded, not fixed — reasons given)

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
