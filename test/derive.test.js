// The judgements that decide whether a card is telling the truth.
import test from 'node:test'
import assert from 'node:assert/strict'
import {
  DONE_LINGER_MS,
  WAITING_DEBOUNCE_MS,
  blockedOn,
  displayState,
  holdState,
  isNewer,
  relativeTime,
  stillWorking,
  worthCelebrating,
  CELEBRATE_AFTER_MS,
  OUTSTANDING_STALE_MS
} from '../src/derive.js'

const NOW = 1_000_000

/** A session file with nothing going on, for tests to bend one field of. */
const quiet = (fields = {}) => ({
  state: 'running',
  outcome: '',
  outcome_ms: 0,
  settles_ms: 0,
  waiting_since: 0,
  pending_since: 0,
  headline: 'Fix the timezone test',
  ...fields
})

test('a prompt inside the debounce is not yet worth showing', () => {
  const session = quiet({ pending_since: NOW - 200, pending_tool: 'Bash' })
  assert.equal(blockedOn(session, NOW), '')
  // '' means "keep showing whatever was already there", not "idle".
  assert.equal(displayState(session, NOW), '')
})

test('a prompt that outlives the debounce means a human is being asked', () => {
  const session = quiet({
    pending_since: NOW - WAITING_DEBOUNCE_MS - 1,
    pending_tool: 'Bash',
    pending_detail: 'run: npm install'
  })
  assert.equal(blockedOn(session, NOW), 'run: npm install')
  assert.equal(displayState(session, NOW), 'waiting')
})

test('an explicit notification outranks a pending permission', () => {
  const session = quiet({
    waiting_since: NOW - 5000,
    waiting_reason: 'Waiting for your reply',
    pending_since: NOW - 5000,
    pending_detail: 'run: npm install'
  })
  assert.equal(blockedOn(session, NOW), 'Waiting for your reply')
})

test('a completion that has not settled is still a running turn', () => {
  const session = quiet({ state: 'running', outcome: 'done', outcome_ms: NOW, settles_ms: NOW + 2000 })
  assert.equal(displayState(session, NOW), 'running')
  assert.equal(displayState(session, NOW + 2001), 'done')
})

test('a settling turn stays on screen even though the producer wrote idle', () => {
  // The hook writes `state: "idle"` together with the outcome. Reporting that
  // idle during the settle window took the card out of the DOM for two
  // seconds and popped it back in as "done" — the flicker this window exists
  // to prevent.
  const session = quiet({ state: 'idle', outcome: 'done', outcome_ms: NOW, settles_ms: NOW + 2000 })
  assert.equal(displayState(session, NOW), 'running')
  assert.equal(displayState(session, NOW + 2001), 'done')
})

test('a turn that ended over running work is not done', () => {
  // The complaint this exists for: the assistant yields the floor while five
  // subagents run and its last words are that it is waiting for them. `Stop`
  // fires anyway, and no hook will ever fire for those finishing.
  const session = quiet({
    state: 'idle',
    outcome: 'done',
    outcome_ms: NOW,
    settles_ms: NOW,
    outstanding: 5,
    updated_ms: NOW
  })
  assert.equal(displayState(session, NOW + 3000), 'finishing')
  assert.equal(stillWorking(session, NOW + 3000), true)
  // Not worth a tray blink either: nothing has finished.
  assert.equal(worthCelebrating(session, NOW + 60_000), false)

  // The last one reports back, and only then is it done.
  const drained = { ...session, outstanding: 0 }
  assert.equal(displayState(drained, NOW + 3000), 'done')
})

test('outstanding work is written off once the session goes quiet', () => {
  // The overlay can start watching halfway through and see a launch whose
  // completion it already missed. A card stuck on "finishing" forever would
  // be its own kind of lie.
  const session = quiet({
    state: 'idle',
    outcome: 'done',
    outcome_ms: NOW,
    settles_ms: NOW,
    outstanding: 2,
    updated_ms: NOW
  })
  assert.equal(displayState(session, NOW + OUTSTANDING_STALE_MS - 1000), 'finishing')
  assert.equal(displayState(session, NOW + OUTSTANDING_STALE_MS + 1000), 'done')
})

