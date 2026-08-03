import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { PetRenderer, GREETING_ROW } from './pet.js'

/** `npm run dev` in a plain browser has no IPC; fall back to a demo loop. */
const IS_TAURI = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

const SLOT_LIMIT = 3
/**
 * A turn can enter and leave `waiting` in a few hundred milliseconds when a
 * hook or auto-mode answers the permission prompt. Only a state that persists
 * is worth telling anyone about.
 */
const WAITING_DEBOUNCE_MS = 800
/** How long a finished project keeps its card before collapsing to a chip. */
const DONE_LINGER_MS = 30_000
/** A failed turn is something you need to see, so it waits around longer. */
const FAILED_LINGER_MS = 5 * 60_000
/** Quiet for this long and the pet visibly dozes off. */
const SLEEP_AFTER_MS = 60_000
const URGENT = new Set(['waiting', 'failed'])
const ACTIVE = new Set(['thinking', 'running', 'waiting', 'failed', 'compacting', 'done'])
/** The durable states a session file can report. */
const RUNNING = new Set(['thinking', 'running', 'compacting'])

/**
 * Which state wins when a project has several, and how long each one holds the
 * card once shown.
 *
 * The hold is what makes the stack readable. Hook events arrive in bursts, and
 * without a floor the card can flick through three states faster than you can
 * read one — and "needs you" can vanish under a later "running" before you
 * ever look up.
 */
const DISPLAY = {
  waiting: { rank: 5, hold: 2500 },
  failed: { rank: 4, hold: 4000 },
  done: { rank: 3, hold: 2500 },
  compacting: { rank: 2, hold: 1500 },
  running: { rank: 2, hold: 900 },
  thinking: { rank: 2, hold: 900 },
  idle: { rank: 0, hold: 0 }
}

const priority = (state) => DISPLAY[state]?.rank ?? 1

const el = {
  stack: document.getElementById('stack'),
  chips: document.getElementById('chips'),
  menu: document.getElementById('menu'),
  pet: document.getElementById('pet'),
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
  quiet: false
}
let sessions = []
/** When any project was last doing something, for the doze. */
let lastLiveAt = Date.now()
let notice = null
let noticeTimer = null
let stackHidden = false
let seenSessions = new Set()

/**
 * Card order is a property of the UI, not of the data. Sorting by "most
 * recently updated" on every hook event made the visible set churn every few
 * hundred milliseconds. A project keeps its slot until it goes quiet.
 */
let slots = []
const views = new Map()
const collapsed = new Set()
const cards = new Map()
const chipNodes = new Map()

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

/**
 * One card per project. Several sessions can share a project (worktrees,
 * subagents); the busiest one speaks for it.
 */
function groupByProject(list) {
  const groups = []
  const index = new Map()
  for (const session of list) {
    if (session.scratch && !config.showScratch) continue
    const key = session.project || session.session_id.slice(0, 8)
    const existing = index.get(key)
    if (existing) {
      existing.count += 1
      if (rank(session) > rank(existing.session)) existing.session = session
      continue
    }
    const group = { key, session, count: 1 }
    index.set(key, group)
    groups.push(group)
  }
  for (const group of groups) {
    group.state = effectiveState(group.key, group.session)
    group.live = ACTIVE.has(group.state)
  }
  return groups
}

/** Which of a project's sessions gets to speak for it. */
function rank(session) {
  if (session.waiting_since || session.pending_since) return 4
  if (session.outcome === 'failed') return 3
  if (RUNNING.has(session.state)) return 2
  if (session.outcome === 'done') return 1
  return 0
}

/**
 * What the session is blocked on, once it has been blocked long enough to be
 * worth saying. Empty while the prompt is still inside the debounce.
 */
function blockedOn(session) {
  const now = Date.now()
  if (session.waiting_since && now - session.waiting_since >= WAITING_DEBOUNCE_MS) {
    return session.waiting_reason || 'Waiting for you'
  }
  // A permission prompt Claude Code raised and nothing has resolved. Most are
  // answered by auto-mode within a few hundred milliseconds and never reach
  // a human, which is exactly what the debounce is filtering out.
  if (session.pending_since && now - session.pending_since >= WAITING_DEBOUNCE_MS) {
    return session.pending_detail || `Permission for ${session.pending_tool}`
  }
  return ''
}

/**
 * Turn a session file into the one word the card shows.
 *
 * The file answers three separate questions — what is it doing, how did the
 * last turn end, is a human blocking it — and this is where they collapse into
 * a single state, in that order of urgency.
 */
function displayState(session) {
  const now = Date.now()

  if (blockedOn(session)) return 'waiting'
  // Inside the debounce, hold whatever was on screen rather than committing to
  // a "needs you" that auto-mode is about to answer.
  if (session.waiting_since || session.pending_since) return ''

  const outcome = session.outcome || ''
  if (outcome === 'done') {
    // Claude Code sends Stop while background work is still finishing. Until
    // the result settles it is still a running turn.
    if (now < (session.settles_ms || 0)) return session.state || 'running'
    return now - (session.outcome_ms || 0) > DONE_LINGER_MS ? 'idle' : 'done'
  }
  if (outcome === 'failed') {
    return now - (session.outcome_ms || 0) > FAILED_LINGER_MS ? 'idle' : 'failed'
  }
  return session.state || 'idle'
}

