import { describe, expect, it, vi } from 'vitest'

// svelte-query's barrel pulls .svelte files the node environment cannot
// transform; the one export this module uses is mocked (agent-design.test.ts
// does the same).
vi.mock('@tanstack/svelte-query', () => ({ createQuery: vi.fn() }))

import {
  ago,
  cadenceId,
  cadenceLabel,
  cadenceMinutes,
  chipView,
  clickupTaskId,
  clickupTaskUrl,
  landsIn,
  lastRunNote,
  mappingRows,
  mappingTargets,
  pollWhileLive,
  remoteStateLabel,
  resyncOutcome,
  runSummary,
  sameMap,
  statusMapOf,
  syncIntake,
  targetNote,
  until,
  type BoardSync,
} from './board-sync'
import type { BoardStatus } from './statuses'

const NOW = Date.parse('2026-11-20T12:00:00Z')
const iso = (offsetMs: number) => new Date(NOW + offsetMs).toISOString()
const MIN = 60_000

function sync(over: Partial<BoardSync> = {}): BoardSync {
  return {
    id: 's1',
    provider: 'clickup',
    state: 'active',
    listId: 'l1',
    listPath: 'Engineering / Sprint board',
    listUrl: null,
    workspaceId: null,
    account: { username: 'Sync Bot', email: 'bot@example.invalid' },
    credentialHint: 'ABCD',
    statusMap: {},
    mapVersion: 0,
    remapPending: false,
    options: { includeClosed: false, comments: true, attachments: true, notifyApps: false },
    intervalMinutes: 60,
    lastSyncedAt: iso(-12 * MIN),
    nextSyncAt: iso(48 * MIN),
    lastSweepAt: null,
    overdue: false,
    schedulerArmed: true,
    liveRun: null,
    lastRun: { id: 'r1', state: 'done', error: null, result: null, finishedAt: iso(-12 * MIN) },
    connectedBy: null,
    warnings: [],
    problems: [],
    problemCount: 0,
    statuses: [],
    remoteStates: {},
    createdAt: iso(-60 * MIN),
    ...over,
  }
}

// The shipped default board, as GET /api/boards/{id}/statuses serves it.
const DEFAULT_BOARD: BoardStatus[] = [
  { id: null, key: 'inbox', label: 'Inbox', color: 'slate', category: 'open', agentStart: false, position: 0 },
  { id: null, key: 'assigned', label: 'Assigned', color: 'blue', category: 'open', agentStart: true, position: 1 },
  { id: null, key: 'in_progress', label: 'In progress', color: 'amber', category: 'active', agentStart: true, position: 2 },
  { id: null, key: 'quality_review', label: 'Quality review', color: 'purple', category: 'review', agentStart: false, position: 3 },
  { id: null, key: 'done', label: 'Done', color: 'green', category: 'done', agentStart: false, position: 4 },
  { id: null, key: 'blocked', label: 'Blocked', color: 'red', category: 'blocked', agentStart: false, position: 5, system: true },
]

describe('time words', () => {
  it('says how long ago, and how long until, against a fixed clock', () => {
    expect(ago(iso(-10_000), NOW)).toBe('just now')
    expect(ago(iso(-12 * MIN), NOW)).toBe('12m ago')
    expect(ago(iso(-3 * 60 * MIN), NOW)).toBe('3h ago')
    expect(ago(iso(-3 * 24 * 60 * MIN), NOW)).toBe('3d ago')
    expect(ago(null, NOW)).toBeNull()
    expect(until(iso(40 * MIN), NOW)).toBe('in 40m')
    expect(until(iso(-MIN), NOW)).toBe('due now')
    expect(until(null, NOW)).toBeNull()
  })
})

describe('cadence', () => {
  it('round-trips every cadence the engine accepts, and names each one', () => {
    for (const m of [null, 15, 60, 1440]) expect(cadenceMinutes(cadenceId(m))).toBe(m)
    expect(cadenceLabel(null)).toMatch(/Manual/)
    expect(cadenceLabel(60)).toBe('Hourly')
    expect(cadenceLabel(1440)).toBe('Daily')
  })
})

