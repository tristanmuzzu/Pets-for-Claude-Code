// Byte: a little CRT-headed bot.
//
// The screen is the status: it shows the state as a face, so you can read the
// pet without reading the card. No mouth, no fur, no ambiguity: the silhouette
// is a monitor on legs.
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  ROW_NAMES,
  Layer,
  drawWisp,
  drawSparkle,
  drawSweat,
  lookDirection,
  wave,
  writePet
} from './pixel.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))
const OUT_DIR = resolve(HERE, '..', 'public', 'pets', 'byte')

// --- palette ------------------------------------------------------------
const OUTLINE = [24, 28, 36, 255]
const SHELL = [122, 134, 152, 255]
const SHELL_LIGHT = [163, 175, 192, 255]
const SHELL_SHADE = [82, 92, 108, 255]
const BEZEL = [44, 50, 62, 255]
const SCREEN = [14, 18, 24, 255]

const LIGHT = {
  idle: [138, 150, 168, 255],
  'running-right': [96, 176, 255, 255],
  'running-left': [96, 176, 255, 255],
  waving: [92, 222, 140, 255],
  jumping: [186, 130, 255, 255],
  failed: [255, 96, 96, 255],
  waiting: [255, 158, 62, 255],
  running: [96, 176, 255, 255],
  review: [92, 222, 140, 255]
}

// --- geometry -----------------------------------------------------------
const CX = 22
const HEAD = { x: 8, y: 7, w: 28, h: 22, r: 5 }
const SCREEN_BOX = { x: 11, y: 10, w: 22, h: 15 }
const BODY = { x: 15, y: 30, w: 14, h: 9, r: 3 }
const FOOT = { y: 39, w: 7, h: 5, r: 2, dx: 6 }
const ARM = { y: 30, w: 4, h: 8, r: 2, dx: 15 }

/**
 * Faces are drawn straight onto the screen in the state colour. Chunky blocks
 * rather than single pixels. At 96 physical pixels tall, thin glyphs turn to
 * mush, and the pet is supposed to read without the card.
 */
function drawFace(layer, kind, colour, dx = 0, dy = 0) {
  const cy = SCREEN_BOX.y + 6 + dy
  const left = SCREEN_BOX.x + 6 + dx
  const right = SCREEN_BOX.x + 16 + dx
  const centre = SCREEN_BOX.x + SCREEN_BOX.w / 2 + dx

  const eye = (x) => layer.rect(x - 1, cy - 2, 3, 5, colour)
  const chevron = (x) => {
    layer.rect(x - 2, cy + 1, 2, 2, colour)
    layer.rect(x - 1, cy - 1, 2, 2, colour)
    layer.rect(x + 1, cy + 1, 2, 2, colour)
  }
  const cross = (x) => {
    for (let i = -2; i <= 2; i++) {
      layer.set(x + i, cy + i, colour)
      layer.set(x + i, cy - i, colour)
      layer.set(x + i + 1, cy + i, colour)
      layer.set(x + i + 1, cy - i, colour)
    }
  }

  switch (kind) {
    case 'blink':
      layer.rect(left - 2, cy, 5, 2, colour)
      layer.rect(right - 2, cy, 5, 2, colour)
      break
    case 'happy':
      chevron(left)
      chevron(right)
      break
    case 'dead':
      cross(left)
      cross(right)
      break
    case 'bang':
      layer.rect(centre - 1, cy - 4, 3, 6, colour)
      layer.rect(centre - 1, cy + 3, 3, 2, colour)
      break
    case 'look':
      eye(left + 1)
      eye(right + 1)
      break
    case 'busy':
      eye(left)
      eye(right)
      layer.rect(left - 2, cy + 5, 4, 1, colour)
      break
    case 'scan':
      eye(left)
      eye(right)
      layer.rect(left - 2, cy + 5, 12, 1, colour)
      break
    default:
      eye(left)
      eye(right)
  }
}

