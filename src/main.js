import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { PetRenderer, GREETING_ROW } from './pet.js'

/** `npm run dev` in a plain browser has no IPC; fall back to a demo loop. */
const IS_TAURI = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

/**
 * How long a live line stays put before the next one may replace it. Tools can
 * fire several times a second; without this the line is unreadable.
 */
const MIN_DWELL_MS = 2500
/** States that jump the queue — you never want a blocked prompt held back. */
const URGENT = new Set(['waiting', 'failed'])
const MAX_CARDS = 3
const STALE_MS = 45_000

const el = {
  stage: document.getElementById('stage'),
  stack: document.getElementById('stack'),
  chips: document.getElementById('chips'),
  menu: document.getElementById('menu'),
  pet: document.getElementById('pet'),
  template: document.getElementById('card-template')
}

const appWindow = IS_TAURI ? getCurrentWindow() : null
const renderer = new PetRenderer(el.pet)

let config = { pet: 'ember', scale: 2, clickThrough: false, showBubble: true }
let sessions = []
let notice = null
let noticeTimer = null
let stackHidden = false
let seenSessions = new Set()

/** project -> { shown, shownAt, pending, skipped, expanded } */
const views = new Map()
/** Projects the user collapsed; cleared automatically when one needs attention. */
const collapsed = new Set()
/** Last state seen per project, to detect transitions into an urgent state. */
const lastStates = new Map()
/** project -> card element, so updates don't rebuild (and re-animate) the DOM. */
const cards = new Map()

// --- data ---------------------------------------------------------------
/**
 * One card per project. Sessions arrive already ordered attention-first, so the
 * first session seen for a project is the one worth showing.
 */
function groupByProject(list) {
  const groups = []
  const index = new Map()
  for (const session of list) {
    const key = session.project || session.session_id.slice(0, 8)
    const existing = index.get(key)
    if (existing) {
      existing.count += 1
      continue
    }
    const group = { key, session, count: 1 }
    index.set(key, group)
    groups.push(group)
  }
  return groups
}

function relativeTime(ms) {
  if (!ms) return ''
  const delta = Math.max(0, Date.now() - ms)
  if (delta < 1000) return 'now'
  if (delta < 60_000) return `${Math.floor(delta / 1000)}s`
  if (delta < 3_600_000) return `${Math.floor(delta / 60_000)}m`
  return `${Math.floor(delta / 3_600_000)}h`
}

function viewFor(key) {
  let view = views.get(key)
  if (!view) {
    view = { shown: '', shownAt: 0, pending: '', skipped: 0, expanded: false }
    views.set(key, view)
  }
  return view
}

/**
 * Rate-limits the live line. Urgent states are adopted immediately; everything
 * else waits out the dwell, and anything that arrived meanwhile is counted so
 * the card can show "+3" rather than silently dropping it.
 */
function pumpLiveLine(key, activity, state) {
  const view = viewFor(key)
  const now = Date.now()
  if (activity !== view.pending) {
    if (view.pending && view.pending !== view.shown) view.skipped += 1
    view.pending = activity
  }
  const urgent = URGENT.has(state)
  const settled = now - view.shownAt >= MIN_DWELL_MS
  if (view.pending !== view.shown && (urgent || settled || !view.shown)) {
    view.shown = view.pending
    view.shownAt = now
    view.skipped = 0
  }
  return view
}

// --- rendering ----------------------------------------------------------
function buildCard(key) {
  const node = el.template.content.firstElementChild.cloneNode(true)
  node.querySelector('.close').addEventListener('click', (event) => {
    event.stopPropagation()
    collapsed.add(key)
    render()
  })
  node.addEventListener('click', () => {
    const view = viewFor(key)
    view.expanded = !view.expanded
    render()
  })
  return node
}

