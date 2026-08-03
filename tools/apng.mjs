// Minimal dependency-free RGBA -> animated PNG encoder.
//
// APNG rather than GIF because the demo is a screen recording: 256 colours
// would band the gradients behind the pet badly, and the encoder is a few
// chunks on top of the PNG writer we already have rather than an LZW
// implementation. GitHub serves it as an ordinary PNG and browsers animate it.
import { deflateSync } from 'node:zlib'

const CRC_TABLE = new Uint32Array(256)
for (let n = 0; n < 256; n++) {
  let c = n
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
  CRC_TABLE[n] = c >>> 0
}

function crc32(buf) {
  let c = 0xffffffff
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8)
  return (c ^ 0xffffffff) >>> 0
}

function chunk(type, data) {
  const len = Buffer.alloc(4)
  len.writeUInt32BE(data.length, 0)
  const tag = Buffer.from(type, 'ascii')
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(Buffer.concat([tag, data])), 0)
  return Buffer.concat([len, tag, data, crc])
}

/** Scanlines with a filter byte in front of each, ready for deflate. */
function rawScanlines(width, height, rgba) {
  const stride = width * 4
  const raw = Buffer.alloc((stride + 1) * height)
  const src = Buffer.from(rgba.buffer, rgba.byteOffset, rgba.byteLength)
  for (let y = 0; y < height; y++) {
    raw[y * (stride + 1)] = 0
    src.copy(raw, y * (stride + 1) + 1, y * stride, y * stride + stride)
  }
  return raw
}

/**
 * @param {number} width
 * @param {number} height
 * @param {Array<{rgba: Uint8Array, delayMs: number}>} frames
 * @param {number} [loops] 0 means forever.
 */
export function encodeApng(width, height, frames, loops = 0) {
  if (frames.length === 0) throw new Error('an animation needs at least one frame')

  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(width, 0)
  ihdr.writeUInt32BE(height, 4)
  ihdr[8] = 8 // bit depth
  ihdr[9] = 6 // colour type: truecolour with alpha

  const actl = Buffer.alloc(8)
  actl.writeUInt32BE(frames.length, 0)
  actl.writeUInt32BE(loops, 4)

  // Sequence numbers run across fcTL *and* fdAT chunks in a single series.
  let sequence = 0
  const fctl = (delayMs) => {
    const data = Buffer.alloc(26)
    data.writeUInt32BE(sequence++, 0)
    data.writeUInt32BE(width, 4)
    data.writeUInt32BE(height, 8)
    data.writeUInt32BE(0, 12) // x offset
    data.writeUInt32BE(0, 16) // y offset
    // Delay is a rational number of seconds. A 1000 denominator makes the
    // numerator plain milliseconds.
    data.writeUInt16BE(Math.max(1, Math.round(delayMs)), 20)
    data.writeUInt16BE(1000, 22)
    data[24] = 0 // dispose: leave the frame in place
    data[25] = 0 // blend: replace rather than composite
    return chunk('fcTL', data)
  }

  const parts = [
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('acTL', actl)
  ]

  // The first frame is the image itself, in IDAT, so anything that does not
  // understand APNG still shows a sensible still.
  parts.push(fctl(frames[0].delayMs))
  parts.push(chunk('IDAT', deflateSync(rawScanlines(width, height, frames[0].rgba), { level: 9 })))

  for (const frame of frames.slice(1)) {
    parts.push(fctl(frame.delayMs))
    const seq = Buffer.alloc(4)
    seq.writeUInt32BE(sequence++, 0)
    const body = deflateSync(rawScanlines(width, height, frame.rgba), { level: 9 })
    parts.push(chunk('fdAT', Buffer.concat([seq, body])))
  }

  parts.push(chunk('IEND', Buffer.alloc(0)))
  return Buffer.concat(parts)
}
