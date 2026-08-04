// Which way the pet turns.
//
// The atlas is laid out clockwise from straight up, and the mapping from an
// offset to a cell is the one part of it that can be wrong without looking
// wrong: a pet facing the wrong way still animates perfectly.
import test from 'node:test'
import assert from 'node:assert/strict'
import { lookFrame, LOOK_ROWS } from '../src/pet.js'

const cell = (dx, dy) => {
  const found = lookFrame(dx, dy)
  return found && `${found.row}:${found.frame}`
}

test('the compass points where the contract says it does', () => {
  // Screen coordinates: y grows downwards, so "up" is negative.
  assert.equal(cell(0, -100), `${LOOK_ROWS[0]}:0`, 'straight up is the first frame')
  assert.equal(cell(100, 0), `${LOOK_ROWS[0]}:4`, 'right is a quarter of the way round')
  assert.equal(cell(0, 100), `${LOOK_ROWS[1]}:0`, 'down starts the second row')
  assert.equal(cell(-100, 0), `${LOOK_ROWS[1]}:4`, 'left is three quarters round')
  assert.equal(cell(100, -100), `${LOOK_ROWS[0]}:2`, 'up and right is the diagonal between them')
})

test('every direction lands on its own cell', () => {
  const seen = new Set()
  for (let step = 0; step < 16; step += 1) {
    const radians = (step * 22.5 * Math.PI) / 180
    seen.add(cell(Math.sin(radians) * 50, -Math.cos(radians) * 50))
  }
  assert.equal(seen.size, 16, 'sixteen directions, sixteen poses')
  assert.ok(!seen.has(null))
})

test('a cursor on top of the pet is not a direction', () => {
  // Otherwise it snaps between opposite poses as the cursor crosses the centre.
  assert.equal(lookFrame(0, 0), null)
  assert.equal(lookFrame(2, -2), null)
  assert.equal(lookFrame(null, null), null, 'and neither is no cursor at all')
})

test('the nearest pose wins, not the one it passed on the way', () => {
  // A hair past the boundary between two poses picks the further one.
  assert.equal(cell(0.01, -100), `${LOOK_ROWS[0]}:0`)
  assert.equal(cell(45, -100), `${LOOK_ROWS[0]}:1`)
})
