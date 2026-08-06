// Sprite-atlas renderer.
//
// Defaults match the Codex pet contract, so a pet folder written for either app
// renders here without conversion. Pipsqueak's own pets declare their real
// geometry in pet.json and override the defaults.
//
// The contract has two versions. Both are 8 columns of 192x208 cells with the
// same nine states in the same order; version 2 adds two rows of sixteen
// still frames, one every 22.5°, for looking towards a point. A v2 pet says so
// with `spriteVersionNumber: 2` and nothing else — no geometry, no frame
// counts — so the version has to imply the whole layout.
const CODEX_DEFAULTS = {
  frameWidth: 192,
  frameHeight: 208,
  columns: 8,
  rows: 9,
  frameCounts: [6, 8, 8, 4, 5, 8, 6, 6, 6],
  fps: 8
}

/** The extra rows a version 2 atlas carries, and their frame counts. */
const LOOK_ROWS = [9, 10]
const CODEX_V2 = {
  ...CODEX_DEFAULTS,
  rows: 11,
  frameCounts: [6, 8, 8, 4, 5, 8, 6, 6, 6, 8, 8]
}

/**
 * The contract's own frame timings, in milliseconds.
 *
 * Even, with a longer hold on the last frame of each row. A pet that ships no
 * timings of its own gets these rather than a flat frame rate, because a flat
 * frame rate is what makes a six-frame loop read as a machine.
 */
const CODEX_DURATIONS = [
  [280, 110, 110, 140, 140, 320],
  [120, 120, 120, 120, 120, 120, 120, 220],
  [120, 120, 120, 120, 120, 120, 120, 220],
  [140, 140, 140, 280],
  [140, 140, 140, 140, 280],
  [140, 140, 140, 140, 140, 140, 140, 240],
  [150, 150, 150, 150, 150, 260],
  [120, 120, 120, 120, 120, 220],
  [150, 150, 150, 150, 150, 280]
]

const ROW_NAMES = [
  'idle', 'running-right', 'running-left', 'waving', 'jumping',
  'failed', 'waiting', 'running', 'review'
]

/** Agent state -> atlas row. */
const ROW_FOR_STATE = {
  idle: 0,
  thinking: 7,
  running: 7,
  waiting: 6,
  failed: 5,
  done: 8,
  compacting: 4
}

const GREETING_ROW = 3
const JUMP_ROW = 4
const BASE_HEIGHT = 48
/** Closer than this to the pet's centre and a direction is meaningless. */
const LOOK_DEADZONE_PX = 6

/** Long enough to read as a transition, short enough not to feel like lag. */
const FADE_MS = 140
/**
 * How long the pet keeps animating an idle loop before settling.
 *
 * The point of this overlay is that you can leave it up while doing something
 * else, which is exactly when a permanently animating canvas is at its most
 * wasteful, compositing a frame every 16ms over a video that wants the GPU.
 */
const SETTLE_AFTER_MS = 5000

export class PetRenderer {
  constructor(canvas) {
    this.canvas = canvas
    this.ctx = canvas.getContext('2d')
    this.ctx.imageSmoothingEnabled = false
    this.manifest = { ...CODEX_DEFAULTS }
    this.image = null
    this.state = 'idle'
    this.scale = 2
    this.frame = 0
    this.elapsed = 0
    this.last = 0
    this.oneShot = null
    this.wanted = false
    this.looping = false
    // The row being faded out, so a state change is a dissolve rather than a
    // hard cut. Easier to catch out of the corner of your eye.
    this.outgoing = null
    this.idleSince = 0
  }

  async load(id, payload) {
    let manifest
    let src
    if (!payload || !payload.imageDataUrl) {
      // Built-in pets ship inside the frontend bundle.
      manifest = await fetch(`/pets/${id}/pet.json`).then((r) => r.json())
      src = `/pets/${id}/spritesheet.png`
    } else {
      manifest = payload.manifest ?? {}
      src = payload.imageDataUrl
    }
    const image = await loadImage(src)
    const contract = manifest.spriteVersionNumber === 2 ? CODEX_V2 : CODEX_DEFAULTS
    this.manifest = { ...contract, ...manifest }
    if (!Array.isArray(this.manifest.frameCounts) || this.manifest.frameCounts.length === 0) {
      this.manifest.frameCounts = contract.frameCounts
    }
    // Timings come from the contract only when the *pet* offered neither its
    // own table nor a frame rate. Testing the merged manifest here would test
    // the default `fps` we just merged in, and no pet would ever get them.
    if (!Array.isArray(manifest.frameDurations) && manifest.fps === undefined) {
      this.manifest.frameDurations = CODEX_DURATIONS
    }
    // A pet can only look at things if it was drawn looking in every direction.
    this.canLook =
      this.manifest.rows >= CODEX_V2.rows &&
      LOOK_ROWS.every((row) => this.frameCount(row) === 8) &&
      image.height >= this.manifest.frameHeight * CODEX_V2.rows
    this.image = image
    this.id = id
    this.frame = 0
    this.applySize()
  }

