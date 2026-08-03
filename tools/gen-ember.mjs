// Ember: the default Pipsqueak companion.
//
// A soft clay pebble with a spark that floats beside it. Deliberately flat,
// rounded and minimal so it sits next to Claude Code without clashing — an
// original character, not anyone's logo.
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  FRAME, ROW_NAMES, Layer, PROP,
  drawEyes, drawWisp, drawLaptop, drawMagnifier, drawBang, drawSweat, drawSparkle,
  wave, writePet
} from './pixel.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))
const OUT_DIR = resolve(HERE, '..', 'public', 'pets', 'ember')

// --- palette ------------------------------------------------------------
const INK = [51, 33, 28, 255]
const BASE = [219, 127, 85, 255]
const SHADE = [178, 96, 60, 255]
const LIGHT = [240, 164, 125, 255]
const CREAM = [247, 228, 212, 255]
const BLUSH = [206, 108, 78, 255]

const EYES = { white: INK, pupil: INK, glint: [255, 240, 232, 255], spread: 5, radius: 3, pupilRadius: 1.1 }

const SPARK = {
  idle: [150, 140, 132, 255],
  'running-right': [96, 160, 250, 255],
  'running-left': [96, 160, 250, 255],
  waving: [80, 208, 132, 255],
  jumping: [176, 116, 246, 255],
  failed: [240, 92, 92, 255],
  waiting: [250, 152, 60, 255],
  running: [96, 160, 250, 255],
  review: [80, 208, 132, 255]
}

// --- geometry -----------------------------------------------------------
const CX = 21
const BODY = { x: 10, y: 11, w: 23, h: 30, r: 10 }
const FOOT = { y: 40, w: 7, h: 5, r: 2, dx: 6 }
const ARM = { cy: 29, rx: 3, ry: 4.5, dx: 12 }

function drawEmber(p) {
  const sil = new Layer()
  const bodyDy = p.bodyDy ?? 0
  const squash = p.squash ?? 1
  const lean = p.lean ?? 0
  const arms = p.arms ?? [{ dx: 0, dy: 0 }, { dx: 0, dy: 0 }]
  const feet = p.feet ?? [{ dx: 0, dy: 0 }, { dx: 0, dy: 0 }]

  feet.forEach((foot, i) => {
    const side = i === 0 ? -1 : 1
    sil.roundRect(
      Math.round(CX + side * FOOT.dx - FOOT.w / 2 + foot.dx),
      FOOT.y + foot.dy,
      FOOT.w,
      FOOT.h,
      FOOT.r,
      BASE
    )
  })
  arms.forEach((arm, i) => {
    const side = i === 0 ? -1 : 1
    sil.ellipse(CX + side * ARM.dx + arm.dx, ARM.cy + arm.dy + bodyDy, ARM.rx, ARM.ry, BASE)
  })

  // One rounded body; squash scales it about its own base so it never floats.
  const h = Math.round(BODY.h * squash)
  const y = BODY.y + bodyDy + (BODY.h - h)
  sil.roundRect(BODY.x + lean, y, BODY.w, h, BODY.r, BASE)

  sil.shade(LIGHT, SHADE)
  const mask = sil.clone()
  sil.outline(INK)

  const marks = new Layer()
  const faceY = y + 12
  marks.ellipse(CX + lean - 8, faceY + 4, 3, 2, BLUSH)
  marks.ellipse(CX + lean + 8, faceY + 4, 3, 2, BLUSH)
  marks.roundRect(BODY.x + lean + 5, y + h - 12, BODY.w - 10, 9, 4, CREAM)
  marks.maskBy(mask)

  const face = new Layer()
  drawEyes(face, CX + lean, faceY, p.eyes ?? 'open', EYES)
  drawMouth(face, CX + lean, faceY + 7, p.mouth ?? 'smile')

  const props = new Layer()
  for (const prop of p.props ?? []) prop(props)
  props.outline(INK)

  const out = new Layer()
  out.blit(sil)
  out.blit(marks)
  out.blit(face)
  out.blit(props)
  const spark = p.spark ?? { x: 39, y: 12, i: 1 }
  drawWisp(out, spark.x, spark.y, p.sparkColour, spark.i)
  return out
}

function drawMouth(layer, cx, cy, kind) {
  if (kind === 'none') return
  if (kind === 'o') {
    layer.circle(cx, cy, 1.5, INK)
    return
  }
  if (kind === 'flat') {
    layer.rect(cx - 2, cy, 5, 1, INK)
    return
  }
  layer.set(cx - 2, cy, INK)
  layer.set(cx - 1, cy + 1, INK)
  layer.set(cx, cy + 1, INK)
  layer.set(cx + 1, cy + 1, INK)
  layer.set(cx + 2, cy, INK)
}

