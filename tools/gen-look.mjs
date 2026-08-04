// Builds assets/look-<pet>.png: the sixteen look directions in one strip.
//
//   node tools/gen-look.mjs [pet]
//
// Reads the generated atlas rather than redrawing anything, so the strip can
// never show a pose the pet does not actually have. Two rows of eight, laid out
// the way the contract orders them: clockwise from straight up.
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { encodePng } from './png.mjs'
import { decodePng } from './pngdec.mjs'
import { FRAME, LOOK_ROWS } from './pixel.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))
const pet = process.argv[2] ?? 'byte'
const ATLAS = resolve(HERE, '..', 'public', 'pets', pet, 'spritesheet.png')
const OUT = resolve(HERE, '..', 'assets', `look-${pet}.png`)

const SCALE = 3
const GAP = 4
const COLUMNS = 8

const { width, rgba } = decodePng(readFileSync(ATLAS))
const cell = FRAME * SCALE
const step = cell + GAP * SCALE
const outW = COLUMNS * cell + (COLUMNS - 1) * GAP * SCALE
const outH = LOOK_ROWS.length * cell + (LOOK_ROWS.length - 1) * GAP * SCALE
const out = new Uint8Array(outW * outH * 4)

LOOK_ROWS.forEach((row, rowIndex) => {
  for (let frame = 0; frame < COLUMNS; frame += 1) {
    const originX = frame * step
    const originY = rowIndex * step
    for (let y = 0; y < cell; y += 1) {
      for (let x = 0; x < cell; x += 1) {
        const sx = frame * FRAME + Math.floor(x / SCALE)
        const sy = row * FRAME + Math.floor(y / SCALE)
        const si = (sy * width + sx) * 4
        const di = ((originY + y) * outW + originX + x) * 4
        out[di] = rgba[si]
        out[di + 1] = rgba[si + 1]
        out[di + 2] = rgba[si + 2]
        out[di + 3] = rgba[si + 3]
      }
    }
  }
})

mkdirSync(dirname(OUT), { recursive: true })
writeFileSync(OUT, encodePng(outW, outH, out))
console.log(`wrote ${OUT} (${outW}x${outH})`)