  applySize() {
    const { frameWidth, frameHeight } = this.manifest
    this.canvas.width = frameWidth
    this.canvas.height = frameHeight
    this.ctx.imageSmoothingEnabled = false
    const height = BASE_HEIGHT * this.scale
    const width = Math.round((height * frameWidth) / frameHeight)
    this.canvas.style.height = `${height}px`
    this.canvas.style.width = `${width}px`
    // Nearest-neighbour is right for upscaled pixel art and wrong for the much
    // larger Codex-sized cells, which are being scaled down.
    this.canvas.style.imageRendering = frameHeight > height ? 'auto' : 'pixelated'
  }

  setScale(scale) {
    this.scale = scale
    this.applySize()
  }

  setState(state) {
    const next = state in ROW_FOR_STATE ? state : 'idle'
    if (next === this.state) return
    if (this.oneShot) {
      // The one-shot owns the canvas until it finishes. Recording a
      // crossfade from its row, or resetting the frame counter it is using,
      // made the sequence visibly jump (…3, 0, 5). The new state simply
      // takes over when the one-shot ends.
      this.state = next
      this.wake()
      return
    }
    const from = this.row
    this.state = next
    if (from !== this.row) {
      this.outgoing = { row: from, frame: this.frame, age: 0 }
    }
    this.frame = 0
    this.elapsed = 0
    this.wake()
  }

  /** Play a row once, then fall back to the current state's row. */
  playOnce(row) {
    this.oneShot = { row, frame: 0 }
    // `draw` reads `this.frame`, so without this reset the one-shot's first
    // visible frame was whatever frame the previous row happened to be on.
    this.frame = 0
    this.elapsed = 0
    this.wake()
  }

  /**
   * Turn towards a point, in pixels relative to the pet's centre.
   *
   * Sixteen poses, one every 22.5°, with zero degrees pointing straight up. A
   * point at the very centre has no direction to give, so it means "stop
   * looking" rather than "look up" — which is also what happens when the cursor
   * leaves. Only pets drawn with the two look rows can do this; the rest ignore
   * it entirely rather than showing a frame from the wrong row.
   */
  lookAt(dx, dy) {
    if (!this.canLook) return
    const look = lookFrame(dx, dy)
    const same = look?.row === this.look?.row && look?.frame === this.look?.frame
    if (same) return
    this.look = look
    // Look rows are eight frames wide and most animation rows are not, so a
    // frame index that outlives the pose it came from lands on an empty cell
    // and the pet disappears for a moment.
    if (!look) {
      this.frame = 0
      this.elapsed = 0
    }
    this.wake()
  }

  get row() {
    if (this.oneShot) return this.oneShot.row
    if (this.looking) return this.look.row
    return ROW_FOR_STATE[this.state] ?? 0
  }

  /**
   * Whether the look pose is what should be on screen.
   *
   * Only while resting. A pet that is working has something to say with the
   * animation it is playing, and a status overlay that stops saying it the
   * moment the cursor drifts past has stopped being a status overlay.
   */
  get looking() {
    return Boolean(this.look) && this.state === 'idle' && !this.oneShot
  }

  start() {
    this.wanted = true
    this.wake()
  }

  stop() {
    this.wanted = false
    this.looping = false
  }

  /** Resume animating; called whenever something actually changed. */
  wake() {
    this.idleSince = 0
    if (!this.wanted || this.looping) return
    this.looping = true
    this.last = 0
    const step = (now) => {
      if (!this.looping) return
      const delta = this.last ? Math.min(now - this.last, 250) : 0
      this.last = now
      this.tick(delta, now)
      if (this.looping) requestAnimationFrame(step)
    }
    requestAnimationFrame(step)
  }

