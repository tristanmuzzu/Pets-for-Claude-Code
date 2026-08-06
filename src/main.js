import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { PetRenderer, GREETING_ROW, JUMP_ROW } from './pet.js'
import {
  ACTIVE,
  DONE_LINGER_MS,
  RUNNING,
  SLEEP_AFTER_MS,
  URGENT,
  WAITING_DEBOUNCE_MS,
  blockedOn,
  displayState,
  duration,
  holdState,
  isNewer,
  relativeTime
} from './derive.js'

/** `npm run dev` in a plain browser has no IPC; fall back to a demo loop. */
const IS_TAURI = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

/** Replaced at build time from package.json. See vite.config.js. */
const APP_VERSION = typeof __APP_VERSION__ === 'string' ? __APP_VERSION__ : '0.0.0'

/** How many chat cards are shown in full before the stack collapses. */
const SLOT_LIMIT = 3
/** How many chats fit once they are one line each. */
const DENSE_LIMIT = 6

const el = {
  stack: document.getElementById('stack'),
  chips: document.getElementById('chips'),
  menu: document.getElementById('menu'),
  panel: document.getElementById('panel'),
  panelTitle: document.getElementById('panel-title'),
  panelBody: document.getElementById('panel-body'),
  panelClose: document.getElementById('panel-close'),
  pet: document.getElementById('pet'),
  hint: document.getElementById('hint'),
  template: document.getElementById('card-template')
}

const appWindow = IS_TAURI ? getCurrentWindow() : null
const renderer = new PetRenderer(el.pet)

let config = {
  pet: 'byte',
  scale: 2,
  clickThrough: false,
  showBubble: true,
  showScratch: false,
  alertOnWaiting: false,
  flashOnFinish: true,
  quiet: false,
  /** off | speech | thoughts. See narration.rs. */
  narrate: 'thoughts',
  welcomed: false,
  updateCheck: false,
  updateDismissed: ''
}
let sessions = []
/** When any project was last doing something, for the doze. */
let lastLiveAt = Date.now()
let notice = null
let noticeTimer = null
/**
 * How long the cursor has to rest on the pet before it explains itself.
 *
 * The operating system's own tooltips wait about half a second, which is short
 * enough to fire while you are reaching past the pet for something behind it.
 */
const HINT_DELAY_MS = 2500
let hintTimer = null
let stackHidden = false
let seenSessions = new Set()

/**
 * Card order is a property of the UI, not of the data. Sorting by "most
 * recently updated" on every hook event made the visible set churn every few
 * hundred milliseconds. A chat keeps its slot until it goes quiet.
 */
let slots = []
const views = new Map()
const collapsed = new Set()
const cards = new Map()
const chipNodes = new Map()

/**
 * Completions that have been seen.
 *
 * A finished card has no timer on it: it says "Done" until somebody
 * acknowledges it, and then it goes away entirely rather than shrinking to a
 * chip, because a chip is a thing that is still going on. Keyed by the turn
 * rather than the project, so the *next* completion in the same project is
 * news again.
 */
const acknowledged = new Set()
/**
 * The chat holding the one full card while the stack is collapsed.
 *
 * Null means "whichever is nearest the pet", which is the busiest or most
 * urgent one. Clicking a collapsed line hands it the card, because the thing
 * you just clicked is by definition the thing you want to read.
 */
let promoted = null
const outcomeKey = (session) => `${session.session_id}@${session.outcome_ms || 0}`
const settled = (state) => state === 'done' || state === 'failed'
/** Recent enough that finishing is still news rather than a standing fact. */
const isFresh = (session) => Date.now() - (session.outcome_ms || 0) < DONE_LINGER_MS

// --- data ---------------------------------------------------------------
function viewFor(key) {
  let view = views.get(key)
  if (!view) {
    view = {
      expanded: false,
      lastStable: 'running',
      wasUrgent: false,
      // A finished turn nobody has acknowledged yet.
      unread: false,
      lastShown: '',
      // The state currently on screen, and the earliest it may be replaced.
      heldState: '',
      heldUntil: 0
    }
    views.set(key, view)
  }
  return view
}

/** What to call a session when it has no chat title of its own yet. */
const projectName = (session) => session.project || session.session_id.slice(0, 8)

/**
 * One card per chat.
 *
 * These used to be grouped by project, with the busiest session speaking for
 * the rest. Two chats open on the same repository are both busy, so the card
 * swapped identity every time the other one ran a tool: title, narration,
 * subagent count and elapsed time all belonged to whichever chat had moved
 * last. It read as one card flickering between two chats, or as a chat being
 * removed. A chat is the thing a person has open, so a chat is what gets a
 * card; the project name stays on it as context.
 */
function cardsFor(list) {
  const out = []
  for (const session of list) {
    if (session.scratch && !config.showScratch) continue
    const key = session.session_id
    const state = effectiveState(key, session)
    out.push({
      key,
      session,
      state,
      live: ACTIVE.has(state),
      // The chat's own name when the desktop app knows it, and the project
      // otherwise: what a one-line chip has room to say.
      label: session.chat_title || projectName(session)
    })
  }
  return out
}


function effectiveState(key, session) {
  const view = viewFor(key)
  const raw = displayState(session) || view.lastStable
  view.lastStable = RUNNING.has(raw) || raw === 'idle' ? raw : view.lastStable
  // Acknowledged means gone, immediately and without a farewell: holding the
  // state for another two seconds would make the click feel unheard.
  if (settled(raw) && acknowledged.has(outcomeKey(session))) {
    view.heldState = ''
    view.heldUntil = 0
    return 'idle'
  }
  return holdState(view, session, raw)
}

/**
 * Keeps a line on screen long enough to be read.
 *
 * The tool line can change three times in a second during a burst of reads.
 * Codex's overlay gets away without this because a model narrating itself
 * changes its mind every twenty seconds; ours falls back to tool calls, which
 * do not. A floor of a second is the difference between a line and a flicker.
 */
const LINE_HOLD_MS = 1000

/**
 * What the main line says when the session has said nothing at all.
 *
 * Rare, and worth handling anyway: a session that has only just started, or one
 * whose transcript the overlay cannot read, would otherwise leave the biggest
 * text on the card blank.
 */
function restingLine(state) {
  if (state === 'waiting') return 'Waiting for you'
  if (state === 'done') return 'Finished'
  if (state === 'failed') return 'The turn failed'
  if (state === 'idle') return 'Nothing running'
  return 'Working…'
}