// --- poses --------------------------------------------------------------
function hopPose(f, n, colour) {
  const swing = Math.sin((f / n) * Math.PI * 2)
  const lift = Math.max(0, swing)
  return {
    sparkColour: colour,
    spark: { x: 39 - swing * 2, y: 12 + swing, i: 1 },
    lean: 1,
    bodyDy: -Math.round(lift * 3),
    squash: 1 - lift * 0.06,
    eyes: 'look-r',
    feet: [
      { dx: Math.round(swing * 2), dy: -Math.round(lift * 3) },
      { dx: Math.round(-swing * 2), dy: -Math.round(lift * 3) }
    ],
    arms: [
      { dx: Math.round(-swing * 2), dy: Math.round(swing * 2) },
      { dx: Math.round(swing * 2), dy: Math.round(-swing * 2) }
    ]
  }
}

function poseFor(row, f) {
  const colour = SPARK[ROW_NAMES[row]]
  switch (ROW_NAMES[row]) {
    case 'idle': {
      const bob = wave(f, 6, 1)
      return {
        sparkColour: colour,
        spark: { x: 39, y: 12 + bob, i: 0.85 },
        bodyDy: bob,
        squash: 1 - bob * 0.03,
        eyes: f === 4 ? 'blink' : 'open'
      }
    }
    case 'running-right':
      return hopPose(f, 8, colour)
    case 'running-left':
      return { ...hopPose(f, 8, colour), flip: true }
    case 'waving': {
      const lift = [0, -1, 0, -1][f]
      return {
        sparkColour: colour,
        spark: { x: [39, 40, 41, 40][f], y: [12, 9, 7, 10][f], i: 1 },
        bodyDy: lift,
        eyes: 'happy',
        arms: [{ dx: -[1, 2, 3, 2][f], dy: [-6, -9, -10, -9][f] }, { dx: 0, dy: 0 }]
      }
    }
    case 'jumping': {
      const dy = [2, -4, -8, -3, 1][f]
      const airborne = f > 0 && f < 4
      return {
        sparkColour: colour,
        spark: { x: 39, y: 14 + dy * 0.5, i: 1 },
        bodyDy: dy,
        squash: [0.84, 1.06, 1.0, 1.0, 0.9][f],
        eyes: f === 2 ? 'happy' : 'open',
        mouth: 'o',
        arms: [{ dx: -1, dy: airborne ? -6 : 0 }, { dx: 1, dy: airborne ? -6 : 0 }],
        // Feet ride with the body, or they get left on the ground mid-jump.
        feet: [{ dx: 0, dy }, { dx: 0, dy }]
      }
    }
    case 'failed': {
      const shake = [-1, 1, -1, 1, 0, -1, 1, 0][f]
      return {
        sparkColour: colour,
        spark: { x: 39 + shake, y: 12, i: f % 2 ? 0.35 : 1 },
        lean: shake,
        eyes: 'x',
        mouth: 'o',
        arms: [{ dx: -2, dy: -3 }, { dx: 2, dy: -3 }],
        props: [(l) => drawSweat(l, 7, 8 + f * 2)]
      }
    }
    case 'waiting': {
      const tilt = [0, 1, 1, 0, -1, -1][f]
      const hop = f % 3 === 0 ? 1 : 0
      return {
        sparkColour: colour,
        spark: { x: 39, y: 12 - hop * 2, i: 1 },
        lean: tilt,
        eyes: f < 3 ? 'look-l' : 'look-r',
        mouth: 'flat',
        feet: [{ dx: 0, dy: 0 }, { dx: 0, dy: f % 2 ? -2 : 0 }],
        props: [(l) => drawBang(l, CX - 2, 2 - hop, colour)]
      }
    }
    case 'running': {
      const type = f % 2 ? -1 : 0
      const angle = (f / 6) * Math.PI * 2
      return {
        sparkColour: colour,
        spark: { x: 39 + Math.cos(angle) * 3, y: 12 + Math.sin(angle) * 2, i: 1 },
        eyes: 'look-d',
        mouth: 'flat',
        arms: [{ dx: 4, dy: 2 + type }, { dx: -4, dy: 2 - type }],
        props: [(l) => drawLaptop(l, f, CX)]
      }
    }
    case 'review': {
      const x = 9 - [0, 1, 2, 2, 1, 0][f]
      const props = [(l) => drawMagnifier(l, x, 26)]
      if (f >= 3) props.push((l) => drawSparkle(l, 38, 30, f - 2))
      return {
        sparkColour: colour,
        spark: { x: 39, y: 11, i: 1 },
        eyes: f >= 3 ? 'happy' : 'open',
        arms: [{ dx: -2, dy: -4 }, { dx: 0, dy: 0 }]
      , props }
    }
    default:
      return { sparkColour: colour }
  }
}

writePet({
  dir: OUT_DIR,
  id: 'ember',
  displayName: 'Ember',
  description: 'A clay pebble with a spark for a status light. The default companion.',
  iconPath: resolve(HERE, '..', 'assets', 'icon-source.png'),
  drawFrame: (row, f) => {
    const pose = poseFor(row, f)
    const frame = drawEmber(pose)
    return pose.flip ? frame.flipped() : frame
  }
})

// Keep the unused-import linter honest about the shared frame size.
void FRAME
void PROP