describe('chipView — never silently stale', () => {
  it('a healthy scheduled sync says when it last ran and when it runs next', () => {
    const v = chipView(sync(), NOW)
    expect(v).toMatchObject({ tone: 'ok', text: 'synced 12m ago' })
    expect(v.detail).toContain('Next sync in 48m')
  })

  it('a refused credential wins over everything else, a running sync included', () => {
    const v = chipView(sync({ state: 'needs_credential', liveRun: { id: 'r', phase: 'x' } }), NOW)
    expect(v.tone).toBe('danger')
    expect(v.text).toBe('needs a new token')
  })

  it('a running sync shows its phase', () => {
    const v = chipView(sync({ liveRun: { id: 'r2', phase: 'Checking updated tasks — page 2' } }), NOW)
    expect(v).toEqual({ tone: 'live', text: 'syncing', detail: 'Checking updated tasks — page 2' })
  })

  it('a gone list and a failed run are failures', () => {
    const gone = sync({ problems: [{ kind: 'list-gone', detail: 'The ClickUp list is gone', fix: 'x' }] })
    expect(chipView(gone, NOW)).toMatchObject({ tone: 'danger', text: 'list not found' })
    const failed = sync({ lastRun: { id: 'r', state: 'error', error: 'boom', result: null, finishedAt: null } })
    expect(chipView(failed, NOW)).toMatchObject({ tone: 'danger', text: 'last sync failed' })
    expect(chipView(failed, NOW).detail).toContain('boom')
  })

  it('a per-ticket list-gone-shaped problem is not a run failure', () => {
    const v = chipView(sync({ problems: [{ kind: 'list-gone', detail: 'x', fix: 'y', ticketId: 't1' }] }), NOW)
    expect(v.tone).toBe('ok')
  })

  it('paused, overdue, manual, never-synced and schedule-off each have their own words', () => {
    expect(chipView(sync({ state: 'paused' }), NOW)).toMatchObject({ tone: 'idle', text: 'paused' })
    const overdue = chipView(sync({ overdue: true, nextSyncAt: iso(-20 * MIN) }), NOW)
    expect(overdue).toMatchObject({ tone: 'warn', text: 'sync overdue' })
    expect(overdue.detail).toContain('20m ago')
    const manual = chipView(sync({ intervalMinutes: null, nextSyncAt: null }), NOW)
    expect(manual.tone).toBe('ok')
    expect(manual.detail).toMatch(/only on Sync now/)
    expect(chipView(sync({ lastSyncedAt: null }), NOW)).toMatchObject({ tone: 'idle', text: 'not synced yet' })
    const off = chipView(sync({ schedulerArmed: false }), NOW)
    expect(off.tone).toBe('warn')
    expect(off.detail).toMatch(/Scheduled syncs are off/)
  })

  it('paused holds the schedule only, and says Sync now still runs', () => {
    expect(chipView(sync({ state: 'paused' }), NOW).detail).toMatch(/scheduled syncs are held; Sync now still runs\. Last synced 12m ago/)
    expect(chipView(sync({ state: 'paused', lastSyncedAt: null }), NOW).detail).toMatch(/has not synced yet/)
  })

  it('with the schedule off on the instance, a scheduled connection names the switch, not an overdue run', () => {
    // Nothing advances next_sync_at while the scheduler is off, so the
    // projection's `overdue` stays true for good — the switch is the reason.
    const off = chipView(sync({ schedulerArmed: false, overdue: true, nextSyncAt: iso(-3 * 60 * MIN) }), NOW)
    expect(off).toMatchObject({ tone: 'warn', text: 'synced 12m ago' })
    expect(off.detail).toMatch(/Scheduled syncs are off on this instance — use Sync now/)
    const fresh = chipView(sync({ schedulerArmed: false, overdue: true, lastSyncedAt: null }), NOW)
    expect(fresh).toMatchObject({ tone: 'warn', text: 'not synced yet' })
    expect(fresh.detail).toMatch(/Scheduled syncs are off/)
    // A manual connection is not scheduled, so the switch is not its story.
    expect(chipView(sync({ schedulerArmed: false, intervalMinutes: null }), NOW).tone).toBe('ok')
  })
})

