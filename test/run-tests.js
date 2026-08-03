// Every *.test.js in this folder, through Node's own test runner.
//
// No framework and no config file on purpose: the Rust side already has
// `cargo test`, and a second toolchain to keep working would cost more than
// these tests are worth.
import { readdirSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const files = readdirSync(here)
  .filter((name) => name.endsWith('.test.js'))
  .sort()
  .map((name) => resolve(here, name))

if (files.length === 0) {
  console.error('no tests found')
  process.exit(1)
}

const result = spawnSync(process.execPath, ['--test', ...files], { stdio: 'inherit' })
process.exit(result.status ?? 1)