function paintCard(node, group) {
  const { session, key, count } = group
  const view = pumpLiveLine(key, session.activity || '', session.state)
  const stale = Date.now() - session.updated_ms > STALE_MS

  node.dataset.state = session.state || 'idle'
  node.dataset.dim = String(session.state === 'idle' && stale)
  node.querySelector('.project').textContent = key
  node.querySelector('.age').textContent = relativeTime(session.updated_ms)

  const countNode = node.querySelector('.count')
  countNode.hidden = count < 2
  countNode.textContent = `${count} sessions`

  // Sessions that started before their first prompt have no headline yet.
  const headline = session.headline || session.activity || 'Working…'
  node.querySelector('.headline').textContent = headline

  const live = node.querySelector('.live')
  // Compare against what is actually on screen, or the fallback above shows the
  // same sentence twice.
  const liveText = view.shown && view.shown !== headline ? view.shown : ''
  live.hidden = !liveText
  node.querySelector('.live-text').textContent = liveText
  const burst = node.querySelector('.burst')
  burst.hidden = view.skipped < 1
  burst.textContent = `+${view.skipped}`

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

function buildChip(key, state, label, onClick) {
  const button = document.createElement('button')
  button.type = 'button'
  const dot = document.createElement('span')
  dot.className = 'dot'
  dot.style.background = `var(--${state in STATE_COLOURS ? state : 'idle'})`
  const text = document.createElement('span')
  text.textContent = label
  button.append(dot, text)
  button.addEventListener('click', onClick)
  return button
}

const STATE_COLOURS = {
  idle: 1, thinking: 1, running: 1, waiting: 1, failed: 1, done: 1, compacting: 1
}

function render() {
  const groups = groupByProject(sessions)

  // A project that starts needing attention un-collapses itself.
  for (const group of groups) {
    const previous = lastStates.get(group.key)
    if (URGENT.has(group.session.state) && previous !== group.session.state) {
      collapsed.delete(group.key)
    }
    lastStates.set(group.key, group.session.state)
  }

  const leader = groups[0]?.session
  renderer.setState(notice ? 'idle' : leader ? leader.state : 'idle')

  const visible = stackHidden
    ? []
    : groups.filter((group) => !collapsed.has(group.key)).slice(0, MAX_CARDS)
  const visibleKeys = new Set(visible.map((group) => group.key))

  for (const [key, node] of cards) {
    if (!visibleKeys.has(key)) {
      node.remove()
      cards.delete(key)
    }
  }

  // DOM order is bottom-up: #stack is column-reverse, so the first child sits
  // closest to the pet and the chip row ends up on top of the stack.
  let previous = null
  for (const group of visible) {
    let node = cards.get(group.key)
    if (!node) {
      node = buildCard(group.key)
      cards.set(group.key, node)
    }
    paintCard(node, group)
    if (previous) {
      if (previous.nextElementSibling !== node) previous.after(node)
    } else if (el.stack.firstElementChild !== node) {
      el.stack.prepend(node)
    }
    previous = node
  }
  el.stack.append(el.chips)

  const hiddenGroups = stackHidden
    ? groups
    : groups.filter((group) => collapsed.has(group.key) || !visibleKeys.has(group.key))
  el.chips.hidden = hiddenGroups.length === 0
  el.chips.replaceChildren(
    ...hiddenGroups.map((group) =>
      buildChip(group.key, group.session.state, group.key, () => {
        stackHidden = false
        collapsed.delete(group.key)
        render()
      })
    )
  )

  syncHitRects()
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
    render()
  }, 6000)
  sessions = [
    {
      session_id: 'notice',
      project: 'Pipsqueak',
      state: 'idle',
      headline: message,
      activity: '',
      detail: '',
      updated_ms: Date.now(),
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
    const payload = await invoke('load_pet', { id })
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
      pet: stored.pet ?? 'ember',
      scale: stored.scale ?? 2,
      clickThrough: Boolean(stored.click_through),
      showBubble: stored.show_bubble !== false,
      x: stored.x,
      y: stored.y
    }
  }

  try {
    await renderer.load(config.pet, await loadPetPayload(config.pet))
  } catch {
    config.pet = 'ember'
    await renderer.load('ember', null)
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
    sessions = notice ? [sessions[0], ...incoming] : incoming
    render()
  })

  await listen('pipsqueak://notice', (event) => showNotice(String(event.payload)))

  // Ages tick and the dwell timer expires even when no event arrives.
  setInterval(render, 1000)
}

/** Built-in pets ship inside the frontend bundle; the rest come from disk. */
async function loadPetPayload(id) {
  if (id === 'ember' || id === 'pip') return null
  return invoke('load_pet', { id })
}

/** Cycles states in a browser tab so the UI can be designed without a build. */
function startBrowserDemo() {
  const scripts = {
    pipsqueak: [
      ['thinking', 'Fix the flaky timezone test', 'Thinking…'],
      ['running', 'Fix the flaky timezone test', 'Reading clock.test.js'],
      ['running', 'Fix the flaky timezone test', 'Running: npm test -- --run'],
      ['waiting', 'Fix the flaky timezone test', 'Needs permission to run: rm -rf build'],
      ['failed', 'Fix the flaky timezone test', 'Bash failed'],
      ['done', 'Fixed: the formatter used local time.', '']
    ],
    orchestrator: [
      ['running', 'Add tier C eval scenarios', 'Editing scenarios.json'],
      ['running', 'Add tier C eval scenarios', 'Running: pytest -q'],
      ['done', 'Added 12 scenarios and wired them into the suite.', '']
    ]
  }
  let tick = 0
  const advance = () => {
    const now = Date.now()
    sessions = Object.entries(scripts).map(([project, steps], i) => {
      const [state, headline, activity] = steps[(tick + i) % steps.length]
      return {
        session_id: project,
        project,
        state,
        headline,
        activity,
        detail: state === 'failed' ? 'expected 03:00 to be 02:00' : '',
        updated_ms: now,
        started_ms: now - 60_000,
        tools: tick,
        recent: [{ ms: now, state, text: activity || headline }]
      }
    })
    tick += 1
    render()
  }
  advance()
  setInterval(advance, 2000)
  setInterval(render, 1000)
}

boot()
