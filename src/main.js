import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { PetRenderer, GREETING_ROW } from './pet.js'

const el = {
  card: document.getElementById('card'),
  project: document.getElementById('project'),
  age: document.getElementById('age'),
  activity: document.getElementById('activity'),
  detail: document.getElementById('detail'),
  expanded: document.getElementById('expanded'),
  tabs: document.getElementById('tabs'),
  log: document.getElementById('log'),
  menu: document.getElementById('menu'),
  pet: document.getElementById('pet')
}

/** `npm run dev` in a plain browser has no IPC; fall back to a demo loop. */
const IS_TAURI = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

const appWindow = IS_TAURI ? getCurrentWindow() : null
const renderer = new PetRenderer(el.pet)

let config = { pet: 'pip', scale: 2, clickThrough: false, showBubble: true }
let sessions = []
let pinnedSession = null
let expanded = false
let notice = null
let noticeTimer = null
let seenSessions = new Set()

// --- state selection ---------------------------------------------------
function activeSession() {
  if (pinnedSession) {
    const match = sessions.find((s) => s.session_id === pinnedSession)
    if (match) return match
  }
  return sessions[0] ?? null
}

function relativeTime(ms) {
  if (!ms) return ''
  const delta = Math.max(0, Date.now() - ms)
  if (delta < 1000) return 'now'
  if (delta < 60_000) return `${Math.floor(delta / 1000)}s`
  if (delta < 3_600_000) return `${Math.floor(delta / 60_000)}m`
  return `${Math.floor(delta / 3_600_000)}h`
}

// --- rendering ---------------------------------------------------------
function render() {
  const session = activeSession()
  renderer.setState(session ? session.state : 'idle')

  if (notice) {
    el.card.hidden = false
    el.card.dataset.state = 'idle'
    el.card.dataset.dim = 'false'
    el.project.textContent = 'Pipsqueak'
    el.age.textContent = ''
    el.activity.textContent = notice
    el.detail.hidden = true
    el.expanded.hidden = !expanded
    renderTabs()
    renderLog(activeSession())
    syncHitRects()
    return
  }

  if (!session) {
    el.card.hidden = !expanded
    if (expanded) {
      el.card.dataset.state = 'idle'
      el.project.textContent = 'Pipsqueak'
      el.age.textContent = ''
      el.activity.textContent = 'No Claude Code session is running.'
      el.detail.hidden = true
    }
    el.expanded.hidden = !expanded
    renderTabs()
    renderLog(null)
    syncHitRects()
    return
  }

  const stale = Date.now() - session.updated_ms > 45_000
  el.card.hidden = !config.showBubble && !expanded
  el.card.dataset.state = session.state
  el.card.dataset.dim = String(session.state === 'idle' && stale)
  el.project.textContent = session.project || 'Claude Code'
  el.age.textContent = relativeTime(session.updated_ms)
  el.activity.textContent = session.activity || '…'

  const showDetail = expanded && Boolean(session.detail)
  el.detail.hidden = !showDetail
  if (showDetail) el.detail.textContent = session.detail

  el.expanded.hidden = !expanded
  renderTabs()
  renderLog(session)
  syncHitRects()
}

function renderTabs() {
  if (!expanded || sessions.length < 2) {
    el.tabs.hidden = true
    el.tabs.replaceChildren()
    return
  }
  el.tabs.hidden = false
  const current = activeSession()
  el.tabs.replaceChildren(
    ...sessions.map((session) => {
      const button = document.createElement('button')
      button.type = 'button'
      button.textContent = session.project || session.session_id.slice(0, 6)
      button.setAttribute('aria-pressed', String(session === current))
      button.addEventListener('click', () => {
        pinnedSession = session.session_id
        render()
      })
      return button
    })
  )
}

