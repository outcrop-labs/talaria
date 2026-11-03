import assert from 'node:assert/strict'
import { test } from 'vitest'
import { readout, tabWorking } from './tab-working'
import type { Working } from './working.svelte'

const lead = (label: string, href: string) => ({ count: 1, label, href })

const TABS = [
  { to: '/home', label: 'Inbox' },
  { to: '/comms', label: 'Comms' },
  { to: '/work', label: 'Work' },
  { to: '/boards', label: 'Boards' },
  { to: '/knowledge', label: 'Knowledge' },
  { to: '/artifacts', label: 'Files' },
]

const busy: Working = {
  comms: lead('Release notes', '/comms/agent/m/1'),
  work: lead('Migrate auth', '/work/2'),
  boards: lead('Ticket 9', '/boards/b/9'),
}

test('only comms, work and boards tabs ever take a glyph', () => {
  const glyphs = TABS.filter((t) => tabWorking(t.to, busy)).map((t) => t.to)
  assert.deepEqual(glyphs, ['/comms', '/work', '/boards'])
})

test('a failed read draws no glyph and no readout', () => {
  for (const t of TABS) assert.equal(tabWorking(t.to, null), null)
  assert.equal(readout(TABS, null, '/work'), null)
})

test('an idle or missing arm draws nothing', () => {
  const partial = { comms: null, work: null } as unknown as Working
  for (const t of TABS) assert.equal(tabWorking(t.to, partial), null)
  assert.equal(readout(TABS, partial, '/home'), null)
})

test('the readout counts views other than the open one and leads with the first in bar order', () => {
  const r = readout(TABS, busy, '/comms/agent/m/1')!
  assert.equal(r.elsewhere, 2)
  assert.equal(r.site, 'nav/tab-work')
  assert.equal(r.lead.href, '/work/2')
  assert.equal(r.title, 'Work: Migrate auth\nBoards: Ticket 9')
})

test('the open view alone working shows no readout', () => {
  const onlyWork: Working = { comms: null, work: busy.work, boards: null }
  assert.equal(readout(TABS, onlyWork, '/work'), null)
  assert.ok(tabWorking('/work', onlyWork))
})

test('a view the viewer has no tab for never counts', () => {
  const noBoards = TABS.filter((t) => t.to !== '/boards')
  const r = readout(noBoards, busy, '/home')!
  assert.equal(r.elsewhere, 2)
  assert.ok(!r.title.includes('Ticket 9'))
})

test('the Work harness at its legacy addresses is the open Work view', () => {
  const onlyWork: Working = { comms: null, work: busy.work, boards: null }
  for (const path of ['/work/2', '/plan/p1', '/research/run-1', '/research']) {
    assert.equal(readout(TABS, onlyWork, path), null, path)
  }
})