function holdLine(view, line, now = Date.now()) {
  if (!line) return view.line ?? ''
  if (view.line === line) return line
  if (view.line && now < (view.lineUntil ?? 0)) return view.line
  view.line = line
  view.lineUntil = now + LINE_HOLD_MS
  return line
}

/** Marks a finished turn as seen, which is what removes its card. */
function acknowledge(key) {
  const card = cardsFor(sessions).find((candidate) => candidate.key === key)
  if (!card || !settled(card.state)) return false
  // The display hold can keep "done" on screen after a new turn already
  // cleared the outcome. There is nothing to acknowledge then, and claiming
  // success anyway made the click do nothing at all: not dismissed, not
  // expanded, not collapsed.
  if (!card.session.outcome) return false
  acknowledged.add(outcomeKey(card.session))
  viewFor(key).unread = false
  return true
}

// --- rendering ----------------------------------------------------------
function buildCard(key) {
  const node = el.template.content.firstElementChild.cloneNode(true)
  // The entrance animation rides a one-shot class: kept on the .card rule it
  // replayed from opacity 0 whenever the card was reordered, because moving a
  // connected node re-inserts it and re-insertion restarts its animations.
  node.classList.add('card-enter')
  const entered = () => {
    node.classList.remove('card-enter')
    // The rect measured mid-rise is a few pixels off; measure again now that
    // the card is where it will stay.
    syncHitRects()
  }
  node.addEventListener('animationend', (event) => {
    if (event.target === node) entered()
  })
  // A hidden window pauses animations, so animationend may never come; the
  // class must still die or every queued entrance replays on unhide.
  setTimeout(entered, 1000)
  node.querySelector('.close').addEventListener('click', (event) => {
    event.stopPropagation()
    // Closing a finished card dismisses it outright. Leaving a chip behind
    // would keep a project in the row that means "still going on".
    if (!acknowledge(key)) collapsed.add(key)
    render()
  })
  node.querySelector('.focus').addEventListener('click', (event) => {
    event.stopPropagation()
    const card = cardsFor(sessions).find((candidate) => candidate.key === key)
    // The key *is* the session, so this always opens the chat on the card
    // rather than whichever chat in the project happened to move last.
    invoke('focus_session', {
      sessionId: key,
      project: card ? projectName(card.session) : '',
      workspace: card?.session.workspace ?? ''
    }).catch(() => {})
  })
  node.addEventListener('click', () => {
    const view = viewFor(key)
    // Three things one click can mean, in order of how sure we are of it.
    // Finished: you have seen it, so it goes. Collapsed to a line: you want to
    // read this one, so it takes the card. Otherwise: open the detail.
    if (acknowledge(key)) {
      // Nothing else to do; the card is on its way out.
    } else if (node.dataset.density === 'compact') {
      promoted = key
    } else {
      view.expanded = !view.expanded
    }
    view.unread = false
    invoke('clear_attention').catch(() => {})
    render()
  })
  return node
}

function paintCard(node, card, compact = false) {
  const { session, key, state } = card

  node.dataset.state = state
  node.dataset.density = compact ? 'compact' : 'full'
  node.querySelector('.project').textContent = projectName(session)
  node.querySelector('.age').textContent = relativeTime(session.updated_ms)

  const workspace = node.querySelector('.workspace')
  workspace.hidden = !session.workspace
  workspace.textContent = session.workspace || ''

  const view = viewFor(key)
  node.querySelector('.unread').hidden = !view.unread

  // What the app itself calls this chat, when it knows: the title in its own
  // sidebar, written by the thing that has read the whole conversation. A
  // prompt's first line is a fair guess at the subject; this is the answer.
  node.querySelector('.title').textContent =
    session.chat_title || session.headline || ''

  // What it is doing right now, which is the line the card is really for. Its
  // own sentence when it has said one, and the tool line when it has not:
  // "Editing render.js" is the category of the work, "Now the frontend: chat
  // titles and the done card" is the point of it. Written twice because a
  // collapsed card shows it on the head row instead of under it.
  const said = holdLine(view, session.narration || session.activity || '') || restingLine(state)
  node.querySelector('.narration').textContent = said
  node.querySelector('.line').textContent = said

  // What it is blocked on, in its own row. This is the only text on the card
  // that is worth interrupting something else to read.
  const ask = node.querySelector('.ask')
  // The reason is sticky for as long as the card reads "needs you". The
  // minimum-display hold can keep that state up for a couple of seconds after
  // the prompt is answered, and "Needs you" with nothing underneath it is a
  // question the card has stopped being able to answer.
  const live = blockedOn(session)
  if (live) view.lastReason = live
  const reason = state === 'waiting' ? live || view.lastReason || '' : ''
  const risk = node.querySelector('.risk')
  const askWasHidden = ask.hidden
  ask.hidden = !reason
  // The unfold is a one-shot for the same reason as the card entrance: it
  // must play when the row appears, not every time the card moves.
  if (askWasHidden && !ask.hidden) {
    ask.classList.add('ask-enter')
    const entered = () => ask.classList.remove('ask-enter')
    ask.addEventListener('animationend', entered, { once: true })
    setTimeout(entered, 1000)
  }
  node.querySelector('.ask-what').textContent = reason
  // Cleared rather than left behind: a stale warning that reappears with the
  // next prompt would be attached to the wrong command.
  risk.hidden = !reason || !session.pending_risk
  risk.textContent = reason && session.pending_risk ? `⚠ ${session.pending_risk}` : ''

  // Two counts that say how much is going on without any text changing:
  // subagents running, and tools that failed and were worked around.
  const subagents = session.subagents ?? 0
  const subagentChip = node.querySelector('.subagents')
  subagentChip.hidden = subagents < 1
  subagentChip.textContent = `${subagents} sub`
  const hiccups = session.hiccups ?? 0
  const hiccupChip = node.querySelector('.hiccups')
  hiccupChip.hidden = hiccups < 1
  hiccupChip.textContent = `${hiccups} retried`
  hiccupChip.title = `${hiccups} tool ${hiccups === 1 ? 'call' : 'calls'} failed and were worked around`

  // Status line: a coarse word plus counters. The detail that used to live here
  // moved to the expanded panel, where fast-changing text is fine.
  node.querySelector('.kind').textContent = kindLabel(state, session)
  // Counters only mean something once a turn has actually started; showing
  // "0 actions" before the first prompt is noise.
  const turnStarted = session.turn_started_ms || 0
  const actions = session.turn_tools ?? 0
  const showCounters = turnStarted > 0
  for (const part of node.querySelectorAll('.sep, .actions, .elapsed')) {
    part.hidden = !showCounters
  }
  if (showCounters) {
    node.querySelector('.actions').textContent = `${actions} ${actions === 1 ? 'action' : 'actions'}`
    node.querySelector('.elapsed').textContent = duration(turnStarted)
  }
  // The exact call, and what the turn was asked for, are both still one hover
  // away without occupying a row.
  node.title = [session.headline, session.activity].filter(Boolean).join(' — ')

  const more = node.querySelector('.more')
  more.hidden = !view.expanded
  if (view.expanded) {
    const detail = node.querySelector('.detail')
    detail.hidden = !session.detail
    detail.textContent = session.detail || ''
    node.querySelector('.log').replaceChildren(
      ...(session.recent ?? [])
        .slice()
        .reverse()
        .map((entry) => {
          const item = document.createElement('li')
          item.dataset.state = entry.state
          const when = document.createElement('span')
          when.textContent = relativeTime(entry.ms)
          const what = document.createElement('span')
          what.textContent = entry.text
          item.append(when, what)
          return item
        })
    )
  }
}

