// Replays realistic Claude Code hook sequences against the installed binary, so
// the overlay can be exercised without starting real agent sessions.
//
//   node tools/simulate.mjs [path-to-pipsqueak-binary]
//
// Three projects run concurrently at different cadences, the case that made
// a single shared bubble unreadable.
import { spawn } from 'node:child_process'
import { appendFileSync, existsSync, mkdirSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = dirname(fileURLToPath(import.meta.url))
const exeName = process.platform === 'win32' ? 'pipsqueak.exe' : 'pipsqueak'
const BIN =
  process.argv[2] ??
  ['debug', 'release']
    .map((profile) => resolve(HERE, '..', 'src-tauri', 'target', profile, exeName))
    .find(existsSync)

if (!BIN) {
  console.error('No pipsqueak binary found. Pass one as the first argument.')
  process.exit(1)
}

const sleep = (ms) => new Promise((done) => setTimeout(done, ms))

/**
 * Transcripts, because half of what the overlay shows comes from them.
 *
 * The card's main line is whatever the session last said, read out of the
 * transcript Claude Code is appending to. A simulation that fires hooks but
 * never says anything exercises half the app and records a demo of the other
 * half, so these are real files in the real format, written as the run goes.
 */
const TRANSCRIPTS = resolve(tmpdir(), 'pipsqueak-sim-transcripts')
mkdirSync(TRANSCRIPTS, { recursive: true })
const transcriptFor = (session) => resolve(TRANSCRIPTS, `${session}.jsonl`)

function say(session, text, kind = 'text') {
  const part = kind === 'text' ? { type: 'text', text } : { type: 'thinking', thinking: text }
  appendFileSync(
    transcriptFor(session),
    `${JSON.stringify({ type: 'assistant', message: { content: [part] } })}
`
  )
}

function fire(session, cwd, event, extra) {
  return new Promise((done, fail) => {
    const child = spawn(BIN, ['hook', event], { stdio: ['pipe', 'ignore', 'inherit'] })
    child.on('error', fail)
    child.on('exit', () => done())
    child.stdin.end(
      JSON.stringify({
        session_id: session,
        cwd,
        transcript_path: transcriptFor(session),
        permission_mode: 'default',
        hook_event_name: event,
        ...extra
      })
    )
  })
}

const bash = (command, description) => ({
  tool_name: 'Bash',
  tool_input: { command, description }
})
const file = (tool, path) => ({ tool_name: tool, tool_input: { file_path: path } })

const PROJECTS = [
  {
    id: 'sim-timezone',
    cwd: 'C:/code/clockwork',
    // Frantic on purpose: a tool every few hundred ms.
    steps: [
      [500, 'SessionStart', { source: 'startup' }],
      [700, 'UserPromptSubmit', { prompt: 'Fix the flaky timezone test' }],
      [0, 'Say', { text: 'Finding every place the formatter decides on a timezone.' }],
      [400, 'PreToolUse', { tool_name: 'Grep', tool_input: { pattern: 'toISOString' } }],
      [300, 'PostToolUse', { tool_name: 'Grep', tool_input: { pattern: 'toISOString' } }],
      [300, 'PreToolUse', file('Read', 'C:/code/clockwork/src/clock.test.js')],
      [300, 'PostToolUse', file('Read', 'C:/code/clockwork/src/clock.test.js')],
      [300, 'PreToolUse', file('Read', 'C:/code/clockwork/src/clock.js')],
      [0, 'Say', { text: 'It parses in local time and compares in UTC. That is the bug.' }],
      [300, 'PostToolUse', file('Read', 'C:/code/clockwork/src/clock.js')],
      [0, 'Say', { text: 'Pinning the zone at the boundary instead of at the assertion.' }],
      [400, 'PreToolUse', file('Edit', 'C:/code/clockwork/src/clock.js')],
      [400, 'PostToolUse', file('Edit', 'C:/code/clockwork/src/clock.js')],
      // Claude Code runs PreToolUse first (it may answer the permission
      // itself) and only then raises PermissionRequest. This pair is the
      // "auto-mode answered it, you were never bothered" case, which the pet
      // must not report: the prompt is resolved well inside the debounce.
      [400, 'PreToolUse', bash('npm run lint', 'Lint the project')],
      [120, 'PermissionRequest', bash('npm run lint')],
      [300, 'PostToolUse', bash('npm run lint', 'Lint the project')],
      // And this is a prompt a human actually saw.
      [0, 'Say', { text: 'Running the suite needs your say-so; the prompt is on your screen.' }],
      [1500, 'Notification', { notification_type: 'permission_prompt', message: 'Allow Bash(npm test)?' }],
      [2600, 'PreToolUse', bash('npm test -- --run', 'Run the test suite')],
      [1500, 'PostToolUseFailure', {
        ...bash('npm test -- --run'),
        error: 'clock.test.js > formats in UTC\n  expected 03:00 to be 02:00'
      }],
      [0, 'Say', { text: 'One still red: expected 03:00, got 02:00. The offset lands twice.' }],
      [1400, 'PreToolUse', file('Edit', 'C:/code/clockwork/src/clock.js')],
      [0, 'Say', { text: 'Converting once, at the boundary, and letting the test compare plainly.' }],
      [1500, 'PreToolUse', bash('npm test -- --run', 'Run the test suite')],
      [1400, 'PostToolUse', bash('npm test -- --run')],
      // A real transcript carries the final answer too, so the live line lands
      // on it as the turn ends rather than keeping the last mid-work sentence.
      [0, 'Say', { text: 'Fixed: the formatter used local time. All 42 tests pass.' }],
      [12000, 'Stop', {
        last_assistant_message: 'Fixed: the formatter used local time.\n\nAll 42 tests pass.'
      }],
      [500, 'SessionEnd', { reason: 'clear' }]
    ]
  },
  {
    id: 'sim-orchestrator',
    cwd: 'C:/code/orchestrator',
    steps: [
      [1200, 'SessionStart', { source: 'startup' }],
      [900, 'UserPromptSubmit', { prompt: 'Add tier C eval scenarios and wire them into the suite' }],
      [0, 'Say', { text: 'Adding the twelve tier C scenarios to the eval set.' }],
      [1400, 'PreToolUse', file('Read', 'C:/code/orchestrator/evals/scenarios.json')],
      [1600, 'PreToolUse', file('Edit', 'C:/code/orchestrator/evals/scenarios.json')],
      [0, 'Say', { text: 'Eight of them time out at the old budget, so I am raising it.' }],
      [2000, 'PreToolUse', bash('pytest -q evals', 'Run the eval suite')],
      [3000, 'PostToolUse', bash('pytest -q evals')],
      [0, 'Say', { text: 'Added 12 tier C scenarios; the suite passes in 41s.' }],
      [14000, 'Stop', {
        last_assistant_message: 'Added 12 tier C scenarios; the suite passes in 41s.'
      }],
      [500, 'SessionEnd', { reason: 'clear' }]
    ]
  },
  {
    // Rooted in a temp directory: hidden unless "Include scratch/temp sessions"
    // is on. This is what an eval or scripted run looks like.
    id: 'sim-scratch',
    cwd: `${process.env.TEMP?.replace(/\\/g, '/') ?? '/tmp'}/harness-ablation/runs/design-quality__r7`,
    steps: [
      [900, 'SessionStart', { source: 'startup' }],
      [900, 'UserPromptSubmit', { prompt: 'Score the transcript against the rubric' }],
      [20000, 'PreToolUse', file('Read', 'transcript.jsonl')],
      [500, 'SessionEnd', { reason: 'clear' }]
    ]
  },
  {
    id: 'sim-website',
    cwd: 'C:/code/website',
    steps: [
      [2600, 'SessionStart', { source: 'startup' }],
      [800, 'UserPromptSubmit', { prompt: 'Rewrite the pricing page copy' }],
      [0, 'Say', { text: 'Leading with the free tier, since that is what people arrive for.' }],
      [2200, 'PreToolUse', file('Edit', 'C:/code/website/app/pricing/page.tsx')],
      [0, 'Say', { text: 'Putting up a preview deploy so you can read it in place.' }],
      [2600, 'PreToolUse', { tool_name: 'mcp__vercel__deploy_preview', tool_input: {} }],
      [16000, 'Notification', { notification_type: 'idle_prompt' }],
      [500, 'SessionEnd', { reason: 'clear' }]
    ]
  },
  // These two arrive after the first recording window has closed, so the same
  // run can be captured twice: three cards early, a collapsed stack later.
  {
    id: 'sim-ledger',
    cwd: 'C:/code/ledger-api',
    steps: [
      [20000, 'Wait', {}],
      [400, 'SessionStart', { source: 'startup' }],
      [600, 'UserPromptSubmit', { prompt: 'Round invoice totals the way finance does' }],
      [0, 'Say', { text: 'Finance rounds half up, and only at the invoice total.' }],
      [1800, 'PreToolUse', file('Edit', 'C:/code/ledger-api/src/totals.ts')],
      [0, 'Say', { text: 'Rewriting the totals so they round once, at the end.' }],
      [14000, 'PreToolUse', bash('npm test -- totals', 'Run the totals tests')],
      [500, 'SessionEnd', { reason: 'clear' }]
    ]
  },
  {
    id: 'sim-atlas',
    cwd: 'C:/code/atlas-tiles',
    steps: [
      [21500, 'Wait', {}],
      [400, 'SessionStart', { source: 'startup' }],
      [700, 'UserPromptSubmit', { prompt: 'Stop re-fetching tiles every hour' }],
      [0, 'Say', { text: 'Tiles are re-fetched hourly for no reason; caching for a week.' }],
      [2400, 'PreToolUse', file('Edit', 'C:/code/atlas-tiles/server/headers.go')],
      [0, 'Say', { text: 'Content hash in the path, so a week of cache is still safe.' }],
      [14000, 'PreToolUse', bash('go test ./server', 'Run the server tests')],
      [500, 'SessionEnd', { reason: 'clear' }]
    ]
  }
]

async function play(project) {
  // A fresh transcript per run, or the second recording narrates the first.
  writeFileSync(transcriptFor(project.id), '')
  for (const [delay, event, extra] of project.steps) {
    if (event === 'Wait') {
      // The delay on a step is served after it, so a project that should join
      // late needs something to wait *on* before its first real event.
      console.log(`  ${project.id.padEnd(18)} waiting ${delay}ms to join`)
    } else if (event === 'Say') {
      say(project.id, extra.text, extra.kind)
      console.log(`  ${project.id.padEnd(18)} said "${extra.text.slice(0, 48)}…"`)
    } else {
      await fire(project.id, project.cwd, event, extra)
      console.log(`  ${project.id.padEnd(18)} ${event}${extra.tool_name ? ` (${extra.tool_name})` : ''}`)
    }
    await sleep(delay)
  }
}

console.log(`replaying ${PROJECTS.length} concurrent projects through ${BIN}\n`)
await Promise.all(PROJECTS.map(play))
console.log('\ndone')