describe('polling', () => {
  it('re-reads only while a run is going or a mapping edit waits', () => {
    expect(pollWhileLive(undefined)).toBe(false)
    expect(pollWhileLive({ sync: null, disconnected: null })).toBe(false)
    expect(pollWhileLive({ sync: sync(), disconnected: null })).toBe(false)
    expect(pollWhileLive({ sync: sync({ liveRun: { id: 'r', phase: '' } }), disconnected: null })).toBeGreaterThan(0)
    expect(pollWhileLive({ sync: sync({ remapPending: true }), disconnected: null })).toBeGreaterThan(0)
  })
})

describe('the mapping table', () => {
  const targets = mappingTargets([
    ...DEFAULT_BOARD,
    // Off-board statuses are never columns; a stray one is never offered.
    { id: null, key: 'failed', label: 'Failed', color: 'red', category: 'done', agentStart: false, position: 9 },
  ])

  it('offers every column and Blocked, agent-start ones included, never failed or cancelled', () => {
    expect(targets.map((t) => t.key)).toEqual(['inbox', 'assigned', 'in_progress', 'quality_review', 'done', 'blocked'])
    expect(targets.find((t) => t.key === 'in_progress')).toMatchObject({ agentStart: true, terminal: false })
    expect(targets.find((t) => t.key === 'done')).toMatchObject({ terminal: true })
  })

  it('the intake is the first open column that is not an agent pickup column', () => {
    expect(syncIntake(targets)?.key).toBe('inbox')
    // Inbox deleted: Assigned (agent-start) is never the intake.
    expect(syncIntake(targets.filter((t) => t.key !== 'inbox'))).toBeNull()
  })

  it('every row says what landing there means, and the unmapped row names the intake', () => {
    const intake = syncIntake(targets)
    expect(targetNote(null, intake)).toMatch(/lands in Inbox/)
    expect(targetNote(null, null)).toMatch(/refused/)
    expect(targetNote(targets.find((t) => t.key === 'in_progress')!, intake)).toMatch(/starts agent work/)
    expect(targetNote(targets.find((t) => t.key === 'done')!, intake)).toMatch(/workchain step hands on/)
    expect(targetNote(targets.find((t) => t.key === 'quality_review')!, intake)).toMatch(/never out of it/)
    expect(targetNote(targets.find((t) => t.key === 'blocked')!, intake)).toMatch(/a person unblocks/)
    expect(targetNote(targets.find((t) => t.key === 'inbox')!, intake)).toBeNull()
  })

  it('rows follow ClickUp’s order, then the saved statuses the list no longer has', () => {
    const rows = mappingRows(
      [
        { status: 'complete', type: 'closed', orderindex: 3 },
        { status: 'To Do', type: 'open', orderindex: 0, color: '#ddd' },
      ],
      { 'to do': 'inbox', retired: 'done' },
    )
    expect(rows.map((r) => [r.name, r.key, r.stale])).toEqual([
      ['To Do', 'to do', false],
      ['complete', 'complete', false],
      ['retired', 'retired', true],
    ])
  })

  it('the wire table drops unmapped rows and targets that are no longer columns, and lowercases keys', () => {
    expect(statusMapOf({ 'In Progress': 'in_progress', qa: '', gone: 'deleted_col' }, targets)).toEqual({
      'in progress': 'in_progress',
    })
    expect(sameMap({ a: 'x', b: 'y' }, { b: 'y', a: 'x' })).toBe(true)
    expect(sameMap({ a: 'x' }, { a: 'z' })).toBe(false)
    expect(sameMap({ a: 'x' }, { a: 'x', b: 'y' })).toBe(false)
  })

  it('a sample task lands where the edited table puts it, else in the intake', () => {
    expect(landsIn('QA', { qa: 'quality_review' }, 'inbox')).toEqual({ column: 'quality_review', unmapped: false })
    expect(landsIn('QA', {}, 'inbox')).toEqual({ column: 'inbox', unmapped: true })
    expect(landsIn('QA', {}, null)).toEqual({ column: null, unmapped: true })
  })
})

