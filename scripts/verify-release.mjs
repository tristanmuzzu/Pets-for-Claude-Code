// Refuses to build a release whose version numbers disagree.
//
// The version lives in three files that nothing keeps in step, plus the tag
// that triggers the build. Any two of them drifting produces an installer that
// reports the wrong version — which is only ever discovered later, by someone
// trying to work out which build they are running.
//
// Runs before the build job, not after, so a mismatch costs seconds.
import { readFileSync, existsSync } from 'node:fs'
import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const problems = []
const read = (relative) => readFileSync(resolve(root, relative), 'utf8')

const pkg = JSON.parse(read('package.json'))
const version = pkg.version

if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
  problems.push(`package.json version "${version}" is not a semantic version`)
}

const cargo = read('src-tauri/Cargo.toml').match(/^version\s*=\s*"([^"]+)"/m)?.[1]
if (cargo !== version) {
  problems.push(`src-tauri/Cargo.toml is ${cargo}, package.json is ${version}`)
}

const tauri = JSON.parse(read('src-tauri/tauri.conf.json')).version
if (tauri !== version) {
  problems.push(`src-tauri/tauri.conf.json is ${tauri}, package.json is ${version}`)
}

const notes = `docs/releases/v${version}.md`
if (!existsSync(resolve(root, notes))) {
  problems.push(`${notes} does not exist — a release needs notes before it needs a build`)
}

// Set by GitHub Actions on a tag push.
const ref = process.env.GITHUB_REF_NAME
if (ref && ref.startsWith('v') && ref !== `v${version}`) {
  problems.push(`tag ${ref} does not match version ${version}`)
}

if (problems.length) {
  console.error(`Release check failed for ${version}:`)
  for (const problem of problems) console.error(`  - ${problem}`)
  process.exit(1)
}

console.log(`Release check passed for ${version}.`)