function kindLabel(state, session) {
  if (session.stalled) return 'Stopped responding'
  if (state === 'waiting') return 'Needs you'
  if (state === 'failed') return 'Failed'
  if (state === 'done') return 'Done'
  if (state === 'compacting') return 'Compacting'
  // `kind` describes work in progress, so it is only true while there is some.
  // Falling back to it once the session went quiet is how a card that had
  // finished ended up saying "Delegating" with nothing running.
  if (state === 'idle') return session.outcome === 'done' ? 'Done' : 'Idle'
  return session.kind || 'Working'
}

function buildChip(key) {
  const button = document.createElement('button')
  button.type = 'button'
  const dot = document.createElement('span')
  dot.className = 'dot'
  const text = document.createElement('span')
  text.className = 'label'
  button.append(dot, text)
  button.addEventListener('click', () => {
    stackHidden = false
    collapsed.delete(key)
    if (!slots.includes(key)) slots.unshift(key)
    render()
  })
  return button
}

function render() {
  const groups = cardsFor(sessions)
  const byKey = new Map(groups.map((group) => [group.key, group]))

  // A chat that is gone is never coming back under the same id, so anything
  // remembered about it is a leak. Keyed by session rather than by project,
  // these maps would otherwise grow for the life of the overlay.
  const known = new Set(sessions.map((session) => session.session_id))
  for (const key of [...views.keys()]) if (!known.has(key)) views.delete(key)
  for (const key of [...collapsed]) if (!known.has(key)) collapsed.delete(key)
  if (promoted && !known.has(promoted)) promoted = null

  // Slots only change when a chat starts or stops being active, or when one
  // starts needing attention. Ordinary progress never reorders anything.
  const liveKeys = groups.filter((group) => group.live).map((group) => group.key)
  slots = slots.filter((key) => liveKeys.includes(key))
  for (const key of liveKeys) if (!slots.includes(key)) slots.push(key)

  for (const group of groups) {
    const view = viewFor(group.key)
    const urgent = URGENT.has(group.state)
    if (urgent && !view.wasUrgent) {
      collapsed.delete(group.key)
      slots = [group.key, ...slots.filter((key) => key !== group.key)]
      // Something needing you outranks whatever you were reading, so it takes
      // the full card back off it.
      promoted = null
      if (group.state === 'waiting' && config.alertOnWaiting) invoke('alert').catch(() => {})
    }
    view.wasUrgent = urgent

    // Finishing while you were looking at something else is the thing this
    // whole overlay exists to tell you about. The mark outlives the card.
    // A notice is the overlay talking about itself, not a completion: it
    // neither blinks the tray nor wears an unread dot.
    if (
      group.key !== 'notice' &&
      settled(group.state) &&
      view.lastShown !== group.state &&
      isFresh(group.session)
    ) {
      view.unread = true
      invoke('flash_tray').catch(() => {})
    }
    // Work restarting answers the question the mark was asking.
    if (RUNNING.has(group.state)) view.unread = false
    view.lastShown = group.state
  }

  // The card waits for you; the pet does not. A completion is worth a little
  // celebration, not an hour of one, so once the news has gone stale the pet
  // settles back even though the card is still sitting there saying "Done".
  const leader = byKey.get(slots[0])
  const celebrating = leader && settled(leader.state) && !isFresh(leader.session)
  renderer.setState(notice || !leader || celebrating ? 'idle' : leader.state)

  // A pet that visibly dozes is doing real work: it says "nothing is running
  // and I will not interrupt you", which is different from an overlay that has
  // silently stopped receiving events. Do Not Disturb looks the same on
  // purpose, because it is the same promise.
  const busy = groups.some(
    (group) => group.live && (!settled(group.state) || isFresh(group.session))
  )
  if (busy) lastLiveAt = Date.now()
  const dozing = config.quiet || Date.now() - lastLiveAt > SLEEP_AFTER_MS
  el.pet.classList.toggle('asleep', dozing)

  // Three chats fit as cards. Past that the stack would own the screen, so it
  // collapses: one card for the chat being watched, one line each for the rest,
  // which is enough to see what every chat is doing and to close any of them.
  // More chats than that fit *because* they are lines.
  // A notice always shows, at the front, and never counts toward the density
  // flip: letting it take a fourth slot collapsed the whole stack to one-line
  // cards for six seconds and popped it back when the notice expired.
  if (byKey.has('notice')) {
    slots = ['notice', ...slots.filter((key) => key !== 'notice')]
  }
  const wanted = stackHidden ? [] : slots.filter((key) => !collapsed.has(key))
  const dense = wanted.filter((key) => key !== 'notice').length > SLOT_LIMIT
  const visible = wanted.slice(0, dense ? DENSE_LIMIT : SLOT_LIMIT)
  const detailed = dense
    ? visible.includes(promoted)
      ? promoted
      : visible[0]
    : null
  const visibleKeys = new Set(visible)

  for (const [key, node] of cards) {
    if (!visibleKeys.has(key)) {
      node.remove()
      cards.delete(key)
    }
  }

  // DOM order is bottom-up: #stack is column-reverse, so the first child sits
  // closest to the pet and the chip row ends up on top of the stack.
  let previous = null
  for (const key of visible) {
    const group = byKey.get(key)
    if (!group) continue
    let node = cards.get(key)
    if (!node) {
      node = buildCard(key)
      cards.set(key, node)
    }
    paintCard(node, group, detailed !== null && key !== detailed)
    if (previous) {
      if (previous.nextElementSibling !== node) previous.after(node)
    } else if (el.stack.firstElementChild !== node) {
      el.stack.prepend(node)
    }
    previous = node
  }

  // Live chats only: a chip for a chat that has gone idle restores nothing
  // when clicked, so it was a button whose only behavior was to vanish.
  renderChips(groups.filter((group) => !visibleKeys.has(group.key) && group.live))
  // Re-appending an element that is already last still re-inserts it, which
  // restarts any animation in its subtree on every render tick.
  if (el.stack.lastElementChild !== el.chips) el.stack.append(el.chips)
  syncHitRects()
}