function renderLog(session) {
  if (!expanded) {
    el.log.replaceChildren()
    return
  }
  const entries = session?.recent ?? []
  el.log.replaceChildren(
    ...entries
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
  add(el.card)
  add(el.menu)
  invoke('set_hit_rects', { rects }).catch(() => {})
}

function showNotice(message) {
  notice = message
  clearTimeout(noticeTimer)
  noticeTimer = setTimeout(() => {
    notice = null
    render()
  }, 6000)
  render()
}

// --- context menu ------------------------------------------------------
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
    children.push(
      button(label, () => selectPet(pet.id), pet.id === config.pet)
    )
  }

  children.push(heading('Size'))
  for (const [label, value] of [['Small', 1.5], ['Medium', 2], ['Large', 3]]) {
    children.push(
      button(label, async () => {
        config.scale = value
        renderer.setScale(value)
        await saveConfig()
      }, config.scale === value)
    )
  }

  children.push(heading('Window'))
  children.push(
    button('Click through the pet', async () => {
      config.clickThrough = !config.clickThrough
      await saveConfig()
    }, config.clickThrough)
  )
  children.push(
    button('Show status bubble', async () => {
      config.showBubble = !config.showBubble
      await saveConfig()
    }, config.showBubble)
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

// --- interaction -------------------------------------------------------
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
      if (el.menu.hidden) expanded = !expanded
      else el.menu.hidden = true
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

  el.card.addEventListener('click', (event) => {
    const action = event.target.closest('[data-action]')?.dataset.action
    if (!action) return
    if (action === 'pets') invoke('open_pets_dir').catch(() => {})
    if (action === 'quit') invoke('quit')
    if (action === 'hooks') {
      invoke('install_hooks')
        .then(showNotice)
        .catch((error) => showNotice(String(error)))
    }
    if (action === 'through') {
      config.clickThrough = !config.clickThrough
      saveConfig().then(render)
    }
  })

  if (!appWindow) return
  appWindow.onMoved(({ payload }) => {
    config.x = payload.x
    config.y = payload.y
    invoke('save_position', { x: payload.x, y: payload.y }).catch(() => {})
  })
}

// --- boot --------------------------------------------------------------
async function boot() {
  const stored = await invoke('get_config').catch(() => null)
  if (stored) {
    config = {
      pet: stored.pet ?? 'pip',
      scale: stored.scale ?? 2,
      clickThrough: Boolean(stored.click_through),
      showBubble: stored.show_bubble !== false,
      x: stored.x,
      y: stored.y
    }
  }

  try {
    const payload = config.pet === 'pip' ? null : await invoke('load_pet', { id: config.pet })
    await renderer.load(config.pet, payload)
  } catch {
    await renderer.load('pip', null)
    config.pet = 'pip'
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
    if (pinnedSession && !live.has(pinnedSession)) pinnedSession = null
    sessions = incoming
    render()
  })

  await listen('pipsqueak://notice', (event) => showNotice(String(event.payload)))

  // Ages tick even when nothing changes, and layout can shift under us.
  setInterval(render, 1000)
}

/** Cycles the states in a browser tab so the UI can be designed without a build. */
function startBrowserDemo() {
  const steps = [
    ['thinking', 'Thinking…', ''],
    ['running', 'Reading clock.test.js', ''],
    ['running', 'Running: npm test -- --run', ''],
    ['waiting', 'Needs permission: Running: rm -rf build', ''],
    ['failed', 'Bash failed', 'clock.test.js > formats in UTC\n  expected 03:00 to be 02:00'],
    ['done', 'Fixed: the formatter used local time.', 'All 42 tests pass.'],
    ['idle', 'Session started', '']
  ]
  let index = 0
  const advance = () => {
    const [state, activity, detail] = steps[index % steps.length]
    const now = Date.now()
    const previous = sessions[0]?.recent ?? []
    sessions = [
      {
        session_id: 'demo',
        state,
        activity,
        detail,
        project: 'pipsqueak',
        cwd: '/demo',
        event: 'demo',
        updated_ms: now,
        started_ms: now - 60_000,
        tools: index,
        recent: [...previous, { ms: now, state, text: activity }].slice(-24)
      }
    ]
    index += 1
    render()
  }
  advance()
  setInterval(advance, 2600)
  setInterval(render, 1000)
}

boot()
