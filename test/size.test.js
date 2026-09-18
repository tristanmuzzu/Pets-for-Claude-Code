import test from 'node:test'
import assert from 'node:assert/strict'
import { sceneSize } from '../src/size.js'

test('S/M/L use the same layout and proportional sprite, bubble, and text sizes', () => {
  for (const [scale, factor] of [[1.5,.75],[2,1],[3,1.5]]) {
    assert.deepEqual(sceneSize(scale,360*factor,640*factor),{factor,width:360,height:640})
  }
})
test('a short display leaves large text large and gives the scene less vertical room', () => {
  assert.deepEqual(sceneSize(3,540,720),{factor:1.5,width:360,height:480})
})
test('a narrow viewport still fits full bubble width; invalid scale uses medium', () => {
  assert.deepEqual(sceneSize(3,300,600),{factor:300/360,width:360,height:720})
  assert.deepEqual(sceneSize(NaN,360,640),{factor:1,width:360,height:640})
})