/** Diffed rather than rebuilt: replacing these every second made them flicker. */
function renderChips(groups) {
  const wanted = new Set(groups.map((group) => group.key))
  for (const [key, node] of chipNodes) {
    if (!wanted.has(key)) {
      node.remove()
      chipNodes.delete(key)
    }
  }
  for (const group of groups) {
    let node = chipNodes.get(group.key)
    if (!node) {
      node = buildChip(group.key)
      chipNodes.set(group.key, node)
      el.chips.append(node)
    }
    node.querySelector('.dot').style.background = `var(--${group.state})`
    // A chat title is longer than a project name was, and a chip is a pill:
    // the row clips it and the tooltip carries the rest.
    node.lastElementChild.textContent = group.label
    node.title = group.label
  }
  el.chips.hidden = chipNodes.size === 0
}

/**
 * Report the regions that should swallow clicks. Everything else in this
 * transparent window stays click-through so the overlay never blocks the app
 * underneath it.
 */
function syncHitRects() {
  const rects = []
  const add = (node) => {
    if (!node || node.hidden) return
    const r = node.getBoundingClientRect()
    if (r.width && r.height) rects.push({ x: r.x, y: r.y, w: r.width, h: r.height })
  }
  add(el.pet)
  add(el.menu)
  add(el.panel)
  if (!el.chips.hidden) add(el.chips)
  for (const node of cards.values()) add(node)
  invoke('set_hit_rects', { rects }).catch(() => {})
}

function showNotice(message) {
  notice = message
  clearTimeout(noticeTimer)
  noticeTimer = setTimeout(() => {
    notice = null
    sessions = sessions.filter((s) => s.session_id !== 'notice')
    render()
  }, 6000)
  sessions = [
    {
      session_id: 'notice',
      project: 'Pipsqueak',
      state: 'idle',
      outcome: 'done',
      outcome_ms: Date.now(),
      settles_ms: 0,
      waiting_since: 0,
      headline: message,
      kind: 'Notice',
      activity: '',
      detail: '',
      updated_ms: Date.now(),
      turn_started_ms: Date.now(),
      turn_tools: 0,
      recent: []
    },
    ...sessions.filter((s) => s.session_id !== 'notice')
  ]
  render()
}

// --- updates -------------------------------------------------------------
const RELEASES_API = 'https://api.github.com/repos/tristanmuzzu/pipsqueak/releases/latest'
const RELEASES_PAGE = 'https://github.com/tristanmuzzu/pipsqueak/releases'
/** Never at launch, and never at the same moment on every machine. */
const FIRST_CHECK_MS = 2 * 60_000
const FIRST_CHECK_JITTER_MS = 3 * 60_000
const CHECK_EVERY_MS = 12 * 60 * 60_000

/**
 * Asks GitHub whether there is a newer release, and says so once.
 *
 * Nothing is downloaded and nothing is installed. A desktop pet that can
 * replace its own binary is a much larger promise than this one wants to make,
 * and the release page is one click away.
 */
async function checkForUpdate(manual) {
  try {
    const response = await fetch(RELEASES_API, { headers: { Accept: 'application/vnd.github+json' } })
    if (!response.ok) throw new Error(`GitHub returned ${response.status}`)
    const latest = String((await response.json()).tag_name ?? '')
    if (!isNewer(latest, APP_VERSION)) {
      // Silent unless they asked. A scheduled check that announces "nothing to
      // report" twice a day is just noise.
      if (manual) showNotice(`Pipsqueak ${APP_VERSION} is the latest version.`)
      return
    }
    if (!manual && config.updateDismissed === latest) return
    showNotice(`Pipsqueak ${latest} is available: ${RELEASES_PAGE}`)
    if (!manual) {
      // Told once. Saying it again every twelve hours is how an update prompt
      // becomes something people learn to ignore.
      config.updateDismissed = latest
      await saveConfig()
    }
  } catch (error) {
    // A scheduled check that cannot reach the network is not news.
    if (manual) showNotice(`Could not check for updates: ${error.message}`)
  }
}

let updateTimersStarted = false

/**
 * The timers always run; the setting is checked when they fire. Gating the
 * scheduling on the setting meant enabling it did nothing until the next
 * launch, and disabling it left an already-scheduled check to fire anyway.
 */
function scheduleUpdateChecks() {
  if (updateTimersStarted) return
  updateTimersStarted = true
  const first = FIRST_CHECK_MS + Math.random() * FIRST_CHECK_JITTER_MS
  setTimeout(() => {
    if (config.updateCheck) checkForUpdate(false)
    setInterval(() => {
      if (config.updateCheck) checkForUpdate(false)
    }, CHECK_EVERY_MS)
  }, first)
}

// --- welcome and setup check --------------------------------------------
/**
 * How long the connection test watches for hook traffic.
 *
 * Long enough to type something into Claude Code and hit enter, short enough
 * that nobody wanders off mid-test.
 */
const WATCH_MS = 10_000

function openPanel(title, build) {
  el.panelTitle.textContent = title
  el.panelBody.replaceChildren(...build())
  el.panel.hidden = false
  el.menu.hidden = true
  syncHitRects()
}

/** Timers owned by the doctor's connection test. Without this they outlived
 * the panel, mutating detached nodes and stacking overlapping tests. */
let doctorTimers = []
function clearDoctorTimers() {
  for (const id of doctorTimers) {
    clearInterval(id)
    clearTimeout(id)
  }
  doctorTimers = []
}

function closePanel() {
  clearDoctorTimers()
  el.panel.hidden = true
  syncHitRects()
}

function para(text, className) {
  const node = document.createElement('p')
  node.textContent = text
  if (className) node.className = className
  return node
}

function action(label, onClick, primary) {
  const node = document.createElement('button')
  node.type = 'button'
  node.className = primary ? 'action primary' : 'action'
  node.textContent = label
  node.addEventListener('click', () => onClick(node))
  return node
}

/**
 * Shown once, on the first run that has not been acknowledged.
 *
 * The hooks are the whole product and they do not install themselves, so
 * historically the first thing a new user had to do was find a tray menu and
 * guess what "install hooks" meant. This says what it edits, in the same
 * breath as asking to do it.
 */
