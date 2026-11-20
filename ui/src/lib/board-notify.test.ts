import assert from 'node:assert/strict'
import { afterEach, test, vi } from 'vitest'
import {
  QUICK_PRESETS,
  availableEvents,
  channelChoice,
  channelValue,
  controlName,
  expandWrite,
  glyphOf,
  levelWords,
  presetChoice,
  presetLabel,
  putBoardNotify,
  refusalMessage,
  saveBoardDefault,
  sourceWords,
  summary,
  writeFrom,
  type BoardCadence,
  type BoardEffective,
  type BoardPreset,
  type BoardRegistry,
  type BoardSource,
} from './board-notify'
import { HttpError } from './fetch-json'

afterEach(() => vi.unstubAllGlobals())

// The registry as the server serves it today (talaria_notify::boards
// registry_wire): every event available — Due since the due-date reminders —
// and the daily digest listed but not available: it was withdrawn before
// release (Home's Inbox is the once-a-day view), so nothing here offers it.
const REGISTRY: BoardRegistry = {
  events: [
    { id: 'created', label: 'New ticket on the board', available: true, boardWideOnly: true },
    { id: 'assigned', label: 'Assigned to me', available: true, boardWideOnly: false },
    { id: 'status', label: 'Status change', available: true, boardWideOnly: false },
    { id: 'comment', label: 'Comment or reply', available: true, boardWideOnly: false },
    { id: 'mention', label: 'Mention', available: true, boardWideOnly: false },
    { id: 'due', label: 'Due soon or overdue', available: true, boardWideOnly: false },
    { id: 'completed', label: 'Ticket completed', available: true, boardWideOnly: false },
  ],
  presets: [
    { id: 'all', label: 'All activity', events: ['created', 'assigned', 'status', 'comment', 'mention', 'due', 'completed'], scope: 'board' },
    { id: 'participating', label: 'Participating', events: ['assigned', 'status', 'comment', 'mention', 'completed'], scope: 'mine' },
    { id: 'mentions', label: 'Mentions & assignments', events: ['assigned', 'mention'], scope: 'mine' },
    { id: 'off', label: 'Off', events: [], scope: 'mine' },
    { id: 'custom', label: 'Custom', events: null, scope: null },
  ],
  cadences: [
    { id: 'instant', label: 'Instant', available: true },
    { id: 'digest', label: 'Daily digest', available: false },
  ],
}

// An older server's registry: Due listed, not yet available (it refuses it),
// so the client must not offer or promise it.
const OLDER: BoardRegistry = {
  ...REGISTRY,
  events: REGISTRY.events.map((e) => (e.id === 'due' ? { ...e, available: false } : e)),
}

// A testing build that served the digest, between its merge and its
// withdrawal: the registry marked it available and a read could say digest.
// The client still offers and says nothing of it.
const DIGEST_SERVED: BoardRegistry = {
  ...REGISTRY,
  cadences: REGISTRY.cadences.map((c) => ({ ...c, available: true })),
}

const NO_CHANNELS = { alerts: null, email: null }
const expand = (preset: BoardPreset, cadence: BoardCadence = 'instant', over: Partial<BoardEffective> = {}): BoardEffective => {
  const p = REGISTRY.presets.find((x) => x.id === preset)!
  return { preset, events: p.events ?? ['status'], scope: p.scope ?? 'mine', cadence, channels: NO_CHANNELS, ...over }
}

const PRESETS: BoardPreset[] = ['all', 'participating', 'mentions', 'off', 'custom']
// Both ids the wire can carry, so a stray digest from a server is covered.
const CADENCES: BoardCadence[] = ['instant', 'digest']
const SOURCES: BoardSource[] = ['board', 'default', 'builtin']

