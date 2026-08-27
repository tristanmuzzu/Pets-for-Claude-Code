// When the sprite loop is allowed to stop.
//
// The loop is the app's single largest standing cost: sixty canvas repaints a
// second, forever, if nothing ever tells it to park. The settle clock used to
// anchor itself to `frame === 0`, which a cycling loop leaves within 300ms, so
// the five-second threshold was unreachable and the loop genuinely never
// stopped. These tests drive `tick` with a fabricated timeline and prove the
// three behaviours that matter: an idle pet parks, a working pet parks once
// events stop arriving, and a wake brings either back.
import test from 'node:test'
import assert from 'node:assert/strict'
import { PetRenderer } from '../src/pet.js'

/** Enough canvas for the renderer to construct and draw nothing. */
const stubCanvas = () => ({
  width: 0,
  height: 0,
  style: {},
  getContext: () => ({ clearRect: () => {}, drawImage: () => {} })
})

const raf = []
globalThis.requestAnimationFrame = (fn) => raf.push(fn)

/** A renderer mid-flight, as `start` plus one wake would leave it. */
function animating(state) {
  const pet = new PetRenderer(stubCanvas())
  pet.wanted = true
  pet.looping = true
  pet.state = state
  pet.wokeAt = 0
  return pet
}

/** Advance the clock in tick-sized steps, like the rAF loop would. */
function run(pet, from, until, step = 50) {
  for (let now = from + step; now <= until; now += step) {
    if (!pet.looping) return now
    pet.tick(step, now)
  }
  return until
}

test('an idle pet parks after five seconds', () => {
  const pet = animating('idle')
  run(pet, 0, 4000)
  assert.equal(pet.looping, true, 'still animating inside the threshold')
  run(pet, 4000, 8000)
  assert.equal(pet.looping, false, 'parked once the threshold passed')
  assert.equal(pet.frame, 0, 'parked at the top of the cycle, not mid-stride')
})

test('a working pet parks once events stop, and not before', () => {
  const pet = animating('running')
  run(pet, 0, 14_000)
  assert.equal(pet.looping, true, 'a working loop outlasts the idle threshold')
  run(pet, 14_000, 20_000)
  assert.equal(pet.looping, false, 'parked once the events went quiet')
  assert.equal(pet.frame, 0, 'parked at the top of the cycle')
})

test('events keep a working loop honest, and a wake restarts a parked one', () => {
  const pet = animating('running')
  let now = 0
  for (; now <= 60_000; now += 5000) {
    pet.wake(now)
    run(pet, now, now + 5000)
  }
  assert.equal(pet.looping, true, 'never parked while events kept arriving')

  run(pet, now, now + 30_000)
  assert.equal(pet.looping, false, 'parked when they stopped')
  pet.wake(now + 30_000)
  assert.equal(pet.looping, true, 'and the next event restarted it')
})