/**
 * Tells the welcome panel which chord actually registered.
 *
 * Filled in after the panel is built, because asking the backend is async and
 * a panel that pops up half a frame late is worse than a line that arrives
 * half a frame late.
 */
function hotkeyLine() {
  const node = para('Checking the keyboard shortcut…')
  invoke('hotkey_binding')
    .then((chord) => {
      node.textContent = chord
        ? `Press ${chord} any time to show or hide the pet.`
        : 'No keyboard shortcut was available; set "hotkey" in ~/.pipsqueak/config.json.'
    })
    .catch(() => {
      node.textContent = ''
    })
  return node
}

function showWelcome() {
  openPanel('Pipsqueak', () => {
    const nodes = [
      para('This shows what Claude Code is doing, per project, while you get on with something else.'),
      para(
        'It needs to register hooks in ~/.claude/settings.json. That file is backed up first, and only entries Pipsqueak added are ever removed.',
        'tight'
      ),
      action('Install Claude Code hooks', async (button) => {
        button.disabled = true
        button.textContent = 'Installing…'
        const message = await invoke('install_hooks').catch((e) => String(e))
        button.textContent = message
      }, true),
      action('Start Pipsqueak with Windows', async (button) => {
        const enabled = await invoke('autostart_enabled').catch(() => false)
        const result = await invoke('set_autostart', { enabled: !enabled }).catch((e) => String(e))
        button.textContent =
          typeof result === 'string' ? result : enabled ? 'Will not start with Windows' : 'Will start with Windows'
      }),
      action('Check GitHub for updates occasionally', async (button) => {
        config.updateCheck = !config.updateCheck
        await saveConfig()
        button.textContent = config.updateCheck
          ? 'Will check for updates. Nothing is ever downloaded'
          : 'Will not check for updates'
      }),
      para('Then start a session. Restart Claude Code first, because it reads its hooks at startup.'),
      hotkeyLine(),
      action('Done', () => {
        closePanel()
      })
    ]
    return nodes
  })
}

/**
 * The setup check.
 *
 * The static checks answer "is it configured"; the connection test answers "is
 * it working", which is a different question and the only one worth asking
 * when someone says nothing is happening.
 */
async function showDoctor() {
  clearDoctorTimers()
  const report = await invoke('run_doctor').catch(() => null)
  openPanel('Setup check', () => {
    if (!report) return [para('Could not run the check.')]
    const nodes = []
    for (const check of report.checks) {
      const row = document.createElement('div')
      row.className = 'check'
      row.dataset.status = check.status
      const dot = document.createElement('span')
      dot.className = 'dot'
      const text = document.createElement('span')
      text.className = 'check-text'
      const label = document.createElement('strong')
      label.className = 'check-label'
      label.textContent = check.label
      const detail = document.createElement('span')
      detail.className = 'check-detail'
      detail.textContent = check.detail
      text.append(label, detail)
      row.append(dot, text)
      if (check.fix === 'install') {
        const fix = document.createElement('button')
        fix.type = 'button'
        fix.className = 'fix'
        fix.textContent = 'Fix'
        fix.addEventListener('click', async () => {
          fix.disabled = true
          await invoke('install_hooks').catch(() => {})
          showDoctor()
        })
        row.append(fix)
      }
      nodes.push(row)
    }

    const status = para('')
    nodes.push(
      action('Test the connection', async (button) => {
        button.disabled = true
        const since = await invoke('watch_start').catch(() => Date.now())
        const deadline = Date.now() + WATCH_MS
        status.textContent = 'Go and run anything in Claude Code now…'
        const tick = setInterval(() => {
          const left = Math.ceil((deadline - Date.now()) / 1000)
          if (left > 0) status.textContent = `Go and run anything in Claude Code now… ${left}s`
        }, 250)
        const finish = setTimeout(async () => {
          clearInterval(tick)
          const [, detail] = await invoke('watch_result', { since }).catch(() => ['none', 'Check failed.'])
          status.textContent = detail
          button.disabled = false
          button.textContent = 'Test again'
        }, WATCH_MS)
        doctorTimers.push(tick, finish)
      }),
      status,
      action('Copy report', async (button) => {
        const text = await invoke('doctor_report').catch(() => '')
        try {
          await navigator.clipboard.writeText(text)
          button.textContent = 'Copied. Paste it into an issue'
        } catch {
          button.textContent = 'Could not reach the clipboard'
        }
      })
    )
    return nodes
  })
}

// --- context menu -------------------------------------------------------
/**
 * The right-click menu.
 *
 * Everything used more than once a week is on it; everything used once, when
 * the app is first set up, is behind "More". A menu with twenty items in it is
 * one nobody reads, and it had grown tall enough to run off the top of the
 * screen.
 */
let menuExpanded = false

