import { describe, expect, it } from 'vitest'
import {
  alertTarget,
  badgeCount,
  centerQuery,
  chunked,
  countLabel,
  IDS_PER_WRITE,
  filtersFrom,
  flattenPages,
  groupRows,
  inView,
  isForYou,
  kindOptions,
  lastDelivery,
  matchesFilters,
  NO_FILTERS,
  reasonOf,
  retryLabel,
  testLine,
  thisBrowserId,
  viewFrom,
  type CenterRow,
} from './notify-center'
import type { ClassInfo, Device } from './notify-wire'

let n = 0
const row = (over: Partial<CenterRow> = {}): CenterRow => ({
  id: `00000000-0000-4000-8000-${String(++n).padStart(12, '0')}`,
  kind: 'mention',
  title: 'A title',
  body: '',
  href: '/x',
  readAt: null,
  createdAt: '2026-11-01T00:00:00.000Z',
  ...over,
})

const CLASSES: ClassInfo[] = [
  {
    id: 'mention',
    label: 'Mentions & shares',
    blurb: '',
    category: 'direct',
    defaults: { desktop: true, push: true, email: true },
    breaksThrough: false,
    adminOnly: false,
    emailDelayMin: 0,
    kinds: [
      { id: 'mention', reason: 'Mentioned you' },
      { id: 'task-assigned', reason: 'Assigned to you' },
    ],
  },
  {
    id: 'work_complete',
    label: 'Work finished',
    blurb: '',
    category: 'following',
    defaults: { desktop: true, push: true, email: false },
    breaksThrough: false,
    adminOnly: false,
    emailDelayMin: 0,
    kinds: [{ id: 'task-status', reason: 'Status changed' }],
  },
]

describe('views and the badge', () => {
  it('For you is Needs you + Direct; a row without a category is For you', () => {
    expect(isForYou({ category: 'needs_you' })).toBe(true)
    expect(isForYou({ category: 'direct' })).toBe(true)
    expect(isForYou({ category: 'following' })).toBe(false)
    expect(isForYou({ category: 'workspace' })).toBe(false)
    expect(isForYou({})).toBe(true)
  })

  it('slices 2 needs-you, 3 direct and 4 following into 5 and 9', () => {
    const rows = [
      ...Array.from({ length: 2 }, () => row({ category: 'needs_you' })),
      ...Array.from({ length: 3 }, () => row({ category: 'direct' })),
      ...Array.from({ length: 4 }, () => row({ category: 'following' })),
    ]
    expect(rows.filter((r) => inView(r, 'foryou')).length).toBe(5)
    expect(rows.filter((r) => inView(r, 'all')).length).toBe(9)
    expect(rows.filter((r) => inView(r, 'done')).length).toBe(0)
  })

  it('a Done row leaves both views and is in Done', () => {
    const r = row({ category: 'direct', archivedAt: '2026-11-01T01:00:00Z', readAt: '2026-11-01T01:00:00Z' })
    expect(inView(r, 'foryou')).toBe(false)
    expect(inView(r, 'all')).toBe(false)
    expect(inView(r, 'done')).toBe(true)
  })

  it('badges unreadForYou, else every unread', () => {
    expect(badgeCount({ unread: 9, unreadForYou: 5 })).toBe(5)
    expect(badgeCount({ unread: 9 })).toBe(9)
    expect(badgeCount(undefined)).toBe(0)
  })

  it('reads the view from the URL', () => {
    expect(viewFrom('done')).toBe('done')
    expect(viewFrom('all')).toBe('all')
    expect(viewFrom(null)).toBe('foryou')
    expect(viewFrom('nonsense')).toBe('foryou')
  })
})

describe('labels', () => {
  it('reason: the server’s, else the registry’s for the kind', () => {
    expect(reasonOf(row({ reason: 'Served' }), CLASSES)).toBe('Served')
    expect(reasonOf(row({ kind: 'task-assigned' }), CLASSES)).toBe('Assigned to you')
    expect(reasonOf(row({ kind: 'unknown' }), CLASSES)).toBeNull()
  })

  it('count: by kind, only past one', () => {
    expect(countLabel({ kind: 'dm', count: 3 })).toBe('3 messages')
    expect(countLabel({ kind: 'task-status', count: 4 })).toBe('Moved 4 times')
    expect(countLabel({ kind: 'mention', count: 2 })).toBe('2 updates')
    expect(countLabel({ kind: 'dm', count: 1 })).toBeNull()
    expect(countLabel({ kind: 'dm' })).toBeNull()
  })
})

describe('groupRows', () => {
  const task = (id: string, title: string) => row({ entity: { type: 'task', id }, title })

  it('groups consecutive rows about one thing, with a shared ticket key', () => {
    const rows = [task('t1', 'TAL-112 moved to Review'), task('t1', 'Bob commented on TAL-112'), task('t1', 'Assigned: TAL-112'), row({ title: 'A DM' })]
    const g = groupRows(rows)
    expect(g.length).toBe(2)
    expect(g[0]?.rows.length).toBe(3)
    expect(g[0]?.label).toBe('TAL-112 · 3 updates')
    expect(g[1]?.label).toBeNull()
  })

  it('names a group by its entity type when no key is shared', () => {
    const g = groupRows([task('t1', 'one'), task('t1', 'two')])
    expect(g[0]?.label).toBe('Ticket · 2 updates')
  })

  it('never pulls a row past a newer row about something else', () => {
    const g = groupRows([task('t1', 'a'), row({ title: 'between' }), task('t1', 'b')])
    expect(g.map((x) => x.rows.length)).toEqual([1, 1, 1])
  })

  it('does not group rows with no entity id', () => {
    const g = groupRows([row({ entity: { type: 'task', id: null } }), row({ entity: { type: 'task', id: null } })])
    expect(g.length).toBe(2)
  })
})

