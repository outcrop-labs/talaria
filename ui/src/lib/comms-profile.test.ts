import assert from 'node:assert/strict'
import { test } from 'vitest'
import {
  conversationGlyph,
  conversationSelection,
  firstName,
  localTimeLabel,
  presenceLabel,
  profileWithPath,
  type SharedConversation,
} from './comms-profile'

const conv = (over: Partial<SharedConversation> = {}): SharedConversation => ({
  id: 'c-1',
  kind: 'channel',
  name: 'general',
  lastAt: '2026-10-05T12:00:00.000Z',
  unreadCount: 0,
  ...over,
})

// ── local time ──────────────────────────────────────────────────────────────

test('local time reads in THEIR zone, with the "local time" suffix', () => {
  const now = new Date('2026-10-05T14:12:00.000Z')
  assert.equal(localTimeLabel('UTC', now, 'en-US'), '2:12 PM local time')
  // Kolkata is UTC+5:30 — the half-hour offset is the case a naive hour shift misses.
  assert.equal(localTimeLabel('Asia/Kolkata', now, 'en-US'), '7:42 PM local time')
  assert.equal(localTimeLabel('America/New_York', now, 'en-US'), '10:12 AM local time')
})

test('no zone, or a zone this runtime cannot resolve, shows nothing', () => {
  const now = new Date('2026-10-05T14:12:00.000Z')
  assert.equal(localTimeLabel(null, now), null)
  assert.equal(localTimeLabel(undefined, now), null)
  assert.equal(localTimeLabel('', now), null)
  assert.equal(localTimeLabel('Mars/Olympus_Mons', now), null)
})

// ── first name ──────────────────────────────────────────────────────────────

test('first name is the first word of the display name', () => {
  assert.equal(firstName('Maya Chen', 'maya@x.co'), 'Maya')
  assert.equal(firstName('  Priya   Nair ', null), 'Priya')
  assert.equal(firstName('Zach', null), 'Zach')
})

test('no name falls back to the email local part, then to "them"', () => {
  assert.equal(firstName(null, 'jordan.ellis@x.co'), 'jordan.ellis')
  assert.equal(firstName('   ', 'jordan@x.co'), 'jordan')
  assert.equal(firstName(null, null), 'them')
})

// ── conversation rows ───────────────────────────────────────────────────────

test('a channel reads #, a relay ⇄, a DM and an agent thread take an avatar', () => {
  assert.equal(conversationGlyph('channel'), '#')
  assert.equal(conversationGlyph('group'), '⇄')
  assert.equal(conversationGlyph('dm'), null)
  assert.equal(conversationGlyph('agent'), null)
})

test('a room opens its channel; an agent thread opens under its agent', () => {
  assert.deepEqual(conversationSelection(conv({ id: 'd-1', kind: 'dm' }), null), { t: 'channel', id: 'd-1' })
  assert.deepEqual(conversationSelection(conv({ id: 'g-1', kind: 'group' }), 'hermes'), { t: 'channel', id: 'g-1' })
  assert.deepEqual(conversationSelection(conv({ id: 'x-9', kind: 'agent' }), 'hermes'), {
    t: 'agent',
    model: 'hermes',
    conversationId: 'x-9',
  })
  // An agent thread with no agent to hang it on has nowhere to go.
  assert.equal(conversationSelection(conv({ kind: 'agent' }), null), null)
})

test('presence reads Active or Away', () => {
  assert.equal(presenceLabel('online'), 'Active')
  assert.equal(presenceLabel('offline'), 'Away')
})

test('the all-conversations path encodes its subject', () => {
  assert.equal(profileWithPath({ kind: 'person', userId: 'u-1' }), '/comms/with/person/u-1')
  assert.equal(profileWithPath({ kind: 'agent', model: 'a/b' }), '/comms/with/agent/a%2Fb')
})
