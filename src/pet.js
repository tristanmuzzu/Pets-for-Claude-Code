// Sprite-atlas renderer.
//
// Defaults match the Codex pet contract (8x9 atlas of 192x208 cells), so a pet
// folder written for either app renders here without conversion. Pipsqueak's
// own pets declare their real geometry in pet.json and override the defaults.
const CODEX_DEFAULTS = {
  frameWidth: 192,
  frameHeight: 208,
  columns: 8,
  rows: 9,
  frameCounts: [6, 8, 8, 4, 5, 8, 6, 6, 6],
  fps: 8
}

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
const BASE_HEIGHT = 48

/** Long enough to read as a transition, short enough not to feel like lag. */
const FADE_MS = 140
/**
 * How long the pet keeps animating an idle loop before settling.
 *
 * The point of this overlay is that you can leave it up while doing something
 * else — which is exactly when a permanently animating canvas is at its most
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
    this.manifest = { ...CODEX_DEFAULTS, ...manifest }
    if (!Array.isArray(this.manifest.frameCounts) || this.manifest.frameCounts.length === 0) {
      this.manifest.frameCounts = CODEX_DEFAULTS.frameCounts
    }
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
    this.wake()
  }

  get row() {
    if (this.oneShot) return this.oneShot.row
    return ROW_FOR_STATE[this.state] ?? 0
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

    this.elapsed += delta
    let guard = 0
    while (this.elapsed >= this.frameDuration() && guard++ < 8) {
      this.elapsed -= this.frameDuration()
      this.advance()
    }
    this.draw()

    // Settle on the first frame of a resting loop and stop asking for frames.
    // Stopping mid-stride would freeze the pet in a walking pose, so this only
    // takes effect at the top of a cycle.
    if (this.state === 'idle' && !this.oneShot && !this.outgoing && this.frame === 0) {
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
    if (this.outgoing) {
      const t = Math.min(1, this.outgoing.age / FADE_MS)
      ctx.globalAlpha = 1 - t
      ctx.drawImage(
        image,
        this.outgoing.frame * fw,
        this.outgoing.row * fh,
        fw, fh, 0, 0, fw, fh
      )
      ctx.globalAlpha = t
      ctx.drawImage(image, this.frame * fw, this.row * fh, fw, fh, 0, 0, fw, fh)
      ctx.globalAlpha = 1
      return
    }
    ctx.drawImage(image, this.frame * fw, this.row * fh, fw, fh, 0, 0, fw, fh)
  }
}

export { ROW_NAMES, GREETING_ROW }

function loadImage(src) {
  return new Promise((resolve, reject) => {
    const image = new Image()
    image.onload = () => resolve(image)
    image.onerror = () => reject(new Error(`cannot load sprite: ${src}`))
    image.src = src
  })
}
