// Shared pixel-art toolkit for the built-in pets.
//
// Everything here is character-agnostic: a tiny raster layer with the drawing
// primitives, the props that appear in several states, and the atlas writer.
// Each pet supplies its own palette, geometry, and poses.
import { writeFileSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { encodePng } from './png.mjs'

export const FRAME = 48
export const COLS = 8
export const ROWS = 9
/** Matches the Codex pet contract exactly, so pets are interchangeable. */
export const FRAME_COUNTS = [6, 8, 8, 4, 5, 8, 6, 6, 6]

/**
 * How long each frame is held, in milliseconds.
 *
 * Even timing is what makes a six-frame loop read as a metronome. Holding the
 * frames where the pet is at rest and snapping through the middle is what makes
 * the same six frames read as breathing, which matters here, because the pet
 * spends most of its life on one of these loops in the corner of someone's eye.
 *
 * Rows follow ROW_NAMES below.
 */
export const FRAME_DURATIONS = [
  // Idle: a long settle, then a quick blink and back.
  [520, 120, 90, 90, 120, 420],
  // Walking is a gait: even, or it limps.
  [110, 110, 110, 110, 110, 110, 110, 110],
  [110, 110, 110, 110, 110, 110, 110, 110],
  // A wave lands on the up-beat and holds there.
  [110, 90, 260, 110],
  // Anticipate, launch, hang, land.
  [150, 80, 90, 260, 200],
  // A stumble: fast, then a long recovery.
  [90, 90, 90, 90, 110, 140, 200, 380],
  // Waiting has to stay legible from across the room, so it stays slow.
  [300, 220, 300, 220, 300, 380],
  // Working: busy, with one held frame so it does not blur into itself.
  [110, 110, 110, 200, 110, 110],
  [180, 140, 140, 260, 140, 180]
]
export const ROW_NAMES = [
  'idle', 'running-right', 'running-left', 'waving', 'jumping',
  'failed', 'waiting', 'running', 'review'
]

/** Colours for things that are not part of the character. */
export const PROP = {
  metal: [54, 56, 70, 255],
  metalLight: [96, 100, 122, 255],
  screen: [22, 24, 32, 255],
  screenText: [126, 231, 135, 255],
  lens: [186, 220, 244, 210],
  sweat: [126, 190, 240, 255],
  glint: [255, 255, 255, 255]
}

export class Layer {
  constructor(w = FRAME, h = FRAME) {
    this.w = w
    this.h = h
    this.d = new Uint8Array(w * h * 4)
  }

  alphaAt(x, y) {
    if (x < 0 || y < 0 || x >= this.w || y >= this.h) return 0
    return this.d[(y * this.w + x) * 4 + 3]
  }

  set(x, y, c) {
    x = Math.round(x)
    y = Math.round(y)
    if (x < 0 || y < 0 || x >= this.w || y >= this.h) return
    const i = (y * this.w + x) * 4
    this.d[i] = c[0]
    this.d[i + 1] = c[1]
    this.d[i + 2] = c[2]
    this.d[i + 3] = c[3] === undefined ? 255 : c[3]
  }

  /** Alpha-blend rather than replace. Used for glows. */
  blend(x, y, c) {
    x = Math.round(x)
    y = Math.round(y)
    if (x < 0 || y < 0 || x >= this.w || y >= this.h) return
    const i = (y * this.w + x) * 4
    const a = (c[3] === undefined ? 255 : c[3]) / 255
    const dst = this.d[i + 3] / 255
    const out = a + dst * (1 - a)
    if (out <= 0) return
    for (let k = 0; k < 3; k++) {
      this.d[i + k] = Math.round((c[k] * a + this.d[i + k] * dst * (1 - a)) / out)
    }
    this.d[i + 3] = Math.round(out * 255)
  }

  rect(x, y, w, h, c) {
    for (let yy = y; yy < y + h; yy++) for (let xx = x; xx < x + w; xx++) this.set(xx, yy, c)
  }

  circle(cx, cy, r, c) {
    const lim = r * r + r * 0.35
    const ri = Math.ceil(r) + 1
    for (let dy = -ri; dy <= ri; dy++) {
      for (let dx = -ri; dx <= ri; dx++) {
        if (dx * dx + dy * dy <= lim) this.set(cx + dx, cy + dy, c)
      }
    }
  }

  ellipse(cx, cy, rx, ry, c) {
    for (let dy = -Math.ceil(ry); dy <= Math.ceil(ry); dy++) {
      for (let dx = -Math.ceil(rx); dx <= Math.ceil(rx); dx++) {
        if ((dx * dx) / (rx * rx) + (dy * dy) / (ry * ry) <= 1.06) this.set(cx + dx, cy + dy, c)
      }
    }
  }

  /** Rounded rectangle: the base shape for blockier characters. */
  roundRect(x, y, w, h, r, c) {
    for (let yy = 0; yy < h; yy++) {
      for (let xx = 0; xx < w; xx++) {
        const dx = Math.max(r - xx, xx - (w - 1 - r), 0)
        const dy = Math.max(r - yy, yy - (h - 1 - r), 0)
        if (dx * dx + dy * dy <= r * r + r * 0.35) this.set(x + xx, y + yy, c)
      }
    }
  }

  triangle(a, b, c, colour) {
    const minX = Math.floor(Math.min(a[0], b[0], c[0]))
    const maxX = Math.ceil(Math.max(a[0], b[0], c[0]))
    const minY = Math.floor(Math.min(a[1], b[1], c[1]))
    const maxY = Math.ceil(Math.max(a[1], b[1], c[1]))
    const edge = (p, q, x, y) => (q[0] - p[0]) * (y - p[1]) - (q[1] - p[1]) * (x - p[0])
    for (let y = minY; y <= maxY; y++) {
      for (let x = minX; x <= maxX; x++) {
        const w0 = edge(a, b, x, y)
        const w1 = edge(b, c, x, y)
        const w2 = edge(c, a, x, y)
        if ((w0 >= 0 && w1 >= 0 && w2 >= 0) || (w0 <= 0 && w1 <= 0 && w2 <= 0)) this.set(x, y, colour)
      }
    }
  }

  stamp(x, y, rows, map) {
    rows.forEach((row, dy) => {
      for (let dx = 0; dx < row.length; dx++) {
        const c = map[row[dx]]
        if (c) this.set(x + dx, y + dy, c)
      }
    })
  }

  clone() {
    const l = new Layer(this.w, this.h)
    l.d.set(this.d)
    return l
  }

  /** Cheap directional shading: lit near the top edge, darkened near the bottom. */
  shade(light, dark) {
    const snap = this.clone()
    for (let y = 0; y < this.h; y++) {
      for (let x = 0; x < this.w; x++) {
        if (!snap.alphaAt(x, y)) continue
        if (!snap.alphaAt(x, y - 2)) this.set(x, y, light)
        else if (!snap.alphaAt(x, y + 2) || !snap.alphaAt(x + 2, y)) this.set(x, y, dark)
      }
    }
  }

  outline(c) {
    const snap = this.clone()
    for (let y = 0; y < this.h; y++) {
      for (let x = 0; x < this.w; x++) {
        if (snap.alphaAt(x, y)) continue
        if (
          snap.alphaAt(x - 1, y) || snap.alphaAt(x + 1, y) ||
          snap.alphaAt(x, y - 1) || snap.alphaAt(x, y + 1)
        ) this.set(x, y, c)
      }
    }
  }

  maskBy(mask) {
    for (let i = 0; i < this.d.length; i += 4) if (!mask.d[i + 3]) this.d[i + 3] = 0
  }

  blit(src, dx = 0, dy = 0) {
    for (let y = 0; y < src.h; y++) {
      for (let x = 0; x < src.w; x++) {
        const a = src.alphaAt(x, y)
        if (!a) continue
        const i = (y * src.w + x) * 4
        if (a === 255) this.set(x + dx, y + dy, [src.d[i], src.d[i + 1], src.d[i + 2], 255])
        else this.blend(x + dx, y + dy, [src.d[i], src.d[i + 1], src.d[i + 2], a])
      }
    }
  }

  flipped() {
    const l = new Layer(this.w, this.h)
    for (let y = 0; y < this.h; y++) {
      for (let x = 0; x < this.w; x++) {
        const i = (y * this.w + x) * 4
        l.set(this.w - 1 - x, y, [this.d[i], this.d[i + 1], this.d[i + 2], this.d[i + 3]])
      }
    }
    return l
  }
}

// --- face ---------------------------------------------------------------
export function drawEyes(layer, cx, cy, kind, palette) {
  const { white, pupil, glint = PROP.glint } = palette
  const lx = cx - (palette.spread ?? 5)
  const rx = cx + (palette.spread ?? 5)
  const r = palette.radius ?? 3
  const pupilRadius = palette.pupilRadius ?? 1.6
  const eye = (ex, pdx, pdy) => {
    layer.circle(ex, cy, r, white)
    layer.circle(ex + pdx, cy + pdy, pupilRadius, pupil)
    layer.set(ex + pdx - 1, cy + pdy - 1, glint)
  }
  switch (kind) {
    case 'blink':
      layer.rect(lx - 2, cy, 5, 1, pupil)
      layer.rect(rx - 2, cy, 5, 1, pupil)
      break
    case 'happy':
      for (const ex of [lx, rx]) {
        layer.set(ex - 2, cy + 1, pupil)
        layer.set(ex - 1, cy, pupil)
        layer.set(ex, cy - 1, pupil)
        layer.set(ex + 1, cy, pupil)
        layer.set(ex + 2, cy + 1, pupil)
      }
      break
    case 'x':
      for (const ex of [lx, rx]) {
        for (let i = -2; i <= 2; i++) {
          layer.set(ex + i, cy + i, pupil)
          layer.set(ex + i, cy - i, pupil)
        }
      }
      break
    case 'look-l': eye(lx, -1, 0); eye(rx, -1, 0); break
    case 'look-r': eye(lx, 1, 0); eye(rx, 1, 0); break
    case 'look-d': eye(lx, 0, 1); eye(rx, 0, 1); break
    default: eye(lx, 0, 0); eye(rx, 0, 0)
  }
}

/** Radial falloff, white-hot core. A hard-edged disc reads as a UI chip. */
export function drawWisp(layer, x, y, colour, intensity = 1) {
  const R = 4.8
  for (let dy = -Math.ceil(R); dy <= Math.ceil(R); dy++) {
    for (let dx = -Math.ceil(R); dx <= Math.ceil(R); dx++) {
      const d = Math.hypot(dx, dy)
      if (d > R) continue
      const falloff = Math.pow(1 - d / R, 1.1)
      const core = Math.max(0, 1 - d / 1.8)
      const alpha = Math.round(Math.min(255, (255 * falloff + core * 150) * intensity))
      if (alpha <= 6) continue
      layer.blend(x + dx, y + dy, [
        Math.round(colour[0] + (255 - colour[0]) * core),
        Math.round(colour[1] + (255 - colour[1]) * core),
        Math.round(colour[2] + (255 - colour[2]) * core),
        alpha
      ])
    }
  }
}

// --- props --------------------------------------------------------------
const GLYPHS = ['.#####..', '.#..###.', '.####...', '.##..##.', '.#####..', '.###....']

export function drawLaptop(layer, frame, cx = 21) {
  layer.rect(cx - 8, 28, 17, 10, PROP.metal)
  layer.rect(cx - 6, 30, 13, 6, PROP.screen)
  for (const [dy, shift] of [[0, 0], [2, 3]]) {
    const row = GLYPHS[(frame + shift) % GLYPHS.length]
    for (let i = 0; i < row.length; i++) {
      if (row[i] === '#') layer.set(cx - 5 + i, 31 + dy, PROP.screenText)
    }
  }
  layer.rect(cx - 10, 38, 21, 4, PROP.metalLight)
  layer.rect(cx - 9, 39, 19, 1, PROP.metal)
}

export function drawMagnifier(layer, x, y) {
  layer.circle(x, y, 4.6, PROP.metalLight)
  layer.circle(x, y, 3.4, PROP.lens)
  for (let i = 0; i < 4; i++) layer.set(x + 4 + i, y + 4 + i, PROP.metal)
}

const BANG = ['.##.', '.##.', '.##.', '.##.', '.##.', '....', '.##.']
export const drawBang = (layer, x, y, colour) => layer.stamp(x, y, BANG, { '#': colour })

export function drawSweat(layer, x, y) {
  layer.set(x, y, PROP.sweat)
  layer.rect(x - 1, y + 1, 3, 2, PROP.sweat)
  layer.set(x, y + 3, PROP.sweat)
}

export function drawSparkle(layer, x, y, size) {
  layer.set(x, y, PROP.glint)
  for (let i = 1; i <= size; i++) {
    layer.set(x + i, y, PROP.glint)
    layer.set(x - i, y, PROP.glint)
    layer.set(x, y + i, PROP.glint)
    layer.set(x, y - i, PROP.glint)
  }
}

export const wave = (f, n, amp) => Math.round(Math.sin((f / n) * Math.PI * 2) * amp)

// --- output -------------------------------------------------------------
/**
 * Renders every frame through `drawFrame(row, frameIndex)` and writes the atlas
 * plus its manifest.
 */
export function writePet({ dir, id, displayName, description, drawFrame, iconPath }) {
  const atlas = new Layer(COLS * FRAME, ROWS * FRAME)
  for (let row = 0; row < ROWS; row++) {
    for (let f = 0; f < FRAME_COUNTS[row]; f++) {
      atlas.blit(drawFrame(row, f), f * FRAME, row * FRAME)
    }
  }

  mkdirSync(dir, { recursive: true })
  writeFileSync(resolve(dir, 'spritesheet.png'), encodePng(atlas.w, atlas.h, atlas.d))
  writeFileSync(
    resolve(dir, 'pet.json'),
    JSON.stringify(
      {
        id,
        displayName,
        description,
        spritesheetPath: 'spritesheet.png',
        frameWidth: FRAME,
        frameHeight: FRAME,
        columns: COLS,
        rows: ROWS,
        frameCounts: FRAME_COUNTS,
        frameDurations: FRAME_DURATIONS,
        /** The fallback for any row a pet does not time explicitly. */
        fps: 8,
        author: 'Pipsqueak',
        license: 'MIT'
      },
      null,
      2
    ) + '\n'
  )

  if (iconPath) {
    const ICON = 512
    const SCALE = 10
    const icon = new Layer(ICON, ICON)
    const source = drawFrame(0, 0)
    const offset = Math.round((ICON - FRAME * SCALE) / 2)
    for (let y = 0; y < FRAME; y++) {
      for (let x = 0; x < FRAME; x++) {
        const a = source.alphaAt(x, y)
        if (!a) continue
        const i = (y * FRAME + x) * 4
        icon.rect(offset + x * SCALE, offset + y * SCALE, SCALE, SCALE, [
          source.d[i], source.d[i + 1], source.d[i + 2], a
        ])
      }
    }
    writeFileSync(iconPath, encodePng(ICON, ICON, icon.d))
  }

  console.log(`wrote ${dir}`)
}