/**
 * Text scrolling under the face while it works.
 *
 * Three things keep it from reading as a broken screen. It stays below the
 * eyes, because a line crossing them looks like damage rather than reading. It
 * is mixed into the screen colour rather than drawn semi-transparent: these
 * pixels replace the dark panel, so an alpha here would end up blended against
 * the *shell* behind it and come out brighter than the screen it is supposed to
 * be inside. And it drifts at half the frame rate, because two pixels of travel
 * per frame at this size is a flicker, not a scroll.
 */
function drawScanlines(layer, frame, colour, dx = 0, dy = 0) {
  const dim = colour.map((channel, i) => Math.round(SCREEN[i] + (channel - SCREEN[i]) * 0.28))
  dim[3] = 255
  const top = SCREEN_BOX.y + 10 + dy
  const travel = 3
  const drift = Math.floor(frame / 2)
  for (let i = 0; i < 2; i++) {
    const y = top + ((drift + i * 2) % travel)
    layer.rect(SCREEN_BOX.x + 3 + dx, y, 5 + ((drift + i) % 5), 1, dim)
  }
}

function drawByte(p) {
  const sil = new Layer()
  const bodyDy = p.bodyDy ?? 0
  const headDy = (p.headDy ?? 0) + bodyDy
  const headDx = p.headDx ?? 0
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
      SHELL
    )
  })
  arms.forEach((arm, i) => {
    const side = i === 0 ? -1 : 1
    sil.roundRect(
      Math.round(CX + side * ARM.dx - ARM.w / 2 + arm.dx),
      ARM.y + arm.dy + bodyDy,
      ARM.w,
      ARM.h,
      ARM.r,
      SHELL
    )
  })
  sil.roundRect(BODY.x, BODY.y + bodyDy, BODY.w, BODY.h, BODY.r, SHELL)
  sil.roundRect(HEAD.x + headDx, HEAD.y + headDy, HEAD.w, HEAD.h, HEAD.r, SHELL)

  sil.shade(SHELL_LIGHT, SHELL_SHADE)
  sil.outline(OUTLINE)

  // Screen: bezel, then the dark panel, then whatever the face is doing.
  const glass = new Layer()
  glass.roundRect(
    SCREEN_BOX.x + headDx - 1,
    SCREEN_BOX.y + headDy - 1,
    SCREEN_BOX.w + 2,
    SCREEN_BOX.h + 2,
    3,
    BEZEL
  )
  glass.roundRect(SCREEN_BOX.x + headDx, SCREEN_BOX.y + headDy, SCREEN_BOX.w, SCREEN_BOX.h, 2, SCREEN)
  // Offset with the head, or the lines drift off the panel and onto the bezel
  // whenever the pet leans.
  if (p.scan !== undefined) drawScanlines(glass, p.scan, p.light, headDx, headDy)
  drawFace(glass, p.face ?? 'open', p.light, headDx + (p.faceDx ?? 0), headDy + (p.faceDy ?? 0))

  // A status lamp on the body, so the colour still reads if the screen is busy.
  const lamp = new Layer()
  lamp.circle(CX, BODY.y + bodyDy + 4, 1.6, p.light)

  const props = new Layer()
  for (const prop of p.props ?? []) prop(props)
  props.outline(OUTLINE)

  const out = new Layer()
  out.blit(sil)
  out.blit(glass)
  out.blit(lamp)
  out.blit(props)
  const halo = p.halo ?? { x: 40, y: 10, i: 0.9 }
  drawWisp(out, halo.x, halo.y, p.light, halo.i)
  return out
}

// --- poses --------------------------------------------------------------
function stepPose(f, n, light) {
  const swing = Math.sin((f / n) * Math.PI * 2)
  const lift = Math.max(0, swing)
  return {
    light,
    halo: { x: 40 - swing * 2, y: 10 + swing, i: 0.9 },
    headDx: 1,
    bodyDy: -Math.round(lift * 2),
    face: 'look',
    scan: f,
    feet: [
      { dx: Math.round(swing * 3), dy: -Math.round(lift * 2) },
      { dx: Math.round(-swing * 3), dy: 0 }
    ],
    arms: [
      { dx: Math.round(-swing * 2), dy: Math.round(swing * 2) },
      { dx: Math.round(swing * 2), dy: Math.round(-swing * 2) }
    ]
  }
}