test('a completion is announced only once it has held', () => {
  // A blocking stop hook can veto a stop seconds later and send the turn back
  // to work. The card can correct itself; a tray blink cannot.
  const session = quiet({ state: 'idle', outcome: 'done', outcome_ms: NOW, updated_ms: NOW })
  assert.equal(worthCelebrating(session, NOW + 2000), false)
  assert.equal(worthCelebrating(session, NOW + CELEBRATE_AFTER_MS + 1), true)
})

test('a finished turn stays finished until somebody looks at it', () => {
  // No timer: a card that removes itself after 30s is a card that finished
  // while you were in a meeting and never told you.
  const session = quiet({ state: 'idle', outcome: 'done', outcome_ms: NOW })
  assert.equal(displayState(session, NOW + 1000), 'done')
  assert.equal(displayState(session, NOW + DONE_LINGER_MS * 10), 'done')
})

test('a failed turn stays visible too', () => {
  const session = quiet({ state: 'idle', outcome: 'failed', outcome_ms: NOW })
  assert.equal(displayState(session, NOW + DONE_LINGER_MS * 10), 'failed')
})

test('a session the sweep gave up on reads as idle, not finished', () => {
  // The sweep sets this when a running session has produced no event for
  // longer than any real turn goes quiet. It is neither a completion nor a
  // failure, and it must not outrank either.
  const session = quiet({ state: 'running', stalled: true })
  assert.equal(displayState(session, NOW), 'idle')
})

test('a question still being asked outranks having given up on the session', () => {
  // Both claims come from the same silence, and only one of them can be
  // checked: the prompt is a fact, "stopped responding" is an inference. A
  // session flagged stalled by an older build — or by any future rule — must
  // still read as waiting while the question is on the card, or the status
  // line contradicts the row underneath it.
  const session = quiet({
    state: 'running',
    stalled: true,
    pending_since: NOW - WAITING_DEBOUNCE_MS * 2,
    pending_tool: 'Bash',
    pending_detail: 'run: npm run deploy'
  })
  assert.equal(displayState(session, NOW), 'waiting')
  assert.equal(blockedOn(session, NOW), 'run: npm run deploy')
})

test('a state holds long enough to be read', () => {
  const view = {}
  const session = quiet()
  assert.equal(holdState(view, session, 'failed', NOW), 'failed')
  // Something less urgent cannot take the card away yet.
  assert.equal(holdState(view, session, 'running', NOW + 100), 'failed')
  // Something more urgent can, immediately.
  assert.equal(holdState(view, session, 'waiting', NOW + 100), 'waiting')
})

test('the hold does not survive the card saying something else', () => {
  const view = {}
  assert.equal(holdState(view, quiet({ headline: 'Add the eval suite' }), 'failed', NOW), 'failed')
  // A new headline is a new situation; holding the old verdict over new text
  // is a worse lie than the flicker the hold prevents.
  assert.equal(
    holdState(view, quiet({ headline: 'Added 12 scenarios; suite passes' }), 'done', NOW + 100),
    'done'
  )
})

test('only a genuinely newer release is worth mentioning', () => {
  assert.ok(isNewer('v0.6.0', '0.5.0'))
  assert.ok(isNewer('0.5.1', '0.5.0'))
  assert.ok(isNewer('1.0.0', '0.9.9'))
  assert.ok(!isNewer('0.5.0', '0.5.0'))
  assert.ok(!isNewer('0.4.9', '0.5.0'))
  // A release beats the prerelease it followed, but never the other way.
  assert.ok(isNewer('0.5.0', '0.5.0-rc.1'))
  assert.ok(!isNewer('0.5.0-rc.2', '0.5.0-rc.1'))
})

test('a nonsense version never triggers a nag', () => {
  // The candidate arrives over the network, so anything unparseable has to
  // fail closed rather than be guessed at.
  for (const junk of ['', null, undefined, 'latest', 'v1', '1.2', '<script>', '99.99.99.99']) {
    assert.equal(isNewer(junk, '0.5.0'), false, `${junk} should not count as newer`)
  }
})

test('ages read the way a glance expects', () => {
  assert.equal(relativeTime(0, NOW), '')
  assert.equal(relativeTime(NOW - 500, NOW), 'now')
  assert.equal(relativeTime(NOW - 90_000, NOW), '1m')
  assert.equal(relativeTime(NOW - 7_200_000, NOW), '2h')
})