async function openMenu() {
  const pets = await invoke('list_pets').catch(() => [])
  const installed = await invoke('hooks_installed').catch(() => false)
  const autostart = await invoke('autostart_enabled').catch(() => false)
  const chord = await invoke('hotkey_binding').catch(() => '')
  const children = []

  /** A click closes the menu unless it changed the menu itself. */
  const button = (text, onClick, pressed, keepOpen = false) => {
    const node = document.createElement('button')
    node.type = 'button'
    node.textContent = text
    if (pressed !== undefined) node.setAttribute('aria-pressed', String(pressed))
    node.addEventListener('click', async () => {
      if (!keepOpen) el.menu.hidden = true
      await onClick()
      render()
    })
    return node
  }
  const row = (buttons) => {
    const node = document.createElement('div')
    node.className = 'row'
    node.append(...buttons)
    return node
  }
  const rule = () => {
    const node = document.createElement('div')
    node.className = 'rule'
    return node
  }

  // Pets and sizes are picks from a short list, so they are chips on one line
  // rather than six full-width rows.
  children.push(
    row(
      pets.map((pet) =>
        button(pet.displayName, () => selectPet(pet.id), pet.id === config.pet, true)
      )
    )
  )
  children.push(
    row(
      [['S', 1.5], ['M', 2], ['L', 3]].map(([label, value]) =>
        button(
          label,
          async () => {
            config.scale = value
            renderer.setScale(value)
            await saveConfig()
          },
          config.scale === value,
          true
        )
      )
    )
  )
  children.push(rule())

  const narration = {
    off: 'Saying nothing while working',
    speech: 'Saying what Claude tells you',
    thoughts: 'Saying what Claude is thinking'
  }
  children.push(
    button(
      narration[config.narrate] ?? narration.thoughts,
      async () => {
        const order = ['off', 'speech', 'thoughts']
        config.narrate = order[(order.indexOf(config.narrate) + 1) % order.length]
        await saveConfig()
      },
      config.narrate !== 'off',
      true
    )
  )
  children.push(
    button(
      'Do not disturb',
      async () => {
        config.quiet = !config.quiet
        if (config.quiet) invoke('clear_attention').catch(() => {})
        await saveConfig()
      },
      config.quiet
    )
  )
  children.push(
    button(stackHidden ? 'Show the cards' : 'Hide the cards', () => {
      stackHidden = !stackHidden
      collapsed.clear()
    })
  )

  children.push(rule())
  children.push(
    button(menuExpanded ? 'Less' : 'More…', () => {
      menuExpanded = !menuExpanded
      openMenu()
    })
  )

  if (menuExpanded) {
    children.push(
      button(
        'Include scratch/temp sessions',
        async () => {
          config.showScratch = !config.showScratch
          await saveConfig()
        },
        config.showScratch
      )
    )
    children.push(
      button(
        'Sound when a project needs you',
        async () => {
          config.alertOnWaiting = !config.alertOnWaiting
          await saveConfig()
        },
        config.alertOnWaiting
      )
    )
    children.push(
      button(
        'Blink the tray when a project finishes',
        async () => {
          config.flashOnFinish = !config.flashOnFinish
          await saveConfig()
        },
        config.flashOnFinish
      )
    )
    children.push(
      button(
        'Click through the pet',
        async () => {
          config.clickThrough = !config.clickThrough
          await saveConfig()
        },
        config.clickThrough
      )
    )
    children.push(rule())
    // The chord shown is the one that registered, which is not always the one
    // that was asked for: another program may already own it.
    children.push(
      button(chord ? `Show or hide with ${chord}` : 'No global hotkey available', () =>
        showNotice(
          chord
            ? `${chord} shows and hides the pet. Change it with "hotkey" in ~/.pipsqueak/config.json.`
            : 'Every candidate hotkey is already taken. Set "hotkey" in ~/.pipsqueak/config.json to a free one.'
        )
      )
    )
    children.push(button('Check my setup…', () => showDoctor()))
    children.push(
      button(
        'Check for updates automatically',
        async () => {
          config.updateCheck = !config.updateCheck
          await saveConfig()
          if (config.updateCheck) checkForUpdate(true)
        },
        config.updateCheck
      )
    )
    children.push(
      button(
        'Start with Windows',
        async () => {
          const result = await invoke('set_autostart', { enabled: !autostart }).catch((e) =>
            String(e)
          )
          if (typeof result === 'string') showNotice(result)
        },
        autostart
      )
    )
    children.push(
      button(installed ? 'Reinstall Claude Code hooks' : 'Install Claude Code hooks', async () => {
        const message = await invoke('install_hooks').catch((e) => String(e))
        showNotice(message)
      })
    )
    children.push(button('Open pets folder', () => invoke('open_pets_dir').catch(() => {})))
  }

  children.push(rule())
  // Two different doors, and it was worth making the difference obvious. The
  // hotkey and the tray icon belong to this process, so quitting takes them
  // with it: nothing on the keyboard brings the pet back afterwards.
  children.push(
    button(chord ? `Hide the pet (${chord})` : 'Hide the pet', () => invoke('hide_window'))
  )
  const quit = button('Quit Pipsqueak', () => invoke('quit'))
  quit.title = chord
    ? `Stops Pipsqueak. ${chord} will not bring it back; start it again from the Start menu.`
    : 'Stops Pipsqueak. Start it again from the Start menu.'
  children.push(quit)

  el.menu.replaceChildren(...children)
  el.menu.hidden = false
  syncHitRects()
}

async function selectPet(id) {
  try {
    const payload = await loadPetPayload(id)
    await renderer.load(id, payload)
    renderer.setScale(config.scale)
    config.pet = id
    await saveConfig()
  } catch (error) {
    showNotice(String(error))
  }
}

async function saveConfig() {
  await invoke('set_config', {
    config: {
      pet: config.pet,
      scale: config.scale,
      click_through: config.clickThrough,
      show_bubble: config.showBubble,
      show_scratch: config.showScratch,
      alert_on_waiting: config.alertOnWaiting,
      flash_on_finish: config.flashOnFinish,
      quiet: config.quiet,
      narrate: config.narrate,
      welcomed: config.welcomed,
      update_check: config.updateCheck,
      update_dismissed: config.updateDismissed,
      x: config.x ?? null,
      y: config.y ?? null
    }
  }).catch(() => {})
}

// --- interaction --------------------------------------------------------
function wireInteraction() {
  let origin = null
  let dragging = false

  el.pet.addEventListener('pointerdown', (event) => {
    if (event.button !== 0) return
    origin = { x: event.clientX, y: event.clientY }
    dragging = false
  })

  el.pet.addEventListener('pointermove', (event) => {
    if (!origin || dragging) return
    if (Math.hypot(event.clientX - origin.x, event.clientY - origin.y) > 3) {
      dragging = true
      appWindow?.startDragging().catch(() => {})
    }
  })

  el.pet.addEventListener('pointerup', () => {
    if (origin && !dragging) {
      // A click outside the menu can't reach us, because the window is
      // click-through there, so the pet itself is what dismisses it.
      if (!el.menu.hidden) el.menu.hidden = true
      else stackHidden = !stackHidden
      // Looking at the pet is looking at the pet.
      invoke('clear_attention').catch(() => {})
      render()
    }
    origin = null
  })

  // Poking it should do something. Cheap, and the first thing anyone tries.
  el.pet.addEventListener('pointerenter', () => {
    renderer.playOnce(JUMP_ROW)
    // Long enough that reaching past the pet for something else never brings
    // it up. It is only useful to someone who has stopped and is wondering.
    clearTimeout(hintTimer)
    // Not added to the hit rects on purpose: it cannot be clicked, so the
    // window stays click-through underneath it.
    hintTimer = setTimeout(() => {
      el.hint.hidden = false
    }, HINT_DELAY_MS)
  })

  const hideHint = () => {
    clearTimeout(hintTimer)
    el.hint.hidden = true
  }
  el.pet.addEventListener('pointerleave', hideHint)
  el.pet.addEventListener('pointerdown', hideHint)

  el.pet.addEventListener('contextmenu', (event) => {
    event.preventDefault()
    if (el.menu.hidden) openMenu()
    else {
      el.menu.hidden = true
      syncHitRects()
    }
  })

  document.addEventListener('contextmenu', (event) => event.preventDefault())

  if (!appWindow) return
  appWindow.onMoved(({ payload }) => {
    config.x = payload.x
    config.y = payload.y
    invoke('save_position', { x: payload.x, y: payload.y }).catch(() => {})
  })
}