test('the quick presets are the menu, in order, worded by the registry', () => {
  assert.deepEqual([...QUICK_PRESETS], ['all', 'participating', 'mentions', 'off'])
  assert.deepEqual(QUICK_PRESETS.map((p) => presetLabel(REGISTRY, p)), ['All activity', 'Participating', 'Mentions & assignments', 'Off'])
  // The registry's word wins: a server that renames a preset renames it here.
  const renamed = { ...REGISTRY, presets: REGISTRY.presets.map((p) => (p.id === 'off' ? { ...p, label: 'Silent' } : p)) }
  assert.equal(presetLabel(renamed, 'off'), 'Silent')
  // With no registry at all, the fallback still names every preset.
  for (const p of PRESETS) assert.ok(presetLabel(null, p).length > 0)
})

test('only what has a producer is offered: today every event, on an older server not Due', () => {
  assert.deepEqual(
    availableEvents(REGISTRY).map((e) => e.id),
    ['created', 'assigned', 'status', 'comment', 'mention', 'due', 'completed'],
  )
  assert.ok(!availableEvents(OLDER).some((e) => e.id === 'due'))
  assert.deepEqual(availableEvents(null), [])
})

test('no daily digest: no word of it, and every write is instant — even from a server that served one', () => {
  for (const registry of [REGISTRY, DIGEST_SERVED]) {
    for (const preset of PRESETS) {
      for (const source of SOURCES) {
        const effective = expand(preset, 'digest')
        // The bell's name and the summary say what the setting is, never a digest.
        assert.doesNotMatch(controlName({ effective, source }, registry), /digest/i, `${preset}/${source}`)
        assert.doesNotMatch(levelWords(effective, registry), /digest/i)
        for (const channels of [false, true]) {
          const line = summary(effective, registry, channels)
          assert.doesNotMatch(line, /digest|once a day/i, `${preset} summary`)
          assert.equal(line, summary(expand(preset), registry, channels), `${preset}: the digest reads as instant`)
        }
      }
      // A change from a setting a server reported as a digest writes instant.
      for (const other of QUICK_PRESETS) {
        assert.equal(writeFrom(expand(preset, 'digest'), { preset: other }, registry).cadence, 'instant', `${preset}→${other}`)
      }
      assert.equal(writeFrom(expand(preset, 'digest'), { channels: { email: true } }, registry).cadence, 'instant')
    }
  }
  // And what a write shows while it is on its way is instant too.
  assert.equal(expandWrite({ preset: 'all' }, REGISTRY).cadence, 'instant')
})

test('every preset × cadence × source: one glyph, one name, tooltip and label agree', () => {
  const glyphs: Record<BoardPreset, string> = { all: 'bell-ring', participating: 'bell', mentions: 'at-sign', off: 'bell-off', custom: 'bell-dot' }
  for (const preset of PRESETS) {
    for (const cadence of CADENCES) {
      for (const source of SOURCES) {
        const effective = expand(preset, cadence)
        assert.equal(glyphOf(effective), glyphs[preset], `${preset} glyph`)
        const name = controlName({ effective, source }, REGISTRY)
        const level = presetLabel(REGISTRY, preset).toLowerCase()
        const inherited = source === 'board' ? '' : ' (your default)'
        assert.equal(name, `Board notifications: ${level}${inherited}`, `${preset}/${cadence}/${source}`)
        assert.equal(levelWords(effective, REGISTRY), level)
        assert.ok(sourceWords(source).length > 0)
      }
    }
  }
  assert.equal(controlName(null), 'Board notifications')
  // The words for where it comes from.
  assert.equal(sourceWords('board'), 'Set for this board')
  assert.equal(sourceWords('default'), 'Your default for every board')
  assert.equal(sourceWords('builtin'), 'The standard default')
})

