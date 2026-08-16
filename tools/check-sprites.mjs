// Asserts that the committed atlases are what their generators produce.
//
// This used to be `npm run sprites && git diff --exit-code public/pets`, which
// compares *files*. The files are PNGs, and the last chunk of a PNG is a
// deflate stream: two zlib builds given identical pixels can, and do, emit
// different bytes for them. That check therefore passed on the machine the
// atlases happened to be generated on and failed everywhere else — the moment
// this repository started running CI on Linux as well as Windows, it went red
// on a pair of files whose pixels were identical to the byte.
//
// So compare the thing the claim is actually about. Regenerate, decode both
// sides, and compare the RGBA. A hand-edited sprite still fails, which is the
// point of the check; a different zlib does not, which never was.
//
// The generators write in place, so the committed bytes are put back when the
// pixels agree, leaving a clean working tree. When they disagree the
// regenerated files are left where they are, so `git diff` shows the damage
// and `tools/preview.mjs` can look at it.
import { execFileSync } from 'node:child_process'
import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { decodePng } from './pngdec.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))
const ROOT = resolve(HERE, '..')
const PETS = ['byte', 'pip', 'ember']
const GENERATORS = ['gen-sprites.mjs', 'gen-ember.mjs', 'gen-byte.mjs']

const atlas = (pet) => resolve(ROOT, 'public', 'pets', pet, 'spritesheet.png')
const manifest = (pet) => resolve(ROOT, 'public', 'pets', pet, 'pet.json')
// gen-ember.mjs writes the app icon on its way past, outside public/pets. It
// is generated from the same code and belongs in the same check; leaving it
// out meant a run of this script left one modified file behind.
const ICON = resolve(ROOT, 'assets', 'icon-source.png')

// Read every committed file before anything overwrites them.
const before = new Map()
for (const pet of PETS) {
  before.set(pet, { png: readFileSync(atlas(pet)), json: readFileSync(manifest(pet)) })
}
const iconBefore = readFileSync(ICON)

for (const generator of GENERATORS) {
  execFileSync(process.execPath, [resolve(HERE, generator)], { stdio: 'inherit' })
}

const problems = []

const pixelDiff = (name, committedBytes, regeneratedBytes) => {
  const a = decodePng(committedBytes)
  const b = decodePng(regeneratedBytes)
  if (a.width !== b.width || a.height !== b.height) {
    return `${name} is ${a.width}x${a.height}, the generator produces ${b.width}x${b.height}`
  }
  let differing = 0
  let first = -1
  for (let i = 0; i < a.rgba.length; i++) {
    if (a.rgba[i] !== b.rgba[i]) {
      differing++
      if (first < 0) first = i
    }
  }
  if (!differing) return null
  const pixel = Math.floor(first / 4)
  return (
    `${name} differs from the generator in ${differing} channel value(s), ` +
    `first at pixel (${pixel % a.width}, ${Math.floor(pixel / a.width)})`
  )
}

for (const pet of PETS) {
  const committed = before.get(pet)
  const regenerated = { png: readFileSync(atlas(pet)), json: readFileSync(manifest(pet)) }

  // The manifest is JSON, so bytes are the right comparison there.
  if (!committed.json.equals(regenerated.json)) {
    problems.push(`${pet}/pet.json differs from what ${GENERATORS} produce`)
  }

  const difference = pixelDiff(`${pet}/spritesheet.png`, committed.png, regenerated.png)
  if (difference) problems.push(difference)
}

const iconDifference = pixelDiff('assets/icon-source.png', iconBefore, readFileSync(ICON))
if (iconDifference) problems.push(iconDifference)

if (problems.length) {
  console.error('The committed atlases are not what the generators produce:\n')
  for (const problem of problems) console.error(`  - ${problem}`)
  console.error('\nRegenerated files left in place; `git diff public/pets` shows them.')
  process.exit(1)
}

// Identical pixels: restore the committed bytes so the tree is as it was.
for (const pet of PETS) {
  const committed = before.get(pet)
  writeFileSync(atlas(pet), committed.png)
  writeFileSync(manifest(pet), committed.json)
}
writeFileSync(ICON, iconBefore)
console.log(`Atlases match their generators, pixel for pixel (${PETS.join(', ')}, app icon).`)
