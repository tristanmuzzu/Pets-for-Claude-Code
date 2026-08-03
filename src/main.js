import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { PetRenderer, GREETING_ROW } from './pet.js'
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
  rank,
  relativeTime
} from './derive.js'

/** `npm run dev` in a plain browser has no IPC; fall back to a demo loop. */
const IS_TAURI = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

/** Replaced at build time from package.json. See vite.config.js. */
const APP_VERSION = typeof __APP_VERSION__ === 'string' ? __APP_VERSION__ : '0.0.0'

/** How many project cards are shown at once before the rest become chips. */
const SLOT_LIMIT = 3

const el = {
  stack: document.getElementById('stack'),
  chips: document.getElementById('chips'),
  menu: document.getElementById('menu'),
  panel: document.getElementById('panel'),
  panelTitle: document.getElementById('panel-title'),
  panelBody: document.getElementById('panel-body'),
  panelClose: document.getElementById('panel-close'),
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
  quiet: false,
  welcomed: false,
  updateCheck: false,
  updateDismissed: ''
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


function effectiveState(key, session) {
  const view = viewFor(key)
  const raw = displayState(session) || view.lastStable
  view.lastStable = RUNNING.has(raw) || raw === 'idle' ? raw : view.lastStable
  return holdState(view, session, raw)
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
    showNotice(`Pipsqueak ${latest} is available — ${RELEASES_PAGE}`)
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

function scheduleUpdateChecks() {
  if (!config.updateCheck) return
  const first = FIRST_CHECK_MS + Math.random() * FIRST_CHECK_JITTER_MS
  setTimeout(() => {
    checkForUpdate(false)
    setInterval(() => checkForUpdate(false), CHECK_EVERY_MS)
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

function closePanel() {
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
          ? 'Will check for updates — nothing is ever downloaded'
          : 'Will not check for updates'
      }),
      para('Then start a session. Restart Claude Code first — it reads its hooks at startup.'),
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
        setTimeout(async () => {
          clearInterval(tick)
          const [, detail] = await invoke('watch_result', { since }).catch(() => ['none', 'Check failed.'])
          status.textContent = detail
          button.disabled = false
          button.textContent = 'Test again'
        }, WATCH_MS)
      }),
      status,
      action('Copy report', async (button) => {
        const text = await invoke('doctor_report').catch(() => '')
        try {
          await navigator.clipboard.writeText(text)
          button.textContent = 'Copied — paste it into an issue'
        } catch {
          button.textContent = 'Could not reach the clipboard'
        }
      })
    )
    return nodes
  })
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
  children.push(button('Check my setup…', () => showDoctor()))
  children.push(button('Check for updates', () => checkForUpdate(true)))
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

  // Any dismissal counts as acknowledged — Done, the close button, or simply
  // never opening it again. A welcome that keeps coming back is worse than one
  // that is missed.
  if (!config.welcomed) {
    config.welcomed = true
    await saveConfig()
    showWelcome()
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

  scheduleUpdateChecks()

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

  // `?panel=welcome` / `?panel=doctor` so both can be designed in a browser
  // without a build, a real install, or a broken machine to point them at.
  const wanted = new URLSearchParams(location.search).get('panel')
  if (wanted === 'welcome') showWelcome()
  if (wanted === 'doctor') {
    openPanel('Setup check', () => [
      para('Rendering with sample results — the real check needs the app.'),
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
