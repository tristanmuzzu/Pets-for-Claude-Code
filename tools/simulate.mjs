// Replays realistic Claude Code hook sequences against the installed binary, so
// the overlay can be exercised without starting real agent sessions.
//
//   node tools/simulate.mjs [path-to-pipsqueak-binary]
//
// Three projects run concurrently at different cadences — the case that made
// a single shared bubble unreadable.
import { spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
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

function fire(session, cwd, event, extra) {
  return new Promise((done, fail) => {
    const child = spawn(BIN, ['hook', event], { stdio: ['pipe', 'ignore', 'inherit'] })
    child.on('error', fail)
    child.on('exit', () => done())
    child.stdin.end(
      JSON.stringify({
        session_id: session,
        cwd,
        transcript_path: '',
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
    // Deliberately frantic: a tool every few hundred ms.
    steps: [
      [500, 'SessionStart', { source: 'startup' }],
      [700, 'UserPromptSubmit', { prompt: 'Fix the flaky timezone test' }],
      [400, 'PreToolUse', { tool_name: 'Grep', tool_input: { pattern: 'toISOString' } }],
      [300, 'PostToolUse', { tool_name: 'Grep', tool_input: { pattern: 'toISOString' } }],
      [300, 'PreToolUse', file('Read', 'C:/code/clockwork/src/clock.test.js')],
      [300, 'PostToolUse', file('Read', 'C:/code/clockwork/src/clock.test.js')],
      [300, 'PreToolUse', file('Read', 'C:/code/clockwork/src/clock.js')],
      [300, 'PostToolUse', file('Read', 'C:/code/clockwork/src/clock.js')],
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
      [1500, 'Notification', { notification_type: 'permission_prompt', message: 'Allow Bash(npm test)?' }],
      [2500, 'PreToolUse', bash('npm test -- --run', 'Run the test suite')],
      [2000, 'PostToolUseFailure', {
        ...bash('npm test -- --run'),
        error: 'clock.test.js > formats in UTC\n  expected 03:00 to be 02:00'
      }],
      [1200, 'PreToolUse', file('Edit', 'C:/code/clockwork/src/clock.js')],
      [2200, 'PreToolUse', bash('npm test -- --run', 'Run the test suite')],
      [1500, 'PostToolUse', bash('npm test -- --run')],
      [800, 'Stop', {
        last_assistant_message: 'Fixed: the formatter used local time.\n\nAll 42 tests pass.'
      }],
      [6000, 'SessionEnd', { reason: 'clear' }]
    ]
  },
  {
    id: 'sim-orchestrator',
    cwd: 'C:/code/orchestrator',
    steps: [
      [1200, 'SessionStart', { source: 'startup' }],
      [900, 'UserPromptSubmit', { prompt: 'Add tier C eval scenarios and wire them into the suite' }],
      [1400, 'PreToolUse', file('Read', 'C:/code/orchestrator/evals/scenarios.json')],
      [1600, 'PreToolUse', file('Edit', 'C:/code/orchestrator/evals/scenarios.json')],
      [2000, 'PreToolUse', bash('pytest -q evals', 'Run the eval suite')],
      [3000, 'PostToolUse', bash('pytest -q evals')],
      [1200, 'Stop', {
        last_assistant_message: 'Added 12 tier C scenarios; the suite passes in 41s.'
      }],
      [9000, 'SessionEnd', { reason: 'clear' }]
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
      [3000, 'PreToolUse', file('Read', 'transcript.jsonl')],
      [9000, 'SessionEnd', { reason: 'clear' }]
    ]
  },
  {
    id: 'sim-website',
    cwd: 'C:/code/website',
    steps: [
      [2600, 'SessionStart', { source: 'startup' }],
      [800, 'UserPromptSubmit', { prompt: 'Rewrite the pricing page copy' }],
      [2200, 'PreToolUse', file('Edit', 'C:/code/website/app/pricing/page.tsx')],
      [2600, 'PreToolUse', { tool_name: 'mcp__vercel__deploy_preview', tool_input: {} }],
      [4000, 'Notification', { notification_type: 'idle_prompt' }],
      [12000, 'SessionEnd', { reason: 'clear' }]
    ]
  }
]

async function play(project) {
  for (const [delay, event, extra] of project.steps) {
    await fire(project.id, project.cwd, event, extra)
    console.log(`  ${project.id.padEnd(18)} ${event}${extra.tool_name ? ` (${extra.tool_name})` : ''}`)
    await sleep(delay)
  }
}

console.log(`replaying ${PROJECTS.length} concurrent projects through ${BIN}\n`)
await Promise.all(PROJECTS.map(play))
console.log('\ndone')
