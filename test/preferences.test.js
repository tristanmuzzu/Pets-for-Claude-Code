import test from 'node:test'
import assert from 'node:assert/strict'
import { projectKey, sessionVisible, cardLayout } from '../src/derive.js'

test('project filters share roots across agents and preserve same-name repositories', () => {
  const a = {session_id:'codex:1', provider:'codex', project:'pet', project_root:'/a/pet/'}
  const b = {...a, session_id:'claude:1', provider:'claude', cwd:'/a/pet/worktree'}
  const prefs = {agentFilter:'all', hiddenProjects:[projectKey(a)]}
  assert.equal(projectKey(a),projectKey(b))
  assert.equal(sessionVisible(a,prefs),false)
  assert.equal(sessionVisible(b,prefs),false)
  assert.equal(sessionVisible({...a,project_root:'/b/pet'},prefs),true)
  assert.equal(projectKey({cwd:'C:\\Work\\Pet\\'}),projectKey({cwd:'c:/work/pet'}))
})
test('agent filters keep notices and hide scratch independently', () => {
  assert.equal(sessionVisible({session_id:'a',provider:'codex'},{agentFilter:'claude'}),false)
  assert.equal(sessionVisible({session_id:'a'},{agentFilter:'claude'}),true)
  assert.equal(sessionVisible({session_id:'notice',scratch:true},{agentFilter:'codex'}),true)
  assert.equal(sessionVisible({session_id:'a',scratch:true},{agentFilter:'all'}),false)
})
test('a pin beyond six slots stays full alongside urgent work', () => {
  const groups = Array.from({length:9},(_,i)=>({key:String(i),state:i===7?'waiting':'running'}))
  const {visible,full} = cardLayout(groups,groups.map(g=>g.key),{pinned:'8',promoted:'3'})
  assert.equal(visible.length,6)
  assert.ok(visible.includes('8') && visible.includes('7'))
  assert.ok(full.has('8') && full.has('7'))
  assert.equal(full.has('3'),false)
})
test('hidden pin reserves no space; resolved urgency returns to pin', () => {
  const groups = ['a','b','c','d'].map(key=>({key,state:'running'}))
  const layout = cardLayout(groups,['a','b','c','d'],{pinned:'hidden'})
  assert.deepEqual(layout.visible,['a','b','c','d'])
  assert.equal(layout.full.has('hidden'),false)
  const pinned = cardLayout(groups,['a','b','c','d'],{pinned:'d'})
  assert.equal(pinned.full.has('d'),true)
  assert.equal(pinned.full.has('a'),false)
})
test('notice and pin do not suppress the urgent card', () => {
  const groups = [{key:'notice',state:'idle'},{key:'pin',state:'idle'},{key:'ask',state:'waiting'}]
  const {visible,full} = cardLayout(groups,['notice','ask','pin'],{pinned:'pin'})
  assert.equal(visible.length,3)
  assert.ok(full.has('notice') && full.has('pin') && full.has('ask'))
})