describe('runs and tickets', () => {
  it('a finished run reads as counts; a stopped one says why', () => {
    expect(runSummary({ created: 7, updated: 1, comments: 3, attachments: 1, checked: 8 })).toBe(
      '7 new · 1 updated · 3 comments · 1 file',
    )
    expect(runSummary({ checked: 8 })).toBe('8 checked, nothing changed')
    expect(runSummary({ stopped: 'the sync is paused' })).toBe('Stopped: the sync is paused.')
    expect(runSummary(null)).toBeNull()
  })

  it('a ticket or remap run is named for what it was, not read as the board’s last sync', () => {
    expect(lastRunNote({ mode: 'manual', created: 2, checked: 2 })).toBe('2 new')
    expect(lastRunNote({ mode: 'scheduled', checked: 8 })).toBe('8 checked, nothing changed')
    expect(lastRunNote({ mode: 'ticket', updated: 1, checked: 1 })).toBe('latest run re-synced one ticket')
    expect(lastRunNote({ mode: 'remap', updated: 3 })).toBe('latest run applied the mapping (3 updated)')
    expect(lastRunNote({ mode: 'remap' })).toBe('latest run applied the mapping')
    expect(lastRunNote({ stopped: 'the sync is paused' })).toBe('Stopped: the sync is paused.')
    expect(lastRunNote(null)).toBeNull()
  })

  it('a re-sync is over when its run finished, or when a later run took the latest slot', () => {
    const run = (id: string, state: 'running' | 'done' | 'error' | 'cancelled') =>
      ({ id, state, error: null, result: null, finishedAt: null }) as const
    expect(resyncOutcome(sync({ liveRun: { id: 'rs', phase: 'x' }, lastRun: run('rs', 'running') }), 'rs')).toBe('going')
    expect(resyncOutcome(sync({ lastRun: run('rs', 'done') }), 'rs')).toBe('done')
    expect(resyncOutcome(sync({ lastRun: run('rs', 'error') }), 'rs')).toBe('error')
    expect(resyncOutcome(sync({ lastRun: run('rs', 'cancelled') }), 'rs')).toBe('error')
    // The schedule held back behind the re-sync started the moment it ended.
    expect(resyncOutcome(sync({ liveRun: { id: 'next', phase: 'x' }, lastRun: run('next', 'running') }), 'rs')).toBe('ended')
    expect(resyncOutcome(sync({ lastRun: run('next', 'done') }), 'rs')).toBe('ended')
  })

  it('reads the ClickUp task behind a ticket and links to it', () => {
    expect(clickupTaskId('clickup:86a1b2c3d')).toBe('86a1b2c3d')
    expect(clickupTaskId('jira:1')).toBeNull()
    expect(clickupTaskId(null)).toBeNull()
    expect(clickupTaskUrl('86a1b2c3d')).toBe('https://app.clickup.com/t/86a1b2c3d')
  })

  it('names every non-active remote state, and nothing for active or unknown', () => {
    expect(remoteStateLabel('archived')).toBe('Archived in ClickUp')
    expect(remoteStateLabel('deleted')).toBe('Deleted in ClickUp')
    expect(remoteStateLabel('moved')).toBe('Moved to another ClickUp list')
    expect(remoteStateLabel('hidden')).toBe('No longer visible to the sync')
    expect(remoteStateLabel('active')).toBeNull()
    expect(remoteStateLabel('toString')).toBeNull()
    expect(remoteStateLabel(undefined)).toBeNull()
  })
})
