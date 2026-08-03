// Upscales a pet atlas with nearest-neighbour so the frames can be eyeballed
// at a sane size.
//
//   node tools/preview.mjs [atlas.png] [scale] [out.png]
import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { encodePng } from './png.mjs'
import { decodePng } from './pngdec.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))
const src = process.argv[2] ?? resolve(HERE, '..', 'public', 'pets', 'pip', 'spritesheet.png')
const scale = Number(process.argv[3] ?? 4)
const dest = process.argv[4] ?? resolve(HERE, '..', 'assets', 'atlas-preview.png')

const { width, height, rgba } = decodePng(readFileSync(src))
const w = width * scale
const h = height * scale
const big = new Uint8Array(w * h * 4)
for (let y = 0; y < h; y++) {
  for (let x = 0; x < w; x++) {
    const si = (Math.floor(y / scale) * width + Math.floor(x / scale)) * 4
    const di = (y * w + x) * 4
    big[di] = rgba[si]
    big[di + 1] = rgba[si + 1]
    big[di + 2] = rgba[si + 2]
    big[di + 3] = rgba[si + 3]
  }
}
writeFileSync(dest, encodePng(w, h, big))
console.log(`wrote ${dest} (${w}x${h})`)