// --- boot ---------------------------------------------------------------
async function boot() {
  const stored = await invoke('get_config').catch(() => null)
  if (stored) {
    config = {
      pet: stored.pet ?? 'byte',
      scale: stored.scale ?? 2,
      clickThrough: Boolean(stored.click_through),
      showBubble: stored.show_bubble !== false,
      showScratch: Boolean(stored.show_scratch),
      alertOnWaiting: Boolean(stored.alert_on_waiting),
      flashOnFinish: stored.flash_on_finish !== false,
      quiet: Boolean(stored.quiet),
      narrate: stored.narrate || 'thoughts',
      welcomed: Boolean(stored.welcomed),
      updateCheck: Boolean(stored.update_check),
      updateDismissed: stored.update_dismissed ?? '',
      x: stored.x,
      y: stored.y
    }
  }

  try {
    await renderer.load(config.pet, await loadPetPayload(config.pet))
  } catch {
    config.pet = 'pip'
    await renderer.load('pip', null)
  }
  renderer.setScale(config.scale)
  renderer.start()

  wireInteraction()
  el.panelClose.addEventListener('click', closePanel)

  if (!IS_TAURI) {
    startBrowserDemo()
    return
  }

  // Any dismissal counts as acknowledged: Done, the close button, or simply
  // never opening it again. A welcome that keeps coming back is worse than one
  // that is missed.
  if (!config.welcomed) {
    config.welcomed = true
    await saveConfig()
    // An existing install has no `welcomed` flag either, so it would get the
    // panel on its first update. Offering to install hooks that are already
    // registered reads as "something is wrong with your setup", and it
    // interrupts whatever you were doing to say it. If the hooks are there,
    // the only thing worth mentioning is the new shortcut, and a notice card
    // says that without taking over the screen.
    if (await invoke('hooks_installed').catch(() => false)) {
      const chord = await invoke('hotkey_binding').catch(() => '')
      showNotice(
        chord
          ? `Updated to ${APP_VERSION}. Press ${chord} to show or hide the pet.`
          : `Updated to ${APP_VERSION}.`
      )
    } else {
      showWelcome()
    }
  }

  sessions = await invoke('get_sessions').catch(() => [])
  sessions.forEach((session) => {
    seenSessions.add(session.session_id)
    // Whatever finished before the overlay started is history, not news. A
    // completion still inside its news window is shown, so restarting the pet
    // in the middle of a turn does not swallow the answer.
    const stale = Date.now() - (session.outcome_ms || 0) > DONE_LINGER_MS
    if (session.outcome && stale) acknowledged.add(outcomeKey(session))
  })

  await listen('pipsqueak://sessions', (event) => {
    const incoming = event.payload ?? []
    for (const session of incoming) {
      if (!seenSessions.has(session.session_id)) {
        seenSessions.add(session.session_id)
        renderer.playOnce(GREETING_ROW)
      }
    }
    const live = new Set(incoming.map((s) => s.session_id))
    seenSessions = new Set([...seenSessions].filter((id) => live.has(id)))
    // A session that is gone can never show its card again, so remembering
    // that it was acknowledged is just a leak.
    for (const key of acknowledged) {
      if (!live.has(key.slice(0, key.lastIndexOf('@')))) acknowledged.delete(key)
    }
    const kept = notice ? sessions.filter((s) => s.session_id === 'notice') : []
    sessions = [...kept, ...incoming]
    render()
  })

  await listen('pipsqueak://notice', (event) => showNotice(String(event.payload)))

  // The cursor, in this window's own coordinates, while it is somewhere near.
  // Pets drawn with the two look rows turn to face it; the rest never hear
  // about it, because the renderer drops it.
  await listen('pipsqueak://cursor', (event) => {
    const at = event.payload
    if (!Array.isArray(at)) {
      renderer.lookAt(null, null)
      return
    }
    const rect = el.pet.getBoundingClientRect()
    renderer.lookAt(at[0] - (rect.x + rect.width / 2), at[1] - (rect.y + rect.height / 2))
  })

  // `pipsqueak control <pet>` changes the config from outside the window.
  await listen('pipsqueak://pet', async (event) => {
    const id = String(event.payload)
    try {
      await renderer.load(id, await loadPetPayload(id))
      renderer.setScale(config.scale)
      config.pet = id
      await saveConfig()
    } catch (error) {
      showNotice(String(error))
    }
  })

  // The backend asking whether the page is alive. Timers are throttled hard
  // for an occluded window, so the render loop can go quiet without the page
  // being broken; event delivery is not throttled, so answer directly and let
  // the watchdog stand down instead of reloading a healthy page.
  await listen('pipsqueak://ping', () => {
    invoke('frontend_pong').catch(() => {})
  })

  // A config change made from the tray. Merge only the fields the tray owns:
  // taking the whole object would clobber an in-flight change on this side,
  // and ignoring it meant the next save here silently reverted the tray's.
  await listen('pipsqueak://config', (event) => {
    const stored = event.payload
    if (!stored || typeof stored !== 'object') return
    config.clickThrough = Boolean(stored.click_through)
    config.quiet = Boolean(stored.quiet)
    render()
  })

  // Listeners are attached: anything emitted before this moment was lost, so
  // ask the poller to send the current state again.
  await invoke('frontend_ready').catch(() => {})
  render()

  scheduleUpdateChecks()

  // Ages and elapsed timers tick even when no event arrives.
  setInterval(render, 1000)
}

/** Built-in pets ship inside the frontend bundle; the rest come from disk. */
async function loadPetPayload(id) {
  if (id === 'byte' || id === 'ember' || id === 'pip') return null
  return invoke('load_pet', { id })
}

/** What each demo chat is "saying", so the main line has real sentences. */
const DEMO_LINES = {
  clockwork: [
    'Reading the formatter to see which clock it trusts.',
    'It parses in local time and compares in UTC — that is the bug.',
    'Rewriting the assertion to fix the timezone at the boundary.',
    'Both suites pass; nothing left to check.'
  ],
  orchestrator: [
    'Adding the twelve tier C scenarios to the eval set.',
    'The runner times out at eight; raising the budget.',
    'Added 12 scenarios; suite passes in 41s.'
  ],
  ledger: [
    'Checking how finance rounds a half cent.',
    'Half-up, and only at the invoice total.',
    'Rewriting the totals to round once, at the end.'
  ],
  atlas: [
    'Tiles are re-fetched every hour for no reason.',
    'Setting a week of cache with a content hash in the path.'
  ],
  migration: [
    'Writing the backfill for the invoices already issued.',
    'Batching it at a thousand rows so the table stays writable.'
  ]
}

