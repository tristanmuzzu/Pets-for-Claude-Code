// Builds assets/states.png: one representative frame per agent state, for the
// README. Reads the generated atlas so it can never drift from the real art.
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { encodePng } from './png.mjs'
import { decodePng } from './pngdec.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))
const pet = process.argv[2] ?? 'ember'
const ATLAS = resolve(HERE, '..', 'public', 'pets', pet, 'spritesheet.png')
const OUT = resolve(HERE, '..', 'assets', `states-${pet}.png`)

const FRAME = 48
const SCALE = 4
const GAP = 6
// [row, frame] — idle, running, waiting, failed, review, jumping.
const PICKS = [
  [0, 0],
  [7, 2],
  [6, 2],
  [5, 3],
  [8, 4],
  [4, 2]
]

const { width, rgba } = decodePng(readFileSync(ATLAS))
const cell = FRAME * SCALE
const outW = PICKS.length * cell + (PICKS.length - 1) * GAP * SCALE
const outH = cell
const out = new Uint8Array(outW * outH * 4)

PICKS.forEach(([row, frame], index) => {
  const originX = index * (cell + GAP * SCALE)
  for (let y = 0; y < cell; y++) {
    for (let x = 0; x < cell; x++) {
      const sx = frame * FRAME + Math.floor(x / SCALE)
      const sy = row * FRAME + Math.floor(y / SCALE)
      const si = (sy * width + sx) * 4
      const di = ((y * outW) + originX + x) * 4
      out[di] = rgba[si]
      out[di + 1] = rgba[si + 1]
      out[di + 2] = rgba[si + 2]
      out[di + 3] = rgba[si + 3]
    }
  }
})

mkdirSync(dirname(OUT), { recursive: true })
writeFileSync(OUT, encodePng(outW, outH, out))
console.log(`wrote ${OUT} (${outW}x${outH})`)