function poseFor(row, f) {
  const light = LIGHT[ROW_NAMES[row]] ?? LIGHT.idle
  switch (ROW_NAMES[row]) {
    // The sixteen look poses. Byte has no neck and no pupils, so a direction
    // can only come from where on the screen the eyes sit: the ordinary open
    // face, slid two pixels, with the head leaning one and the halo drifting
    // after it. Three small agreements read as attention where one large
    // movement would read as a glitch.
    case 'look-a':
    case 'look-b': {
      const { dx, dy } = lookDirection(row, f)
      return {
        light,
        halo: { x: 40 + Math.round(dx * 2), y: 10 + Math.round(dy * 2), i: 0.75 },
        headDx: Math.round(dx),
        headDy: Math.round(dy),
        faceDx: Math.round(dx * 2),
        faceDy: Math.round(dy * 2)
      }
    }
    case 'idle': {
      const bob = wave(f, 6, 1)
      return { light, halo: { x: 40, y: 10 + bob, i: 0.75 }, headDy: bob, face: f === 4 ? 'blink' : 'open' }
    }
    case 'running-right':
      return stepPose(f, 8, light)
    case 'running-left':
      return { ...stepPose(f, 8, light), flip: true }
    case 'waving': {
      const lift = [0, -1, 0, -1][f]
      return {
        light,
        halo: { x: [40, 41, 42, 41][f], y: [10, 8, 6, 8][f], i: 1 },
        bodyDy: lift,
        face: 'happy',
        arms: [{ dx: -[1, 2, 3, 2][f], dy: [-6, -9, -10, -9][f] }, { dx: 0, dy: 0 }]
      }
    }
    case 'jumping': {
      const dy = [2, -4, -8, -3, 1][f]
      const airborne = f > 0 && f < 4
      return {
        light,
        halo: { x: 40, y: 12 + dy * 0.5, i: 1 },
        bodyDy: dy,
        face: f === 2 ? 'happy' : 'open',
        arms: [{ dx: -1, dy: airborne ? -6 : 0 }, { dx: 1, dy: airborne ? -6 : 0 }],
        feet: [{ dx: 0, dy }, { dx: 0, dy }]
      }
    }
    case 'failed': {
      const shake = [-1, 1, -1, 1, 0, -1, 1, 0][f]
      return {
        light,
        halo: { x: 40 + shake, y: 10, i: f % 2 ? 0.3 : 1 },
        headDx: shake,
        face: 'dead',
        arms: [{ dx: -2, dy: -3 }, { dx: 2, dy: -3 }],
        props: [(l) => drawSweat(l, 5, 9 + f * 2)]
      }
    }
    case 'waiting': {
      const hop = f % 3 === 0 ? 1 : 0
      return {
        light,
        halo: { x: 40, y: 10 - hop * 2, i: 1 },
        headDy: -hop,
        face: 'bang',
        faceDx: 0,
        feet: [{ dx: 0, dy: 0 }, { dx: 0, dy: f % 2 ? -2 : 0 }]
      }
    }
    case 'running':
      return {
        light,
        halo: {
          x: 40 + Math.cos((f / 6) * Math.PI * 2) * 2,
          y: 10 + Math.sin((f / 6) * Math.PI * 2) * 2,
          i: 1
        },
        face: 'busy',
        scan: f * 2,
        arms: [{ dx: 2, dy: 2 + (f % 2) }, { dx: -2, dy: 2 - (f % 2) }]
      }
    case 'review': {
      const props = []
      if (f >= 3) props.push((l) => drawSparkle(l, 8, 12, f - 2))
      return {
        light,
        halo: { x: 40, y: 9, i: 1 },
        face: f >= 3 ? 'happy' : 'scan',
        scan: f,
        arms: [{ dx: -1, dy: -4 }, { dx: 0, dy: 0 }],
        props
      }
    }
    default:
      return { light }
  }
}

writePet({
  dir: OUT_DIR,
  id: 'byte',
  displayName: 'Byte',
  description: 'A CRT-headed bot. Its screen shows the state, so the pet reads on its own.',
  drawFrame: (row, f) => {
    const pose = poseFor(row, f)
    const frame = drawByte(pose)
    return pose.flip ? frame.flipped() : frame
  }
})
