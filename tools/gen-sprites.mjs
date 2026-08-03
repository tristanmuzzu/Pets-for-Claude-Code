// Procedural pixel-art generator for Pip, the default Pipsqueak companion.
//
// The art is generated rather than hand-painted so that every state stays
// consistent and re-theming is a palette edit instead of 63 redrawn frames.
// The atlas is laid out exactly like the Codex pet contract (8 columns x 9
// rows, one row per state, same frame counts), so pets are interchangeable in
// both directions.
//
// Pip is a small ember-fox: big head, two ears, a curling tail, and a floating
// wisp whose colour is the agent's status light.
import { writeFileSync, mkdirSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { encodePng } from './png.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))
const OUT_DIR = resolve(HERE, '..', 'public', 'pets', 'pip')

const FRAME = 48
const COLS = 8
const ROWS = 9
const FRAME_COUNTS = [6, 8, 8, 4, 5, 8, 6, 6, 6]
const ROW_NAMES = [
  'idle', 'running-right', 'running-left', 'waving', 'jumping',
  'failed', 'waiting', 'running', 'review'
]

// --- palette -----------------------------------------------------------
const OUTLINE = [40, 26, 30, 255]
const BASE = [226, 132, 68, 255]
const SHADE = [178, 92, 48, 255]
const LIGHT = [247, 179, 116, 255]
const CREAM = [252, 226, 196, 255]
const EAR_INNER = [196, 106, 96, 255]
const EYE_WHITE = [253, 248, 240, 255]
const PUPIL = [40, 26, 30, 255]
const GLINT = [255, 255, 255, 255]
const METAL = [54, 56, 70, 255]
const METAL_LIGHT = [96, 100, 122, 255]
const SCREEN = [22, 24, 32, 255]
const SCREEN_TEXT = [126, 231, 135, 255]
const LENS = [186, 220, 244, 210]
const SWEAT = [126, 190, 240, 255]

// Wisp colour per row — the mascot's status light.
const WISP = {
  idle: [136, 148, 164, 255],
  'running-right': [86, 156, 250, 255],
  'running-left': [86, 156, 250, 255],
  waving: [74, 210, 128, 255],
  jumping: [178, 110, 250, 255],
  failed: [244, 88, 88, 255],
  waiting: [250, 146, 52, 255],
  running: [86, 156, 250, 255],
  review: [74, 210, 128, 255]
}

// --- geometry ----------------------------------------------------------
const CX = 21
const HEAD = { cy: 20, rx: 11, ry: 10 }
const BODY = { cy: 34, rx: 8, ry: 7 }
const FOOT = { cy: 44, rx: 4, ry: 2.5, dx: 6 }
const ARM = { cy: 33, rx: 3, ry: 4.5, dx: 11 }

class Layer {
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

  /** Alpha-blend rather than replace — used for glows. */
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

// --- features ----------------------------------------------------------
function drawEyes(layer, cx, cy, kind) {
  const lx = cx - 5
  const rx = cx + 5
  const eye = (ex, pdx, pdy) => {
    layer.circle(ex, cy, 3, EYE_WHITE)
    layer.circle(ex + pdx, cy + pdy, 1.6, PUPIL)
    layer.set(ex + pdx - 1, cy + pdy - 1, GLINT)
  }
  switch (kind) {
    case 'blink':
      layer.rect(lx - 2, cy, 5, 1, PUPIL)
      layer.rect(rx - 2, cy, 5, 1, PUPIL)
      break
    case 'happy':
      for (const ex of [lx, rx]) {
        layer.set(ex - 2, cy + 1, PUPIL)
        layer.set(ex - 1, cy, PUPIL)
        layer.set(ex, cy - 1, PUPIL)
        layer.set(ex + 1, cy, PUPIL)
        layer.set(ex + 2, cy + 1, PUPIL)
      }
      break
    case 'x':
      for (const ex of [lx, rx]) {
        for (let i = -2; i <= 2; i++) {
          layer.set(ex + i, cy + i, PUPIL)
          layer.set(ex + i, cy - i, PUPIL)
        }
      }
      break
    case 'look-l': eye(lx, -1, 0); eye(rx, -1, 0); break
    case 'look-r': eye(lx, 1, 0); eye(rx, 1, 0); break
    case 'look-d': eye(lx, 0, 1); eye(rx, 0, 1); break
    default: eye(lx, 0, 0); eye(rx, 0, 0)
  }
}

function drawMouth(layer, cx, cy, kind) {
  if (kind === 'none') return
  if (kind === 'o') { layer.circle(cx, cy, 1.6, PUPIL); return }
  if (kind === 'flat') { layer.rect(cx - 2, cy, 5, 1, PUPIL); return }
  // Small fox muzzle: a nose with a two-sided smile under it.
  layer.rect(cx - 1, cy - 2, 3, 2, PUPIL)
  layer.set(cx, cy, PUPIL)
  layer.set(cx - 1, cy + 1, PUPIL)
  layer.set(cx - 2, cy + 1, PUPIL)
  layer.set(cx - 3, cy, PUPIL)
  layer.set(cx + 1, cy + 1, PUPIL)
  layer.set(cx + 2, cy + 1, PUPIL)
  layer.set(cx + 3, cy, PUPIL)
}

/**
 * Quadratic bezier tail made of shrinking blobs. Returned as its own layer so
 * it can be outlined and composited *behind* the body — without that gap the
 * tail merges into the silhouette and reads as a raised arm.
 */
function tailLayer(swish, bodyDy = 0) {
  const layer = new Layer()
  const lift = bodyDy * 0.7
  const p0 = [CX + 5, 41 + lift]
  const p1 = [CX + 16 + swish, 39 + swish + lift]
  const p2 = [CX + 17 + swish * 2, 26 + swish + lift]
  const steps = 10
  for (let i = 0; i <= steps; i++) {
    const t = i / steps
    const mt = 1 - t
    const x = mt * mt * p0[0] + 2 * mt * t * p1[0] + t * t * p2[0]
    const y = mt * mt * p0[1] + 2 * mt * t * p1[1] + t * t * p2[1]
    layer.circle(x, y, 4.6 - t * 2.2, t > 0.72 ? CREAM : BASE)
  }
  layer.shade(LIGHT, SHADE)
  layer.outline(OUTLINE)
  return layer
}

/** Radial falloff, white-hot core — a hard-edged disc reads as a UI chip, not a glow. */
function drawWisp(layer, x, y, colour, intensity = 1) {
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

const GLYPHS = ['.#####..', '.#..###.', '.####...', '.##..##.', '.#####..', '.###....']

function drawLaptop(layer, frame) {
  layer.rect(13, 28, 17, 10, METAL)
  layer.rect(15, 30, 13, 6, SCREEN)
  for (const [dy, shift] of [[0, 0], [2, 3]]) {
    const row = GLYPHS[(frame + shift) % GLYPHS.length]
    for (let i = 0; i < row.length; i++) if (row[i] === '#') layer.set(16 + i, 31 + dy, SCREEN_TEXT)
  }
  layer.rect(11, 38, 21, 4, METAL_LIGHT)
  layer.rect(12, 39, 19, 1, METAL)
}

function drawMagnifier(layer, x, y) {
  layer.circle(x, y, 4.6, METAL_LIGHT)
  layer.circle(x, y, 3.4, LENS)
  for (let i = 0; i < 4; i++) layer.set(x + 4 + i, y + 4 + i, METAL)
}

const BANG = ['.##.', '.##.', '.##.', '.##.', '.##.', '....', '.##.']
const drawBang = (layer, x, y, colour) => layer.stamp(x, y, BANG, { '#': colour })

function drawSweat(layer, x, y) {
  layer.set(x, y, SWEAT)
  layer.rect(x - 1, y + 1, 3, 2, SWEAT)
  layer.set(x, y + 3, SWEAT)
}

function drawSparkle(layer, x, y, size) {
  layer.set(x, y, GLINT)
  for (let i = 1; i <= size; i++) {
    layer.set(x + i, y, GLINT)
    layer.set(x - i, y, GLINT)
    layer.set(x, y + i, GLINT)
    layer.set(x, y - i, GLINT)
  }
}

// --- character ---------------------------------------------------------
function drawPip(p) {
  const sil = new Layer()
  const squash = p.squash ?? 1
  const bodyDy = p.bodyDy ?? 0
  const headDy = (p.headDy ?? 0) + bodyDy
  const headDx = p.headDx ?? 0
  const legs = p.legs ?? [{ dx: 0, dy: 0 }, { dx: 0, dy: 0 }]
  const arms = p.arms ?? [{ dx: 0, dy: 0 }, { dx: 0, dy: 0 }]
  const hx = CX + headDx
  const hy = HEAD.cy + headDy

  legs.forEach((leg, i) => {
    const side = i === 0 ? -1 : 1
    sil.ellipse(CX + side * FOOT.dx + leg.dx, FOOT.cy + leg.dy, FOOT.rx, FOOT.ry, BASE)
  })
  arms.forEach((arm, i) => {
    const side = i === 0 ? -1 : 1
    sil.ellipse(CX + side * ARM.dx + arm.dx, ARM.cy + arm.dy + bodyDy, ARM.rx, ARM.ry, BASE)
  })
  sil.ellipse(CX, BODY.cy + bodyDy, BODY.rx, BODY.ry * squash, BASE)

  // Ears before the head so the head's shading owns the join.
  const earLift = p.earLift ?? 0
  sil.triangle([hx - 11, hy - 4], [hx - 3, hy - 8], [hx - 9, hy - 17 - earLift], BASE)
  sil.triangle([hx + 11, hy - 4], [hx + 3, hy - 8], [hx + 9, hy - 17 - earLift], BASE)
  sil.ellipse(hx, hy, HEAD.rx, HEAD.ry, BASE)

  sil.shade(LIGHT, SHADE)
  const mask = sil.clone()
  sil.outline(OUTLINE)

  const marks = new Layer()
  marks.triangle([hx - 9, hy - 5], [hx - 5, hy - 8], [hx - 8, hy - 14 - earLift], EAR_INNER)
  marks.triangle([hx + 9, hy - 5], [hx + 5, hy - 8], [hx + 8, hy - 14 - earLift], EAR_INNER)
  marks.ellipse(CX, BODY.cy + bodyDy + 1, 5, 5 * squash, CREAM)
  marks.ellipse(hx, hy + 4, 6, 4, CREAM)
  marks.maskBy(mask)

  const face = new Layer()
  drawEyes(face, hx, hy - 1, p.eyes ?? 'open')
  drawMouth(face, hx, hy + 5, p.mouth ?? 'smile')

  const props = new Layer()
  for (const prop of p.props ?? []) prop(props)
  props.outline(OUTLINE)

  const out = new Layer()
  out.blit(tailLayer(p.tail ?? 0, bodyDy))
  out.blit(sil)
  out.blit(marks)
  out.blit(face)
  out.blit(props)
  // The wisp glows over everything, unoutlined.
  const w = p.wisp ?? { x: 38, y: 12, i: 1 }
  drawWisp(out, w.x, w.y, p.wispColour, w.i)
  return out
}

// --- poses -------------------------------------------------------------
const wave = (f, n, amp) => Math.round(Math.sin((f / n) * Math.PI * 2) * amp)

function walkPose(f, n, colour) {
  const swing = Math.sin((f / n) * Math.PI * 2)
  return {
    wispColour: colour,
    wisp: { x: 38 - swing * 2, y: 11 + swing, i: 1 },
    headDx: 1,
    bodyDy: f % 2 ? -1 : 0,
    tail: 1 + swing * 1.5,
    eyes: 'look-r',
    legs: [
      { dx: Math.round(swing * 3), dy: -Math.max(0, Math.round(swing * 2)) },
      { dx: Math.round(-swing * 3), dy: -Math.max(0, Math.round(-swing * 2)) }
    ],
    arms: [
      { dx: Math.round(-swing * 2), dy: Math.round(swing * 2) },
      { dx: Math.round(swing * 2), dy: Math.round(-swing * 2) }
    ]
  }
}

function poseFor(row, f) {
  const colour = WISP[ROW_NAMES[row]]
  switch (ROW_NAMES[row]) {
    case 'idle': {
      const bob = wave(f, 6, 1)
      return {
        wispColour: colour,
        wisp: { x: 38, y: 11 + bob, i: 0.85 },
        headDy: bob,
        tail: bob * 0.8,
        squash: 1 - bob * 0.03,
        eyes: f === 4 ? 'blink' : 'open'
      }
    }
    case 'running-right':
      return walkPose(f, 8, colour)
    case 'running-left':
      return { ...walkPose(f, 8, colour), flip: true }
    case 'waving': {
      const lift = [0, -1, 0, -1][f]
      return {
        wispColour: colour,
        wisp: { x: [38, 40, 41, 39][f], y: [11, 8, 6, 9][f], i: 1 },
        bodyDy: lift,
        earLift: 1,
        tail: 2,
        eyes: 'happy',
        // Waves with the left arm: the tail owns the right side of the frame.
        arms: [{ dx: -[1, 2, 3, 2][f], dy: [-6, -9, -10, -9][f] }, { dx: 0, dy: 0 }]
      }
    }
    case 'jumping': {
      const dy = [2, -4, -8, -3, 1][f]
      const airborne = f > 0 && f < 4
      return {
        wispColour: colour,
        wisp: { x: 38, y: 14 + dy * 0.5, i: 1 },
        bodyDy: dy,
        squash: [0.82, 1.08, 1.02, 1.0, 0.88][f],
        tail: airborne ? 2.5 : 0,
        eyes: f === 2 ? 'happy' : 'open',
        mouth: 'o',
        arms: [{ dx: -1, dy: airborne ? -6 : 0 }, { dx: 1, dy: airborne ? -6 : 0 }],
        legs: [{ dx: 0, dy: airborne ? -2 : 0 }, { dx: 0, dy: airborne ? -2 : 0 }]
      }
    }
    case 'failed': {
      const shake = [-1, 1, -1, 1, 0, -1, 1, 0][f]
      return {
        wispColour: colour,
        wisp: { x: 38 + shake, y: 12, i: f % 2 ? 0.35 : 1 },
        headDx: shake,
        earLift: -2,
        tail: -1,
        eyes: 'x',
        mouth: 'o',
        arms: [{ dx: -2, dy: -3 }, { dx: 2, dy: -3 }],
        props: [(l) => drawSweat(l, 9, 7 + f * 2)]
      }
    }
    case 'waiting': {
      const tilt = [0, 1, 1, 0, -1, -1][f]
      const hop = f % 3 === 0 ? 1 : 0
      return {
        wispColour: colour,
        wisp: { x: 38, y: 11 - hop * 2, i: 1 },
        headDx: tilt,
        tail: tilt,
        eyes: f < 3 ? 'look-l' : 'look-r',
        mouth: 'flat',
        legs: [{ dx: 0, dy: 0 }, { dx: 0, dy: f % 2 ? -2 : 0 }],
        props: [(l) => drawBang(l, CX - 2, 2 - hop, colour)]
      }
    }
    case 'running': {
      const type = f % 2 ? -1 : 0
      const angle = (f / 6) * Math.PI * 2
      return {
        wispColour: colour,
        wisp: { x: 38 + Math.cos(angle) * 3, y: 12 + Math.sin(angle) * 2, i: 1 },
        tail: 1 + Math.sin(angle),
        eyes: 'look-d',
        mouth: 'flat',
        arms: [{ dx: 3, dy: 2 + type }, { dx: -3, dy: 2 - type }],
        props: [(l) => drawLaptop(l, f)]
      }
    }
    case 'review': {
      const x = 9 - [0, 1, 2, 2, 1, 0][f]
      const props = [(l) => drawMagnifier(l, x, 26)]
      if (f >= 3) props.push((l) => drawSparkle(l, 14, 10, f - 2))
      return {
        wispColour: colour,
        wisp: { x: 38, y: 10, i: 1 },
        earLift: 1,
        tail: 1.5,
        eyes: f >= 3 ? 'happy' : 'open',
        arms: [{ dx: -2, dy: -4 }, { dx: 0, dy: 0 }],
        props
      }
    }
    default:
      return { wispColour: colour }
  }
}

// --- assemble ----------------------------------------------------------
const atlas = new Layer(COLS * FRAME, ROWS * FRAME)
for (let row = 0; row < ROWS; row++) {
  for (let f = 0; f < FRAME_COUNTS[row]; f++) {
    const pose = poseFor(row, f)
    let frame = drawPip(pose)
    if (pose.flip) frame = frame.flipped()
    atlas.blit(frame, f * FRAME, row * FRAME)
  }
}

mkdirSync(OUT_DIR, { recursive: true })
writeFileSync(resolve(OUT_DIR, 'spritesheet.png'), encodePng(atlas.w, atlas.h, atlas.d))
writeFileSync(
  resolve(OUT_DIR, 'pet.json'),
  JSON.stringify(
    {
      id: 'pip',
      displayName: 'Pip',
      description: 'An ember-fox with a status wisp. The default Pipsqueak companion.',
      spritesheetPath: 'spritesheet.png',
      frameWidth: FRAME,
      frameHeight: FRAME,
      columns: COLS,
      rows: ROWS,
      frameCounts: FRAME_COUNTS,
      fps: 8,
      author: 'Pipsqueak',
      license: 'MIT'
    },
    null,
    2
  ) + '\n'
)

// App icon: idle frame, nearest-neighbour upscaled onto a 512px canvas.
const ICON = 512
const SCALE = 10
const icon = new Layer(ICON, ICON)
const iconSrc = drawPip(poseFor(0, 0))
const offset = Math.round((ICON - FRAME * SCALE) / 2)
for (let y = 0; y < FRAME; y++) {
  for (let x = 0; x < FRAME; x++) {
    const a = iconSrc.alphaAt(x, y)
    if (!a) continue
    const i = (y * FRAME + x) * 4
    icon.rect(offset + x * SCALE, offset + y * SCALE, SCALE, SCALE, [
      iconSrc.d[i], iconSrc.d[i + 1], iconSrc.d[i + 2], a
    ])
  }
}
mkdirSync(resolve(HERE, '..', 'assets'), { recursive: true })
writeFileSync(resolve(HERE, '..', 'assets', 'icon-source.png'), encodePng(ICON, ICON, icon.d))

console.log(`wrote ${OUT_DIR}\\spritesheet.png (${atlas.w}x${atlas.h}) and assets\\icon-source.png`)
