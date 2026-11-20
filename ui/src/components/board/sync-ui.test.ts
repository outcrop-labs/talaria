// The ClickUp sync's board pieces that hold no query, server-rendered: the
// status mapping table (every row offers every column, says what an
// agent-start or done target means, and names the intake for an unmapped row)
// and the remote-state chip on cards, rows and the ticket, plus the board's
// sync chip as a phone-width row lays it out and the Segmented 24px floor the
// board's view switcher rides on. The light CI ui job
// runs this on every pull request; the browser run against a ClickUp
// stand-in is in the pull request.
import { render } from 'svelte/server'
import { describe, expect, it, vi } from 'vitest'

vi.mock('@tanstack/svelte-query', () => ({ createQuery: vi.fn(), useQueryClient: vi.fn() }))
// The chip's problem list links tickets through the router, which reads
// `location` at import; nothing here navigates.
vi.mock('sv-router', () => ({ searchParams: new URLSearchParams() }))
vi.mock('@/router', () => ({ navigate: vi.fn() }))

import BoardSyncChip from './BoardSyncChip.svelte'
import RemoteStateChip from './RemoteStateChip.svelte'
import SyncMappingTable from './SyncMappingTable.svelte'
import Segmented from '@/components/ui/Segmented.svelte'
import { SETTINGS_TABS, settingsTabOf, ticketSearch } from './board-settings'
import type { BoardSync, MappingRow, SyncTarget } from '@/lib/board-sync'

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

const pausedSync = {
  id: 's1',
  provider: 'clickup',
  state: 'paused',
  listId: 'l1',
  listPath: 'Engineering / Sprint',
  listUrl: null,
  workspaceId: null,
  account: null,
  credentialHint: 'ABCD',
  statusMap: {},
  mapVersion: 0,
  remapPending: false,
  options: { includeClosed: false, comments: true, attachments: true, notifyApps: false },
  intervalMinutes: 60,
  lastSyncedAt: null,
  nextSyncAt: null,
  lastSweepAt: null,
  overdue: false,
  schedulerArmed: true,
  liveRun: null,
  lastRun: null,
  connectedBy: null,
  pausedBy: { id: 'u1', name: 'Dana Reyes', email: null },
  pausedAt: null,
  warnings: [],
  problems: [],
  problemCount: 3,
  statuses: [],
  remoteStates: {},
  createdAt: '2026-11-20T12:00:00Z',
} satisfies BoardSync

describe('BoardSyncChip', () => {
  it('names a paused sync, and shrinks with its row so a phone cuts the label, never the chip', () => {
    const body = render(BoardSyncChip, { props: { boardId: 'b1', role: 'viewer', sync: pausedSync } }).body
    expect(body).toContain('aria-label="ClickUp sync: paused, 3 problems"')
    // The row's flex item may shrink below the label; the button caps at the
    // item's width and its label truncates, the problem count kept.
    expect(body).toMatch(/<div class="min-w-0 max-w-full" data-sync-chip-root/)
    expect(body).toMatch(/<button[^>]*class="[^"]*\bmin-w-0\b[^"]*max-w-\[min\(16rem,100%\)\]/)
  })
})

describe('Segmented', () => {
  it('keeps an sm cell at a 24px target even when its label is an icon', () => {
    const body = render(Segmented, { props: { options: [{ id: 'a', label: 'A' }], value: 'a', onChange: () => {} } }).body
    expect(body).toMatch(/<button[^>]*class="[^"]*\bmin-h-6\b/)
  })
})
