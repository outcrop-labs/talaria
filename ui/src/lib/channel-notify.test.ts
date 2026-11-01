import assert from 'node:assert/strict'
import { afterEach, test, vi } from 'vitest'
import {
  LEVELS,
  MUTE_PRESETS,
  isMuted,
  levelLabel,
  muteLabel,
  muteUntil,
  mutedLabel,
  putChannelNotify,
  readChannelNotify,
} from './channel-notify'
import { FOREVER } from './notify-wire'

afterEach(() => vi.unstubAllGlobals())

const NOW = Date.parse('2026-11-01T15:00:00.000Z')

test('the levels and presets are the menu, in order', () => {
  assert.deepEqual([...LEVELS], ['all', 'mentions', 'none'])
  assert.deepEqual(LEVELS.map(levelLabel), ['All messages', 'Mentions only', 'Nothing'])
  assert.deepEqual(MUTE_PRESETS.map(muteLabel), [
    'Mute for 1 hour',
    'Mute for 8 hours',
    'Mute for 24 hours',
    'Mute until I turn it back on',
  ])
})

test('a preset mutes until that far from now; forever is the contract FOREVER', () => {
  assert.equal(muteUntil('1h', NOW), '2026-11-01T16:00:00.000Z')
  assert.equal(muteUntil('8h', NOW), '2026-11-01T23:00:00.000Z')
  assert.equal(muteUntil('24h', NOW), '2026-11-02T15:00:00.000Z')
  assert.equal(muteUntil('forever', NOW), FOREVER)
  assert.equal(FOREVER, '9999-12-31T23:59:59.999Z')
})

test('the served notify is read defensively — an older api has none', () => {
  assert.deepEqual(readChannelNotify({ notify: { level: 'none', mutedUntil: null, settable: true } }), {
    level: 'none',
    mutedUntil: null,
    settable: true,
  })
  // Absent settable means a member row (the field rides from W2-P on).
  assert.equal(readChannelNotify({ notify: { level: 'all', mutedUntil: '2026-11-01T16:00:00.000Z' } })?.settable, true)
  assert.equal(readChannelNotify({ notify: { level: 'all', mutedUntil: null, settable: false } })?.settable, false)
  // No notify at all, a bad level, not an object: the control stays hidden.
  assert.equal(readChannelNotify({ role: 'member', members: [] }), null)
  assert.equal(readChannelNotify({ notify: { level: 'loud' } }), null)
  assert.equal(readChannelNotify(null), null)
  assert.equal(readChannelNotify('x'), null)
})

test('a mute runs until its instant, and the header says until when', () => {
  const at = (iso: string | null) => ({ mutedUntil: iso })
  assert.equal(isMuted(at(null), NOW), false)
  assert.equal(isMuted(at('2026-11-01T14:59:59.000Z'), NOW), false)
  assert.equal(isMuted(at('2026-11-01T16:00:00.000Z'), NOW), true)
  assert.equal(isMuted(at('garbage'), NOW), false)
  const o = { locale: 'en-US', timeZone: 'UTC' }
  assert.equal(mutedLabel(at('2026-11-01T16:00:00.000Z'), NOW, o), 'Muted until 4:00 PM')
  assert.equal(mutedLabel(at('2026-11-02T15:00:00.000Z'), NOW, o), 'Muted until Mon 3:00 PM')
  assert.equal(mutedLabel(at(FOREVER), NOW, o), 'Muted until you unmute')
  assert.equal(mutedLabel(at(null), NOW, o), null)
  assert.equal(mutedLabel(at('2026-11-01T14:00:00.000Z'), NOW, o), null)
})

test('the write goes to the channel notify route and answers what was stored', async () => {
  const calls: { url: string; init: RequestInit }[] = []
  vi.stubGlobal('fetch', async (url: string, init: RequestInit) => {
    calls.push({ url, init })
    return new Response(JSON.stringify({ ok: true, level: 'mentions', mutedUntil: '2026-11-01T23:00:00.000Z' }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  })
  const r = await putChannelNotify('c-1', { mutedUntil: '2026-11-01T23:00:00.000Z' })
  assert.deepEqual(r, { level: 'mentions', mutedUntil: '2026-11-01T23:00:00.000Z' })
  const [call] = calls
  assert.ok(call)
  assert.equal(call.url, '/api/channels/c-1/notify')
  assert.equal(call.init.method, 'PUT')
  assert.deepEqual(JSON.parse(String(call.init.body)), { mutedUntil: '2026-11-01T23:00:00.000Z' })
})

test('a refusal rejects with the server sentence (403 non-member, 409 team grant)', async () => {
  vi.stubGlobal('fetch', async () =>
    new Response(JSON.stringify({ error: 'forbidden' }), { status: 403, headers: { 'content-type': 'application/json' } }),
  )
  await assert.rejects(putChannelNotify('c-1', { level: 'none' }), /forbidden/)
})