test('the summary says what reaches you, over which tickets — and Off what still does', () => {
  assert.equal(
    summary(expand('participating'), REGISTRY, false),
    'Assigned to me, Status change, Comment or reply, Mention and Ticket completed — on tickets you’re in, as it happens.',
  )
  assert.equal(summary(expand('mentions'), REGISTRY, false), 'Assigned to me and Mention — on tickets you’re in, as it happens.')
  // All names Due among what it brings, now that Due has a producer.
  assert.equal(
    summary(expand('all'), REGISTRY, false),
    'New ticket on the board, Assigned to me, Status change, Comment or reply, Mention, Due soon or overdue and Ticket completed — on every ticket on the board, as it happens.',
  )
  // An older server with no Due producer: All covers it, the sentence does not promise it.
  assert.match(summary(expand('all'), OLDER, false), /— on every ticket on the board, as it happens\.$/)
  assert.doesNotMatch(summary(expand('all'), OLDER, false), /Due soon/)
  assert.match(summary(expand('off'), REGISTRY, true), /^Nothing from this board — assignments and @mentions included\. Approvals, and tickets you watch yourself, still reach you\.$/)
  assert.equal(summary(expand('custom', 'instant', { events: ['comment'] }), REGISTRY, false), 'Comment or reply — on tickets you’re in, as it happens.')
  // The board's own channels, only when the server serves them.
  assert.match(summary(expand('participating'), REGISTRY, true), / Alerts and email follow Settings → Notifications\.$/)
  assert.match(
    summary(expand('participating', 'instant', { channels: { alerts: false, email: true } }), REGISTRY, true),
    / Alerts off, Email on for this board\.$/,
  )
  assert.match(summary(expand('participating', 'instant', { channels: { alerts: true, email: null } }), REGISTRY, true), / Alerts on for this board\.$/)
  // The default speaks of the boards that follow it.
  assert.match(summary(expand('off'), REGISTRY, true, 'default'), /^Nothing from the boards that follow it — /)
  assert.match(
    summary(expand('mentions', 'instant', { channels: { alerts: null, email: false } }), REGISTRY, true, 'default'),
    / Email off for the boards that follow it\.$/,
  )
})

test('a channel is three states: from your settings, on, off', () => {
  for (const v of [null, true, false] as const) assert.equal(channelValue(channelChoice(v)), v)
  assert.equal(channelChoice(undefined), 'settings')
})

test('a change becomes the whole setting, keeping what was not touched', () => {
  // A preset from the menu keeps the cadence and the board's channels.
  const mailed = expand('participating', 'instant', { channels: { alerts: null, email: true } })
  assert.deepEqual(writeFrom(mailed, { preset: 'off' }, REGISTRY), { preset: 'off', cadence: 'instant', channels: { email: true } })
  // Nothing chosen on the channels: none sent (null is Settings → Notifications).
  assert.deepEqual(writeFrom(expand('off'), { preset: 'all' }, REGISTRY), { preset: 'all', cadence: 'instant' })
  // A channel set back to "from your settings" leaves the write.
  assert.deepEqual(writeFrom(mailed, { channels: { email: null } }, REGISTRY), { preset: 'participating', cadence: 'instant' })
  assert.deepEqual(writeFrom(mailed, { channels: { alerts: false } }, REGISTRY), {
    preset: 'participating',
    cadence: 'instant',
    channels: { alerts: false, email: true },
  })
})

test('choosing the preset already in effect sends nothing — an inherited default stays inherited', () => {
  // Every quick preset × where it comes from: the checked entry is no write,
  // so a board that follows the default keeps following it (source stays).
  for (const p of QUICK_PRESETS) {
    for (const cadence of CADENCES) {
      const current = expand(p, cadence, { channels: { alerts: null, email: true } })
      assert.equal(presetChoice(current, p, REGISTRY), null, `${p}/${cadence}`)
      // Any other preset is the whole setting, as writeFrom builds it.
      for (const other of QUICK_PRESETS.filter((x) => x !== p)) {
        assert.deepEqual(presetChoice(current, other, REGISTRY), writeFrom(current, { preset: other }, REGISTRY), `${p}→${other}`)
      }
    }
  }
  // Leaving Custom for a named preset is a change.
  assert.deepEqual(presetChoice(expand('custom', 'instant', { events: ['status'] }), 'participating', REGISTRY), {
    preset: 'participating',
    cadence: 'instant',
  })
})

