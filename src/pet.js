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
    this.running = false
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
    this.state = next
    this.frame = 0
    this.elapsed = 0
  }

  /** Play a row once, then fall back to the current state's row. */
  playOnce(row) {
    this.oneShot = { row, frame: 0 }
  }

  get row() {
    if (this.oneShot) return this.oneShot.row
    return ROW_FOR_STATE[this.state] ?? 0
  }

  start() {
    if (this.running) return
    this.running = true
    const step = (now) => {
      if (!this.running) return
      const delta = this.last ? now - this.last : 0
      this.last = now
      this.tick(delta)
      requestAnimationFrame(step)
    }
    requestAnimationFrame(step)
  }

  tick(delta) {
    const interval = 1000 / (this.manifest.fps || 8)
    this.elapsed += delta
    while (this.elapsed >= interval) {
      this.elapsed -= interval
      this.advance()
    }
    this.draw()
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
