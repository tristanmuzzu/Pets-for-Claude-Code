// Records the README's animation, reproducibly, without filming your desktop.
//
//   node tools/record-demo.mjs [--out assets/demo.png] [--frames 24] [--interval 260]
//
// The obvious way to do this is to run the app, work for a bit, and capture the
// screen — which puts whatever you happen to have open into a public README.
// So this paints its own flat backdrop first, runs the app against an isolated
// profile driven by `simulate.mjs`, and crops the result to the pixels that
// actually changed. Nothing of yours is in frame, and anyone can regenerate it.
//
// Windows only, because the capture is a `CopyFromScreen` through PowerShell.
import { spawn, spawnSync } from 'node:child_process'
import { mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { tmpdir } from 'node:os'
import { decodePng } from './pngdec.mjs'
import { encodeApng } from './apng.mjs'

const BACKDROP_SCRIPT = `param($Hex)
Add-Type -AssemblyName System.Windows.Forms,System.Drawing
$form = New-Object System.Windows.Forms.Form
$form.FormBorderStyle = 'None'
$form.WindowState = 'Maximized'
$form.BackColor = [System.Drawing.ColorTranslator]::FromHtml($Hex)
$form.ShowInTaskbar = $false
[System.Windows.Forms.Application]::Run($form)
`

const CAPTURE_SCRIPT = `param($X,$Y,$W,$H,$Count,$IntervalMs,$OutDir)
Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition 'using System.Runtime.InteropServices; public class Dpi { [DllImport("user32.dll")] public static extern bool SetProcessDPIAware(); }'
[void][Dpi]::SetProcessDPIAware()
$size = New-Object System.Drawing.Size([int]$W, [int]$H)
for ($i = 0; $i -lt [int]$Count; $i++) {
  $bmp = New-Object System.Drawing.Bitmap([int]$W, [int]$H)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen([int]$X, [int]$Y, 0, 0, $size)
  $bmp.Save((Join-Path $OutDir ("f{0:d3}.png" -f $i)), [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose()
  $bmp.Dispose()
  Start-Sleep -Milliseconds ([int]$IntervalMs)
}
`

const HERE = dirname(fileURLToPath(import.meta.url))
const ROOT = resolve(HERE, '..')

const args = new Map()
for (let i = 2; i < process.argv.length; i += 2) {
  args.set(process.argv[i].replace(/^--/, ''), process.argv[i + 1])
}
const OUT = resolve(ROOT, args.get('out') ?? 'assets/demo.png')
const FRAMES = Number(args.get('frames') ?? 24)
const INTERVAL = Number(args.get('interval') ?? 260)
/** Light on purpose: it is the hardest background for a dark card to sit on. */
const BACKDROP = args.get('backdrop') ?? '#E8E4DE'
/**
 * Where in the simulated timeline the recording starts.
 *
 * By this point three projects are going, one of them is genuinely blocked on
 * a human, and a test is about to fail — which is the whole story in one
 * window. Earlier is empty; much later is everything winding down.
 */
const LEAD_IN_MS = Number(args.get('lead') ?? 6_500)
const PADDING = 18

if (process.platform !== 'win32') {
  console.error('Screen capture here is Windows-only.')
  process.exit(1)
}

const BIN = resolve(ROOT, 'src-tauri/target/release/pipsqueak.exe')
try {
  readFileSync(BIN)
} catch {
  console.error(`No release build at ${BIN}. Run: npm run app:build`)
  process.exit(1)
}

const sandbox = resolve(tmpdir(), `pipsqueak-demo-${process.pid}`)
const shots = resolve(sandbox, 'shots')
mkdirSync(resolve(sandbox, '.pipsqueak'), { recursive: true })
mkdirSync(shots, { recursive: true })

// A fixed position makes the capture region deterministic, and keeps the
// recording away from the taskbar.
//
// Two coordinate systems meet here and quietly disagree. Tauri positions the
// window in *physical* pixels, and the capture is DPI-aware so it is physical
// too — but the window's declared size is *logical*, so on a scaled display it
// occupies scale times as many pixels as its numbers suggest. Getting this
// wrong crops the pet off the bottom of its own demo.
const SCREEN = screenSize(true)
const SCALE = SCREEN.w / screenSize(false).w
const WINDOW = { w: Math.round(360 * SCALE), h: Math.round(640 * SCALE) }
const POSITION = {
  x: SCREEN.w - WINDOW.w - Math.round(48 * SCALE),
  y: SCREEN.h - WINDOW.h - Math.round(96 * SCALE)
}
writeFileSync(
  resolve(sandbox, '.pipsqueak/config.json'),
  JSON.stringify({ version: 2, pet: 'byte', scale: 2, welcomed: true, ...POSITION }, null, 2)
)

const env = { ...process.env, USERPROFILE: sandbox, HOME: sandbox }
const children = []
const kill = () => {
  for (const child of children.reverse()) {
    try {
      spawnSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { stdio: 'ignore' })
    } catch {
      /* already gone */
    }
  }
}
process.on('exit', kill)
process.on('SIGINT', () => process.exit(1))

console.log(`Backdrop ${BACKDROP}, window at ${POSITION.x},${POSITION.y}`)
children.push(powershell(BACKDROP_SCRIPT, [BACKDROP]))
await sleep(1200)

