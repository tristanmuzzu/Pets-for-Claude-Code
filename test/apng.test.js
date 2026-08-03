// The animated PNG writer, checked structurally.
//
// A malformed APNG does not fail loudly: browsers fall back to showing the
// first frame, so a broken encoder looks exactly like a still image and the
// README quietly stops selling the product.
import test from 'node:test'
import assert from 'node:assert/strict'
import { encodeApng } from '../tools/apng.mjs'

const solid = (w, h, r, g, b) => {
  const px = new Uint8Array(w * h * 4)
  for (let i = 0; i < px.length; i += 4) {
    px[i] = r
    px[i + 1] = g
    px[i + 2] = b
    px[i + 3] = 255
  }
  return px
}

/** Every chunk in the file, in order, as `{ type, data }`. */
function chunks(buffer) {
  const out = []
  let offset = 8
  while (offset < buffer.length) {
    const length = buffer.readUInt32BE(offset)
    const type = buffer.toString('ascii', offset + 4, offset + 8)
    out.push({ type, data: buffer.subarray(offset + 8, offset + 8 + length) })
    offset += length + 12
  }
  return out
}

const sample = () =>
  encodeApng(4, 4, [
    { rgba: solid(4, 4, 255, 0, 0), delayMs: 120 },
    { rgba: solid(4, 4, 0, 255, 0), delayMs: 80 },
    { rgba: solid(4, 4, 0, 0, 255), delayMs: 200 }
  ])

test('it is a valid PNG before it is anything else', () => {
  const buffer = sample()
  assert.deepEqual([...buffer.subarray(0, 8)], [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])
  const types = chunks(buffer).map((c) => c.type)
  assert.equal(types[0], 'IHDR')
  assert.equal(types.at(-1), 'IEND')
  assert.equal(types.filter((t) => t === 'IDAT').length, 1, 'exactly one still image')
})

test('the animation control chunk precedes the image', () => {
  const types = chunks(sample()).map((c) => c.type)
  // A decoder that meets IDAT before acTL is required to ignore the animation
  // entirely, which is the silent-failure case.
  assert.ok(types.indexOf('acTL') < types.indexOf('IDAT'))
})

test('the frame count matches the frames', () => {
  const parsed = chunks(sample())
  const actl = parsed.find((c) => c.type === 'acTL')
  assert.equal(actl.data.readUInt32BE(0), 3)
  assert.equal(actl.data.readUInt32BE(4), 0, 'loops forever')
  assert.equal(parsed.filter((c) => c.type === 'fcTL').length, 3)
  assert.equal(parsed.filter((c) => c.type === 'fdAT').length, 2, 'the first frame is the IDAT')
})

test('sequence numbers are one unbroken run', () => {
  const numbers = chunks(sample())
    .filter((c) => c.type === 'fcTL' || c.type === 'fdAT')
    .map((c) => c.data.readUInt32BE(0))
  assert.deepEqual(numbers, [0, 1, 2, 3, 4])
})

test('delays survive as milliseconds', () => {
  const delays = chunks(sample())
    .filter((c) => c.type === 'fcTL')
    .map((c) => ({ num: c.data.readUInt16BE(20), den: c.data.readUInt16BE(22) }))
  assert.deepEqual(delays, [
    { num: 120, den: 1000 },
    { num: 80, den: 1000 },
    { num: 200, den: 1000 }
  ])
})

test('a zero delay becomes the shortest real one', () => {
  // Zero means "as fast as possible", which browsers each interpret
  // differently. One millisecond is at least the same everywhere.
  const [first] = chunks(encodeApng(1, 1, [{ rgba: solid(1, 1, 0, 0, 0), delayMs: 0 }])).filter(
    (c) => c.type === 'fcTL'
  )
  assert.equal(first.data.readUInt16BE(20), 1)
})

test('an empty animation is refused rather than written', () => {
  assert.throws(() => encodeApng(4, 4, []))
})
