// Replays a realistic Claude Code hook sequence against the installed binary,
// so the overlay can be exercised without starting a real agent session.
//
//   node tools/simulate.mjs [path-to-pipsqueak-binary]
//
// Defaults to the debug build. Each step prints what it sent, so a mismatch
// between "what the hook said" and "what the pet shows" is obvious.
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

const SESSION = 'simulated-session'
const CWD = process.cwd()

const base = { session_id: SESSION, cwd: CWD, transcript_path: '', permission_mode: 'default' }

const SCRIPT = [
  [400, 'SessionStart', { source: 'startup' }],
  [900, 'UserPromptSubmit', { prompt: 'Fix the flaky timezone test' }],
  [1200, 'PreToolUse', { tool_name: 'Grep', tool_input: { pattern: 'toISOString' } }],
  [900, 'PostToolUse', { tool_name: 'Grep', tool_input: { pattern: 'toISOString' } }],
  [900, 'PreToolUse', { tool_name: 'Read', tool_input: { file_path: `${CWD}/src/clock.test.js` } }],
  [1400, 'PreToolUse', { tool_name: 'Edit', tool_input: { file_path: `${CWD}/src/clock.js` } }],
  [1200, 'PermissionRequest', { tool_name: 'Bash', tool_input: { command: 'npm test -- --run' } }],
  [2600, 'PreToolUse', {
    tool_name: 'Bash',
    tool_input: { command: 'npm test -- --run', description: 'Run the test suite' }
  }],
  [2000, 'PostToolUseFailure', {
    tool_name: 'Bash',
    tool_input: { command: 'npm test -- --run' },
    error: 'clock.test.js > formats in UTC\n  expected 03:00 to be 02:00'
  }],
  [1800, 'PreToolUse', { tool_name: 'Edit', tool_input: { file_path: `${CWD}/src/clock.js` } }],
  [1600, 'PreToolUse', {
    tool_name: 'Bash',
    tool_input: { command: 'npm test -- --run', description: 'Run the test suite' }
  }],
  [2200, 'PostToolUse', { tool_name: 'Bash', tool_input: { command: 'npm test -- --run' } }],
  [1200, 'Stop', {
    last_assistant_message: 'Fixed: the formatter used local time.\n\nAll 42 tests pass.'
  }],
  [4000, 'SessionEnd', { reason: 'clear' }]
]

const sleep = (ms) => new Promise((done) => setTimeout(done, ms))

function fire(event, extra) {
  return new Promise((done, fail) => {
    const child = spawn(BIN, ['hook', event], { stdio: ['pipe', 'ignore', 'inherit'] })
    child.on('error', fail)
    child.on('exit', () => done())
    child.stdin.end(JSON.stringify({ ...base, hook_event_name: event, ...extra }))
  })
}

console.log(`replaying ${SCRIPT.length} events through ${BIN}\n`)
for (const [delay, event, extra] of SCRIPT) {
  await fire(event, extra)
  const label = extra.tool_name ? `${event} (${extra.tool_name})` : event
  console.log(`  ${label}`)
  await sleep(delay)
}
console.log('\ndone')