describe('filters', () => {
  it('reads the URL, dropping what it does not know', () => {
    expect(filtersFrom((k) => ({ unread: '1', category: 'direct,bogus,direct', kind: 'dm' })[k])).toEqual({ unread: true, categories: ['direct'], kind: 'dm' })
    expect(filtersFrom(() => null)).toEqual(NO_FILTERS)
    expect(filtersFrom((k) => (k === 'kind' ? 'DROP TABLE' : null)).kind).toBeNull()
  })

  it('builds the inbox API query', () => {
    expect(centerQuery('foryou', NO_FILTERS)).toBe('view=foryou&limit=50')
    expect(centerQuery('all', { unread: true, categories: ['direct', 'following'], kind: 'dm' }, 'CUR', 20)).toBe(
      'view=all&unread=1&category=direct%2Cfollowing&kind=dm&limit=20&before=CUR',
    )
  })

  it('matches client-side the way the server filters', () => {
    const f = { unread: true, categories: ['direct' as const], kind: null }
    expect(matchesFilters(row({ category: 'direct' }), f)).toBe(true)
    expect(matchesFilters(row({ category: 'direct', readAt: '2026-11-01T00:00:00Z' }), f)).toBe(false)
    expect(matchesFilters(row({ category: 'following' }), f)).toBe(false)
    expect(matchesFilters(row({ kind: 'dm' }), { ...NO_FILTERS, kind: 'mention' })).toBe(false)
  })

  it('offers the kinds of the chosen categories, under their reasons', () => {
    expect(kindOptions(CLASSES).map((k) => k.id)).toEqual(['mention', 'task-assigned', 'task-status'])
    expect(kindOptions(CLASSES, ['following'])).toEqual([{ id: 'task-status', label: 'Status changed' }])
  })

  it('flattens pages, each row once', () => {
    const a = row()
    const b = row()
    expect(flattenPages([{ notifications: [a, b] }, { notifications: [b, row()] }]).length).toBe(3)
  })
})

describe('devices and the test', () => {
  const dev = (over: Partial<Device>): Device => ({
    id: 'd',
    label: 'Chrome on Linux',
    createdAt: '2026-11-01T00:00:00Z',
    lastSeenAt: '2026-11-01T00:00:00Z',
    lastAttemptAt: null,
    lastStatus: null,
    lastError: null,
    failures: 0,
    thisSession: false,
    ...over,
  })

  it('marks the newest device of this session as this browser', () => {
    expect(thisBrowserId([dev({ id: 'a' }), dev({ id: 'b', thisSession: true }), dev({ id: 'c', thisSession: true })])).toBe('b')
    expect(thisBrowserId([dev({ id: 'a' })])).toBeNull()
  })

  it('says the last delivery', () => {
    expect(lastDelivery(dev({})).ok).toBeNull()
    expect(lastDelivery(dev({ lastAttemptAt: '2026-11-01T00:00:00Z', lastStatus: 201 })).ok).toBe(true)
    expect(lastDelivery(dev({ lastAttemptAt: '2026-11-01T00:00:00Z', lastStatus: 410 })).text).toBe('Last push failed (HTTP 410)')
  })

  it('words each test outcome', () => {
    expect(testLine({ kind: 'sent', device: { id: 'd', status: 201, ms: 230, error: null } }).text).toBe(
      'Delivered to the push service in 230 ms. A banner should appear on that device.',
    )
    expect(testLine({ kind: 'sent', device: { id: 'd', status: 410, ms: 80, error: 'gone' } }).tone).toBe('bad')
    expect(testLine({ kind: 'unavailable' }).text).toBe('Server test not available on this instance yet.')
    expect(testLine({ kind: 'limited', retryAfter: 540 }).text).toBe('Too many tests. Try again in 9 min.')
    expect(retryLabel(30)).toBe('30 s')
    expect(retryLabel(61)).toBe('2 min')
  })
})

describe('alertTarget', () => {
  const snap = (looking: string | null, active: string | null, clients: Array<[string, 'web' | 'desktop']>) => ({
    looking,
    active,
    clients: clients.map(([clientId, kind]) => ({ clientId, kind, state: 'looking' as const, at: '2026-11-01T00:00:00Z' })),
  })

  it('names the winner: looking first, then active', () => {
    expect(alertTarget(snap('tab-aaaaaaaa', null, [['tab-aaaaaaaa', 'web']]), 'tab-aaaaaaaa').target).toBe('this-browser')
    expect(alertTarget(snap(null, 'desk-1234567', [['desk-1234567', 'desktop']]), 'tab-aaaaaaaa').target).toBe('desktop')
    expect(alertTarget(snap('tab-bbbbbbbb', null, [['tab-bbbbbbbb', 'web']]), 'tab-aaaaaaaa').target).toBe('other-browser')
    // Without the runtime's own id, a web winner is not claimed as this tab.
    expect(alertTarget(snap('tab-aaaaaaaa', null, [['tab-aaaaaaaa', 'web']]), null).target).toBe('other-browser')
    expect(alertTarget(snap(null, null, []), null).target).toBe('none')
  })
})

describe('bulk writes', () => {
  it('slice ids to what one PUT takes, in order, and never send an empty slice', () => {
    expect(IDS_PER_WRITE).toBe(200)
    const ids = Array.from({ length: 450 }, (_, i) => `id-${i}`)
    const parts = chunked(ids)
    expect(parts.map((p) => p.length)).toEqual([200, 200, 50])
    expect(parts.flat()).toEqual(ids)
    expect(chunked(ids.slice(0, 200))).toHaveLength(1)
    // An empty ids list means "every row" to the api: it is never a slice.
    expect(chunked([])).toEqual([])
  })
})