  tick(delta, now) {
    if (this.outgoing) {
      this.outgoing.age += delta
      if (this.outgoing.age >= FADE_MS) this.outgoing = null
    }

    // A look pose is a single still frame chosen by direction, so nothing
    // advances while one is held.
    if (this.looking) {
      this.frame = this.look.frame
      this.elapsed = 0
    } else {
      this.elapsed += delta
      let guard = 0
      while (this.elapsed >= this.frameDuration() && guard++ < 8) {
        this.elapsed -= this.frameDuration()
        this.advance()
      }
    }
    this.draw()

    // Settle on the first frame of a resting loop and stop asking for frames.
    // Stopping mid-stride would freeze the pet in a walking pose, so this only
    // takes effect at the top of a cycle. A held look pose is already still,
    // so it settles immediately.
    if (this.looking) {
      if (!this.idleSince) this.idleSince = now
      else if (now - this.idleSince > FADE_MS) this.looping = false
    } else if (this.state === 'idle' && !this.oneShot && !this.outgoing && this.frame === 0) {
      if (!this.idleSince) this.idleSince = now
      else if (now - this.idleSince > SETTLE_AFTER_MS) this.looping = false
    } else {
      this.idleSince = 0
    }
  }

  advance() {
    const count = this.frameCount(this.row)
    if (this.oneShot) {
      this.oneShot.frame += 1
      if (this.oneShot.frame >= count) this.oneShot = null
      this.frame = this.oneShot ? this.oneShot.frame : 0
      return
    }
    this.frame = (this.frame + 1) % count
  }

  /**
   * How long the current frame is held.
   *
   * A pet may give any frame its own duration through `frameDurations`. Even
   * timing is what makes a short pixel loop read as a metronome; holding the
   * resting frames and snapping through the middle is what makes the same
   * frames read as alive.
   */
  frameDuration() {
    const table = this.manifest.frameDurations
    const row = Array.isArray(table) ? table[this.row] : null
    const value = Array.isArray(row) ? row[this.frame] : null
    if (Number.isFinite(value) && value > 0) return value
    return 1000 / (this.manifest.fps || 8)
  }

  frameCount(row) {
    const counts = this.manifest.frameCounts
    const value = counts[row]
    return Number.isInteger(value) && value > 0 ? value : 1
  }

  draw() {
    const { ctx, image, manifest } = this
    ctx.clearRect(0, 0, this.canvas.width, this.canvas.height)
    if (!image) return
    const { frameWidth: fw, frameHeight: fh } = manifest
    // Rows are not all the same width, and an index from a wider one lands on
    // an empty cell: the pet vanishes rather than draws wrong, which is the
    // harder kind of bug to see.
    const cell = (row, frame) => Math.min(frame, this.frameCount(row) - 1) * fw
    if (this.outgoing) {
      const t = Math.min(1, this.outgoing.age / FADE_MS)
      ctx.globalAlpha = 1 - t
      ctx.drawImage(
        image,
        cell(this.outgoing.row, this.outgoing.frame),
        this.outgoing.row * fh,
        fw, fh, 0, 0, fw, fh
      )
      ctx.globalAlpha = t
      ctx.drawImage(image, cell(this.row, this.frame), this.row * fh, fw, fh, 0, 0, fw, fh)
      ctx.globalAlpha = 1
      return
    }
    ctx.drawImage(image, cell(this.row, this.frame), this.row * fh, fw, fh, 0, 0, fw, fh)
  }
}

/**
 * Which look pose faces a point, given its offset from the pet's centre.
 *
 * Sixteen poses, one every 22.5°, laid out clockwise from straight up across
 * two rows of eight. `null` means there is no direction worth facing: either
 * nothing is pointing at it, or the point is close enough to the centre that
 * the angle would swing wildly for a pixel of movement.
 */
export function lookFrame(dx, dy) {
  if (!Number.isFinite(dx) || !Number.isFinite(dy)) return null
  if (Math.hypot(dx, dy) <= LOOK_DEADZONE_PX) return null
  const degrees = (Math.atan2(dx, -dy) * (180 / Math.PI) + 360) % 360
  const step = Math.round(degrees / 22.5) % 16
  return { row: LOOK_ROWS[Math.floor(step / 8)], frame: step % 8 }
}

export { ROW_NAMES, GREETING_ROW, JUMP_ROW, LOOK_ROWS }

function loadImage(src) {
  return new Promise((resolve, reject) => {
    const image = new Image()
    image.onload = () => resolve(image)
    image.onerror = () => reject(new Error(`cannot load sprite: ${src}`))
    image.src = src
  })
}
