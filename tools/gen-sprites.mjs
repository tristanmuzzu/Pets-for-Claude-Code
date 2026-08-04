// Pip: an ember-fox with a floating status wisp.
//
// The art is generated rather than hand-painted so that every state stays
// consistent and re-theming is a palette edit instead of 63 redrawn frames.
// Shared primitives live in pixel.mjs; this file is only Pip's palette,
// proportions, and poses.
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  ROW_NAMES, Layer,
  drawEyes, drawWisp, drawLaptop, drawMagnifier, drawBang, drawSweat, drawSparkle,
  lookDirection, wave, writePet
} from './pixel.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))
const OUT_DIR = resolve(HERE, '..', 'public', 'pets', 'pip')

// --- palette ------------------------------------------------------------
const OUTLINE = [40, 26, 30, 255]
const BASE = [226, 132, 68, 255]
const SHADE = [178, 92, 48, 255]
const LIGHT = [247, 179, 116, 255]
const CREAM = [252, 226, 196, 255]
const EAR_INNER = [196, 106, 96, 255]

const EYES = {
  white: [253, 248, 240, 255],
  pupil: [40, 26, 30, 255],
  spread: 5,
  radius: 3,
  pupilRadius: 1.6
}
const PUPIL = EYES.pupil

// Wisp colour per row: the mascot's status light.
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

// --- geometry -----------------------------------------------------------
const CX = 21
const HEAD = { cy: 20, rx: 11, ry: 10 }
const BODY = { cy: 34, rx: 8, ry: 7 }
const FOOT = { cy: 44, rx: 4, ry: 2.5, dx: 6 }
const ARM = { cy: 33, rx: 3, ry: 4.5, dx: 11 }

/**
 * Quadratic bezier tail made of shrinking blobs. Built as its own layer so it
 * can be outlined and composited *behind* the body. Without that gap the tail
 * merges into the silhouette and reads as a raised arm.
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
  drawEyes(face, hx, hy - 1, p.eyes ?? 'open', EYES)
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

// --- poses --------------------------------------------------------------
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
  const colour = WISP[ROW_NAMES[row]] ?? WISP.idle
  switch (ROW_NAMES[row]) {
    // Sixteen directions: pupils lead, head follows by a pixel, wisp drifts
    // the other way like something being watched.
    case 'look-a':
    case 'look-b': {
      const { dx, dy } = lookDirection(row, f)
      return {
        wispColour: colour,
        wisp: { x: 38 + Math.round(dx * 2), y: 11 + Math.round(dy * 2), i: 0.85 },
        headDx: Math.round(dx),
        headDy: Math.round(dy),
        tail: -dx,
        eyes: { gaze: { dx: Math.round(dx * 1.8), dy: Math.round(dy * 1.8) } }
      }
    }
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
        props: [(l) => drawLaptop(l, f, CX)]
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

writePet({
  dir: OUT_DIR,
  id: 'pip',
  displayName: 'Pip',
  description: 'An ember-fox with a status wisp.',
  drawFrame: (row, f) => {
    const pose = poseFor(row, f)
    const frame = drawPip(pose)
    return pose.flip ? frame.flipped() : frame
  }
})
