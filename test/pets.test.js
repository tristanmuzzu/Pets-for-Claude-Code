// The built-in pets have to agree with their own atlases.
//
// A frame count that does not match the image is invisible until the pet
// reaches that row at runtime and draws a slice of empty space, which, being
// a pet, is where nobody is looking closely.
import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, readdirSync } from 'node:fs'
import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const petsDir = resolve(root, 'public/pets')
const pets = readdirSync(petsDir)

/** Width and height out of a PNG's IHDR, without a decoder. */
function pngSize(path) {
  const bytes = readFileSync(path)
  assert.equal(bytes.readUInt32BE(12), 0x49484452, `${path} does not start with an IHDR`)
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) }
}

test('there are pets to check', () => {
  assert.ok(pets.length > 0)
})

for (const id of pets) {
  test(`${id} manifest matches its atlas`, () => {
    const dir = resolve(petsDir, id)
    const manifest = JSON.parse(readFileSync(resolve(dir, 'pet.json'), 'utf8'))
    const image = pngSize(resolve(dir, manifest.spritesheetPath))

    assert.equal(manifest.id, id, 'the folder name is the id')
    assert.equal(image.width, manifest.columns * manifest.frameWidth, 'atlas width')
    assert.equal(image.height, manifest.rows * manifest.frameHeight, 'atlas height')

    assert.equal(manifest.frameCounts.length, manifest.rows, 'one frame count per row')
    for (const [row, count] of manifest.frameCounts.entries()) {
      assert.ok(count >= 1, `row ${row} has no frames`)
      assert.ok(count <= manifest.columns, `row ${row} claims more frames than the atlas holds`)
    }

    // Timings are optional, but a row that has them must have one per frame or
    // the loop silently falls back to a flat rate partway through.
    // A version 2 atlas ends with two rows of look directions, which are still
    // poses rather than animations and so are timed by the cursor, not here.
    if (manifest.frameDurations) {
      assert.ok(
        manifest.frameDurations.length <= manifest.rows,
        'more timing rows than the atlas has'
      )
      assert.ok(
        manifest.frameDurations.length >= manifest.rows - 2,
        'animated rows must all be timed'
      )
      for (const [row, durations] of manifest.frameDurations.entries()) {
        assert.equal(
          durations.length,
          manifest.frameCounts[row],
          `row ${row} times a different number of frames than it has`
        )
        for (const ms of durations) {
          assert.ok(Number.isFinite(ms) && ms > 0, `row ${row} has a non-positive duration`)
        }
      }
    }
  })
}
