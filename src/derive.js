// Turning a session file into the one word a card shows.
//
// Kept apart from the rendering because this is where every judgement about
// truthfulness lives — whether a prompt is real, whether a turn is finished,
// which of several sessions speaks for a project — and those are worth being
// able to test without a window.
//
// Every function takes `now` so the awkward moments (a debounce that has not
// elapsed, a completion that has not settled) can be examined directly rather
// than waited for.

/**
 * A turn can enter and leave "blocked" in a few hundred milliseconds when
 * auto-mode or a permission hook answers the prompt. Only a wait that persists
 * is worth telling anyone about.
 */
export const WAITING_DEBOUNCE_MS = 800
/** How long a finished project keeps its card before collapsing to a chip. */
export const DONE_LINGER_MS = 30_000
/** A failed turn is something you need to see, so it waits around longer. */
export const FAILED_LINGER_MS = 5 * 60_000
/** Quiet for this long and the pet visibly dozes off. */
export const SLEEP_AFTER_MS = 60_000

export const URGENT = new Set(['waiting', 'failed'])
export const ACTIVE = new Set(['thinking', 'running', 'waiting', 'failed', 'compacting', 'done'])
/** The durable states a session file can report. */
export const RUNNING = new Set(['thinking', 'running', 'compacting'])

/**
 * Which state wins when a project has several, and how long each one holds the
 * card once shown.
 *
 * The hold is what makes the stack readable. Hook events arrive in bursts, and
 * without a floor the card can flick through three states faster than you can
 * read one — and "needs you" can vanish under a later "running" before you
 * ever look up.
 */
export const DISPLAY = {
  waiting: { rank: 5, hold: 2500 },
  failed: { rank: 4, hold: 4000 },
  done: { rank: 3, hold: 2500 },
  compacting: { rank: 2, hold: 1500 },
  running: { rank: 2, hold: 900 },
  thinking: { rank: 2, hold: 900 },
  idle: { rank: 0, hold: 0 }
}

export const priority = (state) => DISPLAY[state]?.rank ?? 1

/** Which of a project's sessions gets to speak for it. */
export function rank(session) {
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
export function blockedOn(session, now = Date.now()) {
  if (session.waiting_since && now - session.waiting_since >= WAITING_DEBOUNCE_MS) {
    return session.waiting_reason || 'Waiting for you'
  }
  // A permission prompt Claude Code raised and nothing has resolved. Most are
  // answered by auto-mode within a few hundred milliseconds and never reach a
  // human, which is exactly what the debounce filters out.
  if (session.pending_since && now - session.pending_since >= WAITING_DEBOUNCE_MS) {
    return session.pending_detail || `Permission for ${session.pending_tool}`
  }
  return ''
}

/**
 * Collapse the three questions a session file answers — what is it doing, how
 * did the last turn end, is a human blocking it — into a single word, in that
 * order of urgency.
 *
 * Returns '' while a prompt is inside its debounce, meaning "keep showing
 * whatever was already there".
 */
export function displayState(session, now = Date.now()) {
  if (blockedOn(session, now)) return 'waiting'
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

/**
 * Apply the minimum-display hold to a freshly derived state.
 *
 * `view` is mutated: it carries the state currently on screen, the earliest it
 * may be replaced, and the headline it belongs to. The headline matters —
 * holding "failed" over text that already says the turn succeeded is a worse
 * lie than the flicker the hold was preventing.
 */
export function holdState(view, session, raw, now = Date.now()) {
  const subject = session.headline || ''
  if (view.heldSubject !== subject) {
    view.heldSubject = subject
    view.heldState = ''
    view.heldUntil = 0
  }
  if (view.heldState && now < view.heldUntil && priority(raw) <= priority(view.heldState)) {
    return view.heldState
  }
  if (raw !== view.heldState) {
    view.heldState = raw
    view.heldUntil = now + (DISPLAY[raw]?.hold ?? 0)
  }
  return raw
}

/**
 * Is `candidate` a later release than `current`?
 *
 * Deliberately strict about what it accepts. This decides whether to tell
 * someone there is an update, based on a string from a network response, and
 * "probably newer" is not a good enough reason to nag.
 */
export function isNewer(candidate, current) {
  const parse = (value) => {
    const match = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/.exec(String(value ?? '').trim())
    return match ? { parts: match.slice(1, 4).map(Number), pre: match[4] ?? '' } : null
  }
  const a = parse(candidate)
  const b = parse(current)
  if (!a || !b) return false
  for (let i = 0; i < 3; i += 1) {
    if (a.parts[i] !== b.parts[i]) return a.parts[i] > b.parts[i]
  }
  // Same numbers: a prerelease is older than the release it precedes, and one
  // prerelease is never worth interrupting anyone about.
  return Boolean(b.pre) && !a.pre
}

export function relativeTime(ms, now = Date.now()) {
  if (!ms) return ''
  const delta = Math.max(0, now - ms)
  if (delta < 1000) return 'now'
  if (delta < 60_000) return `${Math.floor(delta / 1000)}s`
  if (delta < 3_600_000) return `${Math.floor(delta / 60_000)}m`
  return `${Math.floor(delta / 3_600_000)}h`
}

export function duration(ms, now = Date.now()) {
  if (!ms) return '0s'
  const seconds = Math.max(0, Math.floor((now - ms) / 1000))
  if (seconds < 60) return `${seconds}s`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  return `${Math.floor(seconds / 3600)}h`
}