function effectiveState(key, session) {
  const view = viewFor(key)
  const now = Date.now()

  const raw = displayState(session) || view.lastStable
  view.lastStable = RUNNING.has(raw) || raw === 'idle' ? raw : view.lastStable

  // The hold exists to stop the colour flickering between two states that are
  // really the same moment. It must not survive the card's *text* changing:
  // holding "failed" over a headline that already says the turn succeeded is a
  // worse lie than the flicker it was preventing.
  const subject = session.headline || ''
  if (view.heldSubject !== subject) {
    view.heldSubject = subject
    view.heldState = ''
    view.heldUntil = 0
  }

  // Otherwise a state that has not been up long enough only gives way to
  // something more urgent than itself.
  if (view.heldState && now < view.heldUntil && priority(raw) <= priority(view.heldState)) {
    return view.heldState
  }
  if (raw !== view.heldState) {
    view.heldState = raw
    view.heldUntil = now + (DISPLAY[raw]?.hold ?? 0)
  }
  return raw
}

function relativeTime(ms) {
  if (!ms) return ''
  const delta = Math.max(0, Date.now() - ms)
  if (delta < 1000) return 'now'
  if (delta < 60_000) return `${Math.floor(delta / 1000)}s`
  if (delta < 3_600_000) return `${Math.floor(delta / 60_000)}m`
  return `${Math.floor(delta / 3_600_000)}h`
}

function duration(ms) {
  if (!ms) return '0s'
  const seconds = Math.max(0, Math.floor((Date.now() - ms) / 1000))
  if (seconds < 60) return `${seconds}s`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  return `${Math.floor(seconds / 3600)}h`
}

// --- rendering ----------------------------------------------------------
function buildCard(key) {
  const node = el.template.content.firstElementChild.cloneNode(true)
  node.querySelector('.close').addEventListener('click', (event) => {
    event.stopPropagation()
    collapsed.add(key)
    render()
  })
  node.querySelector('.focus').addEventListener('click', (event) => {
    event.stopPropagation()
    const group = groupByProject(sessions).find((candidate) => candidate.key === key)
    invoke('focus_project', {
      project: key,
      workspace: group?.session.workspace ?? ''
    }).catch(() => {})
  })
  node.addEventListener('click', () => {
    const view = viewFor(key)
    view.expanded = !view.expanded
    // Clicking a card is the one unambiguous signal that you have seen it.
    view.unread = false
    invoke('clear_attention').catch(() => {})
    render()
  })
  return node
}

function paintCard(node, group) {
  const { session, key, count, state } = group

  node.dataset.state = state
  node.querySelector('.project').textContent = key
  node.querySelector('.age').textContent = relativeTime(session.updated_ms)

  const workspace = node.querySelector('.workspace')
  workspace.hidden = !session.workspace
  workspace.textContent = session.workspace || ''

  const countNode = node.querySelector('.count')
  countNode.hidden = count < 2
  countNode.textContent = `${count}×`

  const view = viewFor(key)
  node.querySelector('.unread').hidden = !view.unread

  node.querySelector('.headline').textContent =
    session.headline || session.activity || 'Working…'

  // What it is blocked on, in its own row. This is the only text on the card
  // that is worth interrupting something else to read.
  const ask = node.querySelector('.ask')
  const reason = state === 'waiting' ? blockedOn(session) : ''
  const risk = node.querySelector('.risk')
  ask.hidden = !reason
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
  // The exact call is still one hover away, without occupying a row.
  node.title = session.activity || ''

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
  if (state === 'waiting') return 'Needs you'
  if (state === 'failed') return 'Failed'
  if (state === 'done') return 'Done'
  if (state === 'compacting') return 'Compacting'
  return session.kind || 'Working'
}