/** Cycles states in a browser tab so the UI can be designed without a build. */
function startBrowserDemo() {
  // The desktop app supplies the chat title in a real session; here it is
  // written down so the card can be designed with the line it will actually
  // have to fit.
  const titles = {
    clockwork: 'Timezone test flakiness',
    orchestrator: 'Tier C eval coverage',
    ledger: 'Invoice rounding rules',
    atlas: 'Tile server cache headers',
    migration: 'Backfill for issued invoices'
  }
  // Two chats on the same repository, because that is the case the card layout
  // has to survive: they get a card each, and the project name on both.
  const projects = { migration: 'ledger' }
  // Five chats rather than two: three is where the stack collapses, and a
  // layout whose rule you cannot see is a layout you cannot design.
  const scripts = {
    clockwork: [
      ['thinking', 'Thinking', 'Fix the flaky timezone test'],
      ['running', 'Reading', 'Fix the flaky timezone test'],
      ['running', 'Editing', 'Fix the flaky timezone test'],
      ['running', 'Running', 'Fix the flaky timezone test'],
      ['waiting', 'Needs you', 'Fix the flaky timezone test'],
      ['done', 'Done', 'Fixed: the formatter used local time.']
    ],
    orchestrator: [
      ['running', 'Editing', 'Add tier C eval scenarios'],
      ['running', 'Running', 'Add tier C eval scenarios'],
      ['failed', 'Failed', 'Add tier C eval scenarios'],
      ['done', 'Done', 'Added 12 scenarios; suite passes in 41s.']
    ],
    ledger: [
      ['running', 'Reading', 'Round invoice totals the way finance does'],
      ['running', 'Editing', 'Round invoice totals the way finance does'],
      ['running', 'Running', 'Round invoice totals the way finance does']
    ],
    atlas: [
      ['running', 'Running', 'Cache tiles for a week, not an hour'],
      ['thinking', 'Thinking', 'Cache tiles for a week, not an hour'],
      ['running', 'Editing', 'Cache tiles for a week, not an hour']
    ],
    migration: [
      ['running', 'Editing', 'Backfill the invoices already issued'],
      ['running', 'Running', 'Backfill the invoices already issued'],
      ['thinking', 'Thinking', 'Backfill the invoices already issued']
    ]
  }
  // In the app the cursor arrives from the window manager, because the overlay
  // is click-through and never sees a pointer event it is not under. In a
  // browser tab the pointer is all there is, so the look poses can still be
  // designed without a build.
  document.addEventListener('pointermove', (event) => {
    const rect = el.pet.getBoundingClientRect()
    renderer.lookAt(event.clientX - (rect.x + rect.width / 2), event.clientY - (rect.y + rect.height / 2))
  })
  // The renderer, for poking at from the console while designing. Only ever in
  // a browser tab: the app takes this branch never.
  window.pet = renderer

  let tick = 0
  const started = Date.now() - 143_000
  const advance = () => {
    const now = Date.now()
    sessions = Object.entries(scripts).map(([chat, steps], i) => {
      const [want, kind, headline] = steps[(tick + i) % steps.length]
      // The demo speaks in display states; the real files speak in the three
      // separate fields, so translate rather than special-case the renderer.
      const outcome = want === 'done' || want === 'failed' ? want : ''
      let durable = want
      if (outcome) durable = 'idle'
      // Being blocked does not change what the session was doing.
      else if (want === 'waiting') durable = 'running'
      return {
        session_id: chat,
        project: projects[chat] ?? chat,
        chat_title: titles[chat] ?? '',
        workspace: i === 1 ? 'feature-x' : '',
        scratch: false,
        state: durable,
        outcome,
        outcome_ms: outcome ? now : 0,
        settles_ms: 0,
        waiting_since: 0,
        waiting_reason: '',
        // The demo's one blocked step is a risky command, so the ask row and
        // its warning can be designed without a real session.
        pending_since: want === 'waiting' ? now - WAITING_DEBOUNCE_MS : 0,
        pending_tool: want === 'waiting' ? 'Bash' : '',
        pending_detail: want === 'waiting' ? 'run: git push --force origin main' : '',
        pending_risk: want === 'waiting' ? 'Force-pushes over remote history' : '',
        hiccups: i === 1 ? 2 : 0,
        subagents: want === 'running' && i === 0 ? 3 : 0,
        kind,
        headline,
        // In a real session this is whatever Claude last said or thought; the
        // demo needs one so the line can be designed at its real length.
        narration: DEMO_LINES[chat]?.[tick % DEMO_LINES[chat].length] ?? '',
        activity: `${kind} something`,
        detail: want === 'failed' ? 'expected 03:00 to be 02:00' : '',
        updated_ms: now,
        started_ms: started,
        turn_started_ms: started,
        turn_tools: 12 + tick * 3,
        recent: [{ ms: now, state: want, text: `${kind} something` }]
      }
    })
    tick += 1
    render()
  }
  advance()
  setInterval(advance, 2600)
  setInterval(render, 1000)

  // `?panel=welcome` / `?panel=doctor` so both can be designed in a browser
  // without a build, a real install, or a broken machine to point them at.
  const wanted = new URLSearchParams(location.search).get('panel')
  if (wanted === 'welcome') showWelcome()
  if (wanted === 'doctor') {
    openPanel('Setup check', () => [
      para('Rendering with sample results. The real check needs the app.'),
      ...[
        ['ok', 'Claude Code hooks', 'All 15 events registered.'],
        ['fail', 'Hook program', 'The hooks point at a copy that no longer exists.'],
        ['ok', 'Session folder', '~/.pipsqueak/sessions is writable.'],
        ['warn', 'Recent activity', 'No sessions recorded yet.']
      ].map(([status, label, detail]) => {
        const row = document.createElement('div')
        row.className = 'check'
        row.dataset.status = status
        const dot = document.createElement('span')
        dot.className = 'dot'
        const text = document.createElement('span')
        text.className = 'check-text'
        const strong = document.createElement('strong')
        strong.className = 'check-label'
        strong.textContent = label
        const span = document.createElement('span')
        span.className = 'check-detail'
        span.textContent = detail
        text.append(strong, span)
        row.append(dot, text)
        return row
      }),
      action('Test the connection', () => {}),
      action('Copy report', () => {})
    ])
  }
}

boot()