test('Custom starts from what applies, never from nothing, and never asks the impossible', () => {
  // From a named preset: its events and its scope.
  assert.deepEqual(writeFrom(expand('mentions'), { preset: 'custom' }, REGISTRY), {
    preset: 'custom',
    cadence: 'instant',
    events: ['assigned', 'mention'],
    scope: 'mine',
  })
  // From Off (no events): the built-in's, which the server accepts.
  assert.deepEqual(writeFrom(expand('off'), { preset: 'custom' }, REGISTRY).events, ['assigned', 'status', 'comment', 'mention', 'completed'])
  // From All: the whole board, with New ticket kept.
  const fromAll = writeFrom(expand('all'), { preset: 'custom' }, REGISTRY)
  assert.equal(fromAll.scope, 'board')
  assert.ok(fromAll.events?.includes('created'))
  // Narrowing to the tickets you're in drops New ticket (nobody is in a ticket
  // before it exists — the server refuses it).
  const custom = expand('custom', 'instant', { events: ['created', 'status'], scope: 'board' })
  assert.deepEqual(writeFrom(custom, { scope: 'mine' }, REGISTRY).events, ['status'])
  // Events land in registry order, once each.
  assert.deepEqual(writeFrom(custom, { events: ['completed', 'status', 'created', 'status'] }, REGISTRY).events, ['created', 'status', 'completed'])
  // A named preset carries no events or scope (the server refuses them there).
  const named = writeFrom(custom, { preset: 'participating' }, REGISTRY)
  assert.equal('events' in named, false)
  assert.equal('scope' in named, false)
})

test('a write expands the way the server answers it', () => {
  for (const preset of ['all', 'participating', 'mentions', 'off'] as const) {
    const w = writeFrom(expand('participating'), { preset }, REGISTRY)
    assert.deepEqual(expandWrite(w, REGISTRY), expand(preset))
  }
  const w = writeFrom(expand('all', 'instant', { channels: { alerts: true, email: null } }), { preset: 'custom', events: ['status'], scope: 'mine' }, REGISTRY)
  assert.deepEqual(expandWrite(w, REGISTRY), {
    preset: 'custom',
    events: ['status'],
    scope: 'mine',
    cadence: 'instant',
    channels: { alerts: true, email: null },
  })
})

test('a refused write says why: off the board, or the server’s own sentence', () => {
  assert.equal(refusalMessage(new HttpError(403, 'forbidden')), 'You’re no longer on this board, so its notifications can’t be changed.')
  assert.equal(refusalMessage(new HttpError(400, 'pick at least one event, or choose off')), 'pick at least one event, or choose off')
})

test('the writes go where the server reads them', async () => {
  const calls: Array<{ url: string; method: string; body: unknown }> = []
  vi.stubGlobal('fetch', async (url: string, init: RequestInit) => {
    calls.push({ url, method: init.method ?? 'GET', body: init.body ? JSON.parse(String(init.body)) : undefined })
    return new Response(JSON.stringify({ ok: true }), { status: 200, headers: { 'content-type': 'application/json' } })
  })
  await putBoardNotify('b 1', { preset: 'off' })
  await saveBoardDefault({ preset: 'mentions', cadence: 'instant' })
  await saveBoardDefault(null)
  assert.deepEqual(calls, [
    { url: '/api/boards/b%201/notify', method: 'PUT', body: { preset: 'off' } },
    { url: '/api/notifications', method: 'PATCH', body: { boards: { preset: 'mentions', cadence: 'instant' } } },
    { url: '/api/notifications', method: 'PATCH', body: { boards: null } },
  ])
})
