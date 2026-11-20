// The ClickUp sync's board pieces that hold no query, server-rendered: the
// status mapping table (every row offers every column, says what an
// agent-start or done target means, and names the intake for an unmapped row)
// and the remote-state chip on cards, rows and the ticket. The light CI ui job
// runs this on every pull request; the browser run against a ClickUp
// stand-in is in the pull request.
import { render } from 'svelte/server'
import { describe, expect, it, vi } from 'vitest'

vi.mock('@tanstack/svelte-query', () => ({ createQuery: vi.fn(), useQueryClient: vi.fn() }))

import RemoteStateChip from './RemoteStateChip.svelte'
import SyncMappingTable from './SyncMappingTable.svelte'
import { SETTINGS_TABS, settingsTabOf, ticketSearch } from './board-settings'
import type { MappingRow, SyncTarget } from '@/lib/board-sync'

const targets: SyncTarget[] = [
  { key: 'inbox', label: 'Inbox', category: 'open', agentStart: false, terminal: false },
  { key: 'in_progress', label: 'In progress', category: 'active', agentStart: true, terminal: false },
  { key: 'done', label: 'Done', category: 'done', agentStart: false, terminal: true },
]
const rows: MappingRow[] = [
  { name: 'in progress', key: 'in progress', color: '#4194f6', type: 'custom', stale: false },
  { name: 'complete', key: 'complete', color: '#6bc950', type: 'closed', stale: false },
  { name: 'QA', key: 'qa', color: null, type: 'custom', stale: false },
  { name: 'retired', key: 'retired', color: null, type: null, stale: true },
]

const table = (draft: Record<string, string>, disabled = false, intake: SyncTarget | null = targets[0]!) =>
  render(SyncMappingTable, { props: { rows, targets, intake, draft, disabled } }).body

describe('SyncMappingTable', () => {
  it('draws one row per status, each a select offering the unmapped intake and every column', () => {
    const body = table({ 'in progress': 'in_progress', complete: 'done' })
    expect(body.match(/data-mapping-row=/g)).toHaveLength(4)
    expect(body.match(/<select/g)).toHaveLength(4)
    expect(body).toContain('aria-label="Column for the ClickUp status QA"')
    expect(body).toContain('Inbox (unmapped)')
    expect(body).toContain('In progress — starts agent work')
    expect(body).toContain('Done — done')
    expect(body).toContain('no longer in the list')
  })

  it('says what each target means: agent pickup, a done close, and where an unmapped status lands', () => {
    const body = table({ 'in progress': 'in_progress', complete: 'done' })
    expect(body).toMatch(/In progress starts agent work\. The sync never adds agents/)
    expect(body).toMatch(/Done is a done column: a task closed in ClickUp closes its ticket here, and a workchain step hands on/)
    expect(body).toMatch(/Unmapped: lands in Inbox and is listed as a problem/)
    // No intake column (Inbox deleted): an unmapped status is refused, and says so.
    expect(table({}, false, null)).toContain('Unmapped — refused')
  })

  it('is read-only when disabled (an editor reads the table, an owner edits it)', () => {
    // The attribute, not the `disabled:` utility classes every Select carries.
    const attr = /<select[^>]*\sdisabled(?=[\s=>])/g
    expect(table({}, true).match(attr)).toHaveLength(4)
    expect(table({}, false).match(attr)).toBeNull()
  })
})

describe('RemoteStateChip', () => {
  it('names a task archived, deleted, moved or hidden in ClickUp, and nothing otherwise', () => {
    const chip = (state: string | null) => render(RemoteStateChip, { props: { state } }).body
    expect(chip('archived')).toContain('Archived in ClickUp')
    expect(chip('deleted')).toContain('Deleted in ClickUp')
    expect(chip('moved')).toContain('Moved to another ClickUp list')
    expect(chip('hidden')).toContain('No longer visible to the sync')
    expect(chip('archived')).toContain('never deletes it')
    expect(chip('active')).not.toContain('<span')
    expect(chip(null)).not.toContain('<span')
  })
})

describe('the settings deep link', () => {
  it('opens only a tab that exists, the sync tab among them', () => {
    expect(SETTINGS_TABS).toContain('sync')
    expect(settingsTabOf('sync')).toBe('sync')
    expect(settingsTabOf('general')).toBe('general')
    expect(settingsTabOf('nope')).toBeNull()
    expect(settingsTabOf(true)).toBeNull()
    expect(settingsTabOf(undefined)).toBeNull()
  })
})

describe('a ticket opened from the sync chip or the sync tab', () => {
  it('keeps the board’s view and filters, and drops only the settings modal', () => {
    expect(ticketSearch('view=list&group=priority&q=login&settings=sync')).toBe('view=list&group=priority&q=login')
    expect(ticketSearch('?v=abc&status=open')).toBe('v=abc&status=open')
    expect(ticketSearch('settings=sync')).toBe('')
    expect(ticketSearch('')).toBe('')
  })
})