children.push(spawn(BIN, [], { env, stdio: 'ignore', detached: false }))
await sleep(2500)

children.push(
  spawn(process.execPath, [resolve(HERE, 'simulate.mjs'), BIN], { env, stdio: 'ignore' })
)
console.log(`Letting the simulator warm up for ${LEAD_IN_MS / 1000}s…`)
await sleep(LEAD_IN_MS)

const region = {
  x: POSITION.x - PADDING,
  y: POSITION.y - PADDING,
  w: WINDOW.w + PADDING * 2,
  h: WINDOW.h + PADDING * 2
}
console.log(`Capturing ${FRAMES} frames every ${INTERVAL}ms…`)
const capture = powershell(CAPTURE_SCRIPT, [
  String(region.x),
  String(region.y),
  String(region.w),
  String(region.h),
  String(FRAMES),
  String(INTERVAL),
  shots
])
await once(capture)

kill()
await sleep(300)

const files = readdirSync(shots)
  .filter((name) => name.endsWith('.png'))
  .sort()
if (files.length === 0) {
  console.error('Nothing was captured.')
  process.exit(1)
}
const decoded = files.map((name) => decodePng(readFileSync(resolve(shots, name))))
// Clamped to the window's own rectangle. A Windows toast wandering into the
// corner during a recording is otherwise "not the backdrop", and the crop
// helpfully widens to include somebody's battery notification.
const box = interestingBox(decoded, BACKDROP, {
  x: PADDING,
  y: PADDING,
  w: WINDOW.w,
  h: WINDOW.h
})
console.log(`Cropping to ${box.w}x${box.h} at ${box.x},${box.y}`)

const frames = decoded.map((frame) => ({
  rgba: crop(frame, box),
  delayMs: INTERVAL
}))
const buffer = encodeApng(box.w, box.h, frames)
writeFileSync(OUT, buffer)
rmSync(sandbox, { recursive: true, force: true })
console.log(`Wrote ${OUT} — ${(buffer.length / 1024).toFixed(0)} KB, ${frames.length} frames`)

// --- helpers -------------------------------------------------------------

function sleep(ms) {
  return new Promise((done) => setTimeout(done, ms))
}

function once(child) {
  return new Promise((done) => child.on('exit', done))
}

function powershell(script, params) {
  const file = resolve(sandbox, `${Math.random().toString(36).slice(2)}.ps1`)
  writeFileSync(file, script)
  return spawn('powershell', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', file, ...params], {
    stdio: 'ignore'
  })
}

function screenSize(physical) {
  // The type name has to be unique per process, and each of these is a fresh
  // PowerShell, so a fixed name is fine — but declaring it is not enough, it
  // has to be *called* before anything asks about the screen.
  const aware = physical
    ? `Add-Type -TypeDefinition 'using System.Runtime.InteropServices; public class Dpi { [DllImport("user32.dll")] public static extern bool SetProcessDPIAware(); }'; [void][Dpi]::SetProcessDPIAware(); `
    : ''
  const out = spawnSync(
    'powershell',
    [
      '-NoProfile',
      '-Command',
      `${aware}Add-Type -AssemblyName System.Windows.Forms; $b=[System.Windows.Forms.SystemInformation]::VirtualScreen; "$($b.Width) $($b.Height)"`
    ],
    { encoding: 'utf8' }
  )
  const [w, h] = out.stdout.trim().split(/\s+/).map(Number)
  return { w: w || 1920, h: h || 1080 }
}

/**
 * The smallest box containing every pixel that is not the backdrop, in any
 * frame.
 *
 * Cropping to what actually moved beats guessing a region: the card stack
 * changes height as projects come and go, and a fixed rectangle either clips
 * it or leaves a lake of empty backdrop around it.
 */
function interestingBox(frames, hex, limit) {
  const target = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16))
  const near = (r, g, b) =>
    Math.abs(r - target[0]) < 10 && Math.abs(g - target[1]) < 10 && Math.abs(b - target[2]) < 10
  const { width, height } = frames[0]
  let minX = width
  let minY = height
  let maxX = 0
  let maxY = 0
  for (const frame of frames) {
    for (let y = 0; y < height; y++) {
      if (y < limit.y || y >= limit.y + limit.h) continue
      for (let x = limit.x; x < Math.min(width, limit.x + limit.w); x++) {
        const o = (y * width + x) * 4
        if (near(frame.rgba[o], frame.rgba[o + 1], frame.rgba[o + 2])) continue
        if (x < minX) minX = x
        if (y < minY) minY = y
        if (x > maxX) maxX = x
        if (y > maxY) maxY = y
      }
    }
  }
  if (minX > maxX) return { x: 0, y: 0, w: width, h: height }
  const pad = 12
  const x = Math.max(0, minX - pad)
  const y = Math.max(0, minY - pad)
  return {
    x,
    y,
    // Even dimensions: some encoders and a lot of video tooling dislike odd ones.
    w: Math.min(width - x, maxX - x + pad + 1) & ~1,
    h: Math.min(height - y, maxY - y + pad + 1) & ~1
  }
}

function crop(frame, box) {
  const out = new Uint8Array(box.w * box.h * 4)
  for (let y = 0; y < box.h; y++) {
    const from = ((y + box.y) * frame.width + box.x) * 4
    out.set(frame.rgba.subarray(from, from + box.w * 4), y * box.w * 4)
  }
  return out
}
