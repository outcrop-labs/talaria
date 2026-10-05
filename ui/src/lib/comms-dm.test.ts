import assert from 'node:assert/strict'
import { test } from 'vitest'
import { findDm, groupDmCount, groupDmLabel, isGroupDm } from './comms-dm'

const maya = { userId: 'u-maya', name: 'Maya Chen', email: 'maya@x.co' }
const jordan = { userId: 'u-jordan', name: 'Jordan Ellis', email: 'jordan@x.co' }
const pair = { id: 'd-pair', kind: 'dm', name: '', peer: maya, members: [maya], agents: [] }
const group = { id: 'd-group', kind: 'dm', name: '', peer: null, members: [maya, jordan], agents: ['atlas-operations'] }
const named = { ...group, id: 'd-named', name: 'Launch prep', agents: [] }
const channel = { id: 'c1', kind: 'channel', name: 'general', peer: null }
const label = (m: string) => (m === 'atlas-operations' ? 'Atlas' : m)

test('a group DM is a DM with no single peer', () => {
  assert.equal(isGroupDm(group), true)
  assert.equal(isGroupDm(pair), false)
  assert.equal(isGroupDm(channel), false)
})

test("a group DM is called by its name, else by everyone else's first names then its agents", () => {
  assert.equal(groupDmLabel(group, label), 'Maya, Jordan, Atlas')
  assert.equal(groupDmLabel(named, label), 'Launch prep')
  assert.equal(groupDmLabel({ kind: 'dm', name: '', members: [{ userId: 'u', name: null, email: 'sam@x.co' }] }, label), 'sam')
  assert.equal(groupDmLabel({ kind: 'dm', name: '' }, label), 'Direct message')
})

test('the badge counts everyone else, people and agents', () => {
  assert.equal(groupDmCount(group), 3)
  assert.equal(groupDmCount({ kind: 'dm', name: '' }), 0)
})

test('recipients find their existing DM — the same people and the same agents, in any order', () => {
  const all = [channel, pair, group, named]
  assert.equal(findDm(all, 'me', ['u-maya'], [])?.id, 'd-pair', 'one person is their plain DM')
  assert.equal(findDm(all, 'me', ['u-jordan', 'me', 'u-maya'], ['atlas-operations'])?.id, 'd-group')
  assert.equal(findDm(all, 'me', ['u-maya', 'u-jordan'], [])?.id, 'd-named')
  assert.equal(findDm(all, 'me', ['u-maya'], ['atlas-operations']), null, 'a different set is a new conversation')
  assert.equal(findDm(all, 'me', [], ['atlas-operations']), null, 'one agent alone is its own conversation')
  assert.equal(findDm(all, 'me', [], []), null)
})