function buildChip(key) {
  const button = document.createElement('button')
  button.type = 'button'
  const dot = document.createElement('span')
  dot.className = 'dot'
  const text = document.createElement('span')
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
  const groups = groupByProject(sessions)
  const byKey = new Map(groups.map((group) => [group.key, group]))

  // Slots only change when a project starts or stops being active, or when one
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
      if (group.state === 'waiting' && config.alertOnWaiting) invoke('alert').catch(() => {})
    }
    view.wasUrgent = urgent

    // Finishing while you were looking at something else is the thing this
    // whole overlay exists to tell you about, and a 30s linger is no use if
    // the 30s happened during a video. The mark outlives the card.
    const settled = group.state === 'done' || group.state === 'failed'
    const fresh = Date.now() - (group.session.outcome_ms || 0) < DONE_LINGER_MS
    if (settled && view.lastShown !== group.state && fresh) {
      view.unread = true
      invoke('flash_tray').catch(() => {})
    }
    // Work restarting answers the question the mark was asking.
    if (RUNNING.has(group.state)) view.unread = false
    view.lastShown = group.state
  }

  const leader = byKey.get(slots[0])
  renderer.setState(notice ? 'idle' : leader ? leader.state : 'idle')

  // A pet that visibly dozes is doing real work: it says "nothing is running
  // and I will not interrupt you", which is different from an overlay that has
  // silently stopped receiving events. Do Not Disturb looks the same on
  // purpose — it is the same promise.
  if (groups.some((group) => group.live)) lastLiveAt = Date.now()
  const dozing = config.quiet || Date.now() - lastLiveAt > SLEEP_AFTER_MS
  el.pet.classList.toggle('asleep', dozing)

  const visible = stackHidden
    ? []
    : slots.filter((key) => !collapsed.has(key)).slice(0, SLOT_LIMIT)
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
    paintCard(node, group)
    if (previous) {
      if (previous.nextElementSibling !== node) previous.after(node)
    } else if (el.stack.firstElementChild !== node) {
      el.stack.prepend(node)
    }
    previous = node
  }

  renderChips(groups.filter((group) => !visibleKeys.has(group.key) && (group.live || collapsed.has(group.key))))
  el.stack.append(el.chips)
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
    node.lastElementChild.textContent = group.key
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

// --- context menu -------------------------------------------------------
async function openMenu() {
  const pets = await invoke('list_pets').catch(() => [])
  const installed = await invoke('hooks_installed').catch(() => false)
  const autostart = await invoke('autostart_enabled').catch(() => false)
  const children = []

  const heading = (text) => {
    const node = document.createElement('div')
    node.className = 'heading'
    node.textContent = text
    return node
  }
  const button = (text, onClick, pressed) => {
    const node = document.createElement('button')
    node.type = 'button'
    node.textContent = text
    if (pressed !== undefined) node.setAttribute('aria-pressed', String(pressed))
    node.addEventListener('click', async () => {
      el.menu.hidden = true
      await onClick()
      render()
    })
    return node
  }

  children.push(heading('Pet'))
  for (const pet of pets) {
    const label = pet.source === 'codex' ? `${pet.displayName} (Codex)` : pet.displayName
    children.push(button(label, () => selectPet(pet.id), pet.id === config.pet))
  }

  children.push(heading('Size'))
  for (const [label, value] of [['Small', 1.5], ['Medium', 2], ['Large', 3]]) {
    children.push(
      button(
        label,
        async () => {
          config.scale = value
          renderer.setScale(value)
          await saveConfig()
        },
        config.scale === value
      )
    )
  }

  children.push(heading('Cards'))
  children.push(
    button('Show all projects', () => {
      stackHidden = false
      collapsed.clear()
    })
  )
  children.push(
    button('Hide all cards', () => {
      stackHidden = true
    })
  )
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
    button(
      'Click through the pet',
      async () => {
        config.clickThrough = !config.clickThrough
        await saveConfig()
      },
      config.clickThrough
    )
  )

  children.push(heading('Setup'))
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
  children.push(button('Quit Pipsqueak', () => invoke('quit')))

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
      // A click outside the menu can't reach us — the window is click-through
      // there — so the pet itself is what dismisses it.
      if (!el.menu.hidden) el.menu.hidden = true
      else stackHidden = !stackHidden
      // Looking at the pet is looking at the pet.
      invoke('clear_attention').catch(() => {})
      render()
    }
    origin = null
  })

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

  if (!IS_TAURI) {
    startBrowserDemo()
    return
  }

  sessions = await invoke('get_sessions').catch(() => [])
  sessions.forEach((s) => seenSessions.add(s.session_id))
  render()

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
    const kept = notice ? sessions.filter((s) => s.session_id === 'notice') : []
    sessions = [...kept, ...incoming]
    render()
  })

  await listen('pipsqueak://notice', (event) => showNotice(String(event.payload)))

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

  // Ages and elapsed timers tick even when no event arrives.
  setInterval(render, 1000)
}

/** Built-in pets ship inside the frontend bundle; the rest come from disk. */
async function loadPetPayload(id) {
  if (id === 'byte' || id === 'ember' || id === 'pip') return null
  return invoke('load_pet', { id })
}

/** Cycles states in a browser tab so the UI can be designed without a build. */
function startBrowserDemo() {
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
    ]
  }
  let tick = 0
  const started = Date.now() - 143_000
  const advance = () => {
    const now = Date.now()
    sessions = Object.entries(scripts).map(([project, steps], i) => {
      const [want, kind, headline] = steps[(tick + i) % steps.length]
      // The demo speaks in display states; the real files speak in the three
      // separate fields, so translate rather than special-case the renderer.
      const outcome = want === 'done' || want === 'failed' ? want : ''
      let durable = want
      if (outcome) durable = 'idle'
      // Being blocked does not change what the session was doing.
      else if (want === 'waiting') durable = 'running'
      return {
        session_id: project,
        project,
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
}

boot()
