import { describe, expect, it } from 'vitest'
import type { Artifact } from '@/lib/artifacts'
import type { Conversation } from '@/lib/conversations.svelte'
import type { BriefEntry, BriefLine, BriefSection, BriefView } from '@/lib/daily-brief-types'
import type { ResearchRun } from '@/lib/research'
import type { HomeSummary, WorkItem } from '@/routes/app/home/home'
import { STATUS_LABEL } from '@/lib/task-const'
import {
  ago,
  alsoToday,
  blockerOf,
  buildInboxCards,
  cardActions,
  cardMeta,
  glyphOf,
  isMachineText,
  laneCards,
  laneTally,
  ledeStops,
  needsBreakdown,
  openLabel,
  ownersOf,
  refOf,
  sender,
  sentences,
  shownCards,
  splitRef,
  statusText,
  ticketStatus,
  todayMessages,
  type InboxCard,
  type InboxInput,
} from './inbox-board'
import { initials } from './initials'

const NOW = new Date(2026, 10, 13, 15, 0, 0).getTime()
const hoursAgo = (h: number): string => new Date(NOW - h * 3_600_000).toISOString()
const YESTERDAY = hoursAgo(30)

function entry(over: Partial<BriefEntry> = {}): BriefEntry {
  return {
    id: 'e',
    seq: 1,
    batch: null,
    kind: 'item',
    section: 'action',
    sourceKey: null,
    sourceType: 'task',
    sourceId: null,
    sourceHref: '/boards/b1/t1',
    fingerprint: null,
    supersedes: null,
    priority: 'p1',
    statusLabel: 'REVIEW',
    badge: null,
    title: 'Approve the completed work?',
    body: 'Review the evidence.',
    evidence: [],
    createdAt: hoursAgo(2),
    ...over,
  }
}

function line(key: string, section: BriefSection, over: Partial<BriefEntry> = {}, resolved = false): BriefLine {
  const current = entry({ section, sourceKey: key, ...over })
  return { key, section, current, history: [current], resolved, unseen: false }
}

function brief(all: BriefLine[], comms: BriefView['comms'] = []): BriefView {
  const sections = (['action', 'schedule', 'comms', 'highlights'] as const)
    .map((section) => ({ section, lines: all.filter((l) => l.section === section) }))
    .filter((s) => s.lines.length > 0)
  return {
    id: 'brief-1',
    date: '2026-11-13',
    zone: 'UTC',
    openedAt: hoursAgo(8),
    lede: 'A quiet day.',
    agent: { configured: true, model: 'glyph', name: 'Glyph' },
    artifactId: 'brief-doc',
    sections,
    updates: [],
    comms,
    lastSeq: 1,
    readSeq: 1,
    unseenCount: 0,
    lastSweptAt: null,
  }
}

const ticket = (id: string, updatedAt = hoursAgo(5)): WorkItem => ({
  id,
  boardId: 'b1',
  board: 'Talaria',
  ticketRef: `TALA-${id}`,
  title: `Ticket ${id}`,
  status: 'inbox',
  updatedAt,
})

const queues = (triage: WorkItem[] = [], review: WorkItem[] = [], blocked: WorkItem[] = []): HomeSummary['queues'] => ({
  triage: { count: triage.length, items: triage },
  review: { count: review.length, items: review },
  blocked: { count: blocked.length, items: blocked },
})

const session = (id: string, over: Partial<Conversation> = {}): Conversation => ({
  id,
  agentModel: 'opus',
  title: `Session ${id}`,
  updatedAt: hoursAgo(1),
  ...over,
})

const run = (id: string, over: Partial<ResearchRun> = {}): ResearchRun => ({
  id,
  ownerUserId: 'u1',
  requestedBy: 'u1',
  agentModel: 'opus',
  mode: 'recon',
  question: `Question ${id}`,
  title: null,
  status: 'running',
  phase: null,
  awaiting: null,
  artifactId: null,
  conversationId: null,
  error: null,
  stats: {},
  createdAt: hoursAgo(1),
  completedAt: null,
  ...over,
})

const doc = (id: string, updatedAt: string, createdAt = YESTERDAY): Artifact =>
  ({ id, kind: 'doc', title: `Doc ${id}`, updatedAt, createdAt }) as Artifact

function input(over: Partial<InboxInput> = {}): InboxInput {
  return {
    now: NOW,
    brief: null,
    queues: null,
    sessions: null,
    runs: null,
    artifacts: null,
    agentName: (id) => (id === 'opus' ? 'Opus 5.5' : null),
    assignees: () => null,
    ...over,
  }
}

const FORGE = { name: 'Forge', agent: true }
const OPUS = { name: 'Opus 5.5', agent: true }

const laneKeys = (cards: InboxCard[]) => {
  const lanes = laneCards(cards)
  return { needs: lanes.needs.map((c) => c.key), running: lanes.running.map((c) => c.key), done: lanes.done.map((c) => c.key) }
}

describe('buildInboxCards', () => {
  it('puts what waits on the owner or failed in Needs you, live work in Running, and today’s finished work in Done', () => {
    const cards = buildInboxCards(
      input({
        brief: brief([
          line('task:t1', 'action', { sourceId: 't1' }),
          line('task:t2', 'action', { sourceId: 't2', statusLabel: 'FAILED' }),
          line('task:t3', 'action', { sourceId: 't3', kind: 'resolved', statusLabel: 'DONE' }, true),
          line('channel:c1', 'comms', { sourceType: 'channel', title: 'Replied to Jon', kind: 'resolved', statusLabel: 'YOU REPLIED' }, true),
        ]),
        queues: queues([ticket('9')]),
        sessions: [
          session('s-live', { working: true }),
          session('s-broken', { failed: true }),
          session('s-today'),
          session('s-old', { updatedAt: YESTERDAY }),
        ],
        runs: [
          run('r-live'),
          run('r-asks', { status: 'awaiting', awaiting: { key: 'k', question: 'Which market?', options: [] } }),
          run('r-error', { status: 'error', error: 'Search failed' }),
          run('r-today', { status: 'done', completedAt: hoursAgo(3) }),
          run('r-old', { status: 'done', completedAt: YESTERDAY }),
        ],
        artifacts: [doc('d-today', hoursAgo(4)), doc('d-old', YESTERDAY), doc('brief-doc', hoursAgo(1))],
      }),
    )
    const lanes = laneKeys(cards)
    expect(new Set(lanes.needs)).toEqual(
      new Set(['brief:task:t1', 'brief:task:t2', 'ticket:9', 'session:s-broken', 'run:r-asks', 'run:r-error']),
    )
    expect(new Set(lanes.running)).toEqual(new Set(['session:s-live', 'run:r-live']))
    expect(new Set(lanes.done)).toEqual(
      new Set(['brief:task:t3', 'brief:channel:c1', 'session:s-today', 'run:r-today', 'doc:d-today']),
    )
    expect(cards.find((c) => c.key === 'brief:task:t2')?.state).toBe('failed')
  })

  it('shows a ticket the brief already names once, as the brief’s card, with the ticket’s title, ref and board', () => {
    const cards = buildInboxCards(
      input({
        brief: brief([line('task:t7', 'action', { sourceId: '7' })]),
        queues: queues([ticket('7')], [ticket('7')], [ticket('8')]),
      }),
    )
    expect(cards.map((c) => c.key).sort()).toEqual(['brief:task:t7', 'ticket:8'])
    const named = cards.find((c) => c.key === 'brief:task:t7')!
    expect(named.title).toBe('Ticket 7')
    expect(named.ref).toBe('TALA-7')
    expect(named.where).toBe('Talaria')
    expect(named.taskId).toBe('7')
    expect(named.line?.key).toBe('task:t7')
  })

  it('shows a research run, its thread and its report once, as the run', () => {
    const cards = buildInboxCards(
      input({
        sessions: [session('thread-1')],
        runs: [run('r1', { conversationId: 'thread-1', artifactId: 'report-1', status: 'done', completedAt: hoursAgo(1) })],
        artifacts: [doc('report-1', hoursAgo(1))],
      }),
    )
    expect(cards.map((c) => c.key)).toEqual(['run:r1'])
  })

  it('orders every lane newest first', () => {
    const cards = buildInboxCards(
      input({ queues: queues([ticket('old', hoursAgo(9)), ticket('new', hoursAgo(1)), ticket('mid', hoursAgo(4))]) }),
    )
    expect(laneCards(cards).needs.map((c) => c.key)).toEqual(['ticket:new', 'ticket:mid', 'ticket:old'])
  })
})

describe('shownCards', () => {
  it('shows a lane its newest five, the same on the board and in the list, until asked for all', () => {
    const sessions = Array.from({ length: 12 }, (_, i) => session(`s${i}`, { updatedAt: hoursAgo(i + 1) }))
    const lanes = laneCards(buildInboxCards(input({ sessions })))
    expect(shownCards(lanes.done, false).map((c) => c.key)).toEqual(['s0', 's1', 's2', 's3', 's4'].map((id) => `session:${id}`))
    expect(shownCards(lanes.done, true)).toHaveLength(12)
  })
})

describe('what a card says', () => {
  it('names the ticket, not the brief’s question about it, open or resolved', () => {
    const cards = buildInboxCards(
      input({
        brief: brief([
          line('task:a', 'action', { sourceId: 'a', title: 'Approve the completed work for “Desktop updater trusts the new key”?' }, true),
          line('task:b', 'action', { sourceId: 'b', title: 'Where should "[Doug] Rotate the GHCR token" go next?' }),
          line('notification:n', 'action', { sourceType: 'notification', title: 'Scout finished "Agent inbox patterns"' }),
        ]),
      }),
    )
    expect(Object.fromEntries(cards.map((c) => [c.key, [c.title, c.from]]))).toEqual({
      'brief:task:a': ['Desktop updater trusts the new key', null],
      'brief:task:b': ['Rotate the GHCR token', 'Doug'],
      'brief:notification:n': ['Scout finished "Agent inbox patterns"', null],
    })
  })

  it('moves a "[Name] " title prefix to who raised it, on any source', () => {
    const [card] = buildInboxCards(input({ queues: queues([{ ...ticket('1'), title: "[Doug] Workbench can't start jobs" }]) }))
    expect([card!.title, card!.from]).toEqual(["Workbench can't start jobs", 'Doug'])
  })

  it('moves a ticket ref a title opens with to the card’s ref, and leaves any other title alone', () => {
    expect(
      [
        'TALA-93: Inbox board data adapter and lanes',
        'HELP-7: fix double slash in workbench branch names',
        'HELP-2 - rotate GHCR pull token',
        'TALA-9: iOS build fails',
        'Bug: KB sidebar tree forgets collapse state',
        'Feature: Browse view in Talaria Desktop',
        'Same root cause as HELP-3: the harness config',
        'HELP-7',
      ].map(splitRef),
    ).toEqual([
      { title: 'Inbox board data adapter and lanes', ref: 'TALA-93' },
      { title: 'Fix double slash in workbench branch names', ref: 'HELP-7' },
      { title: 'Rotate GHCR pull token', ref: 'HELP-2' },
      { title: 'iOS build fails', ref: 'TALA-9' },
      { title: 'Bug: KB sidebar tree forgets collapse state', ref: null },
      { title: 'Feature: Browse view in Talaria Desktop', ref: null },
      { title: 'Same root cause as HELP-3: the harness config', ref: null },
      { title: 'HELP-7', ref: null },
    ])
  })

  it('gives a session named for its ticket that ticket’s ref, and keeps a ticket’s own ref over one its title names', () => {
    const cards = buildInboxCards(
      input({
        sessions: [session('s', { working: true, title: 'TALA-93: Inbox board data adapter' })],
        queues: queues([{ ...ticket('1'), title: 'TALA-1: Same ticket' }, { ...ticket('2'), title: 'HELP-3: Follow-up to another ticket' }]),
      }),
    )
    expect(Object.fromEntries(cards.map((c) => [c.key, [c.title, c.ref]]))).toEqual({
      'session:s': ['Inbox board data adapter', 'TALA-93'],
      'ticket:1': ['Same ticket', 'TALA-1'],
      'ticket:2': ['HELP-3: Follow-up to another ticket', 'TALA-2'],
    })
  })

  it('drops the brief’s stock advice for the line’s own evidence, or for nothing', () => {
    const stock = 'Review the evidence, then approve it or request changes.'
    const cards = buildInboxCards(
      input({
        brief: brief([
          line('task:a', 'action', { sourceId: 'a', body: stock, evidence: [{ label: 'Agent', text: 'Forge finished; e2e green.' }] }),
          line('task:b', 'action', { sourceId: 'b', body: stock }),
          line('task:c', 'action', { sourceId: 'c', body: 'Same root cause as HELP-3.' }),
        ]),
      }),
    )
    expect(Object.fromEntries(cards.map((c) => [c.key, c.summary]))).toEqual({
      'brief:task:a': 'Forge finished; e2e green.',
      'brief:task:b': null,
      'brief:task:c': 'Same root cause as HELP-3.',
    })
  })

  it('gives each kind of waiting its own glyph and word, from the brief or the ticket queue', () => {
    const cards = buildInboxCards(
      input({
        brief: brief([
          line('task:a', 'action', { sourceId: 'a', statusLabel: 'BLOCKED' }),
          line('task:b', 'action', { sourceId: 'b', statusLabel: 'REVIEW' }),
          line('task:c', 'action', { sourceId: 'c', statusLabel: 'TRIAGE' }),
          line('task:d', 'action', { sourceId: 'd', statusLabel: 'FAILED' }),
          line('approval:e', 'action', { sourceType: 'approval', statusLabel: 'APPROVAL · SEND EMAIL' }),
          line('notification:f', 'action', { sourceType: 'notification', statusLabel: 'MENTION' }),
        ]),
        queues: queues([ticket('t')], [ticket('r')], [ticket('k')]),
      }),
    )
    expect(Object.fromEntries(cards.map((c) => [c.key, [glyphOf(c), c.label]]))).toEqual({
      'brief:task:a': ['blocked', 'Blocked'],
      'brief:task:b': ['signoff', 'Awaiting sign-off'],
      'brief:task:c': ['triage', 'Triage'],
      'brief:task:d': ['failed', 'Failed'],
      'brief:approval:e': ['signoff', 'Awaiting sign-off'],
      'brief:notification:f': ['waiting', 'Mention'],
      'ticket:t': ['triage', 'Triage'],
      'ticket:r': ['signoff', 'Awaiting sign-off'],
      'ticket:k': ['blocked', 'Blocked'],
    })
  })
})

describe('the one age format', () => {
  it('reads anything that happened as how long ago, never a bare span', () => {
    const minutes = (m: number): number => m * 60_000
    expect([0, 0.9, 1, 48, 59, 60, 179, 23 * 60, 24 * 60, 9 * 24 * 60].map((m) => ago(minutes(m)))).toEqual([
      'just now',
      'just now',
      '1m ago',
      '48m ago',
      '59m ago',
      '1h ago',
      '2h ago',
      '23h ago',
      '1d ago',
      '9d ago',
    ])
    expect(ago(-5000)).toBe('just now')
  })

  it('gives a waiting, failed or finished card an age and only running work a duration, and leads with the word only where the lane does not say it', () => {
    const [review] = buildInboxCards(input({ brief: brief([line('task:t1', 'action', { sourceId: 't1' })]) }))
    expect(cardMeta(review!, NOW)).toMatchObject({ word: 'Awaiting sign-off', statusPrefix: 'Awaiting sign-off', time: '2h ago', timeIsDuration: false })

    const [failed] = buildInboxCards(input({ sessions: [session('s0', { failed: true, updatedAt: hoursAgo(1) })] }))
    expect(cardMeta(failed!, NOW)).toMatchObject({ word: 'Failed', statusPrefix: 'Failed', time: '1h ago', timeIsDuration: false })

    const live = new Date(NOW - (4 * 60 + 11) * 1000).toISOString()
    const [running] = buildInboxCards(input({ runs: [run('r1', { createdAt: live })] }))
    expect(cardMeta(running!, NOW)).toMatchObject({ word: 'Running', statusPrefix: null, time: '4m 11s', timeIsDuration: true })
    const [queued] = buildInboxCards(input({ runs: [run('r2', { createdAt: live, status: 'queued' })] }))
    expect(cardMeta(queued!, NOW)).toMatchObject({ word: 'Queued', statusPrefix: 'Queued', timeIsDuration: true })

    const [done] = buildInboxCards(input({ sessions: [session('s1', { updatedAt: hoursAgo(3) })] }))
    expect(cardMeta(done!, NOW)).toMatchObject({ word: 'Done', statusPrefix: null, time: '3h ago', timeIsDuration: false })
  })

  it('keeps the owner slot on every card: nobody on it is an empty owner, open or finished', () => {
    const cards = buildInboxCards(
      input({
        queues: queues([ticket('open')]),
        brief: brief([line('task:t3', 'action', { sourceId: 't3', kind: 'resolved', statusLabel: 'RESOLVED' }, true)]),
        assignees: () => [],
      }),
    )
    expect(Object.fromEntries(cards.map((c) => [c.key, cardMeta(c, NOW).owners]))).toEqual({ 'ticket:open': [], 'brief:task:t3': [] })
  })

  it('says the drawer’s status as the word alone, with the duration only while it runs', () => {
    expect(statusText({ word: 'Blocked', time: '48m ago', timeIsDuration: false })).toBe('Blocked')
    expect(statusText({ word: 'Running', time: '1m 46s', timeIsDuration: true })).toBe('Running · 1m 46s')
  })
})

describe('cardMeta', () => {
  it('leads with the word, then the time, the ref and the owner, never who raised it', () => {
    const [blocked] = buildInboxCards(
      input({
        queues: queues([], [], [{ ...ticket('7', hoursAgo(2)), title: '[Doug] Jobs fail', ticketRef: 'HELP-7' }]),
        assignees: () => [FORGE],
      }),
    )
    expect(cardMeta(blocked!, NOW)).toEqual({ word: 'Blocked', statusPrefix: 'Blocked', time: '2h ago', timeIsDuration: false, ref: { kind: null, ref: 'HELP-7' }, owners: [FORGE] })
    expect(blocked!.from).toBe('Doug')
  })

  it('names who wrote a document, never the place it lives', () => {
    const cards = buildInboxCards(
      input({
        artifacts: [{ ...doc('by-agent', hoursAgo(1)), createdBy: 'opus' }, { ...doc('by-person', hoursAgo(2)), createdBy: 'u-1' }],
      }),
    )
    const owners = (key: string) => cardMeta(cards.find((c) => c.key === key)!, NOW).owners
    expect(owners('doc:by-agent')).toEqual([OPUS])
    expect(owners('doc:by-person')).toEqual([])
  })

  it('reads every finished card one way: Done, how long ago, what kind of record and who did it, with no verb for how', () => {
    const cards = buildInboxCards(
      input({
        brief: brief([line('task:t3', 'action', { sourceId: 't3', kind: 'resolved', statusLabel: 'RESOLVED' }, true)]),
        sessions: [session('s-today', { updatedAt: hoursAgo(3) })],
        artifacts: [{ ...doc('new', hoursAgo(1), hoursAgo(1)), createdBy: 'opus' }],
      }),
    )
    const metas = Object.fromEntries(cards.map((c) => [c.key, cardMeta(c, NOW)]))
    expect(metas).toEqual({
      'doc:new': { word: 'Done', statusPrefix: null, time: '1h ago', timeIsDuration: false, ref: { kind: 'Doc', ref: null }, owners: [OPUS] },
      'brief:task:t3': { word: 'Done', statusPrefix: null, time: '2h ago', timeIsDuration: false, ref: { kind: null, ref: null }, owners: [] },
      'session:s-today': { word: 'Done', statusPrefix: null, time: '3h ago', timeIsDuration: false, ref: { kind: 'Run', ref: null }, owners: [OPUS] },
    })
  })
})

describe('refOf', () => {
  it('says what type of record a card is apart from its ref, and gives a ticket no type', () => {
    const cards = buildInboxCards(
      input({
        queues: queues([], [], [{ ...ticket('7'), ticketRef: 'HELP-7' }]),
        brief: brief([line('task:9', 'action', { sourceId: '9' }), line('message:m', 'comms', {}, true)]),
        sessions: [session('on-ticket', { title: 'HELP-7: fix double slash', working: true }), session('free', { title: 'Morning sweep', working: true })],
        runs: [run('r1')],
        artifacts: [doc('d1', hoursAgo(1))],
      }),
    )
    expect(Object.fromEntries(cards.map((c) => [c.key, refOf(c)]))).toEqual({
      'ticket:7': { kind: null, ref: 'HELP-7' },
      'brief:task:9': { kind: null, ref: null },
      'brief:message:m': { kind: null, ref: null },
      'session:on-ticket': { kind: 'Run', ref: 'HELP-7' },
      'session:free': { kind: 'Run', ref: null },
      'run:r1': { kind: 'Research', ref: null },
      'doc:d1': { kind: 'Doc', ref: null },
    })
  })
})

describe('ownersOf', () => {
  it('names a ticket’s assignees on the card, in the list and in the drawer alike, never who raised it', () => {
    const on: Record<string, Array<{ name: string; agent: boolean }>> = { a: [FORGE], b: [], c: [FORGE, { name: 'Ana', agent: false }] }
    const cards = buildInboxCards(
      input({
        brief: brief([line('task:a', 'action', { sourceId: 'a', title: '[Doug] Jobs fail' })]),
        queues: queues([ticket('b'), ticket('c'), ticket('d')]),
        sessions: [session('s', { working: true })],
        assignees: (id) => on[id] ?? null,
      }),
    )
    const owners = Object.fromEntries(cards.map((c) => [c.key, ownersOf(c)]))
    expect(owners).toEqual({
      'brief:task:a': [FORGE],
      'ticket:b': [],
      'ticket:c': [FORGE, { name: 'Ana', agent: false }],
      'ticket:d': null,
      'session:s': [OPUS],
    })
    for (const c of cards) expect(cardMeta(c, NOW).owners).toEqual(ownersOf(c) ?? [])
  })
})

describe('cardActions', () => {
  const card = (over: Partial<BriefEntry> = {}, resolved = false) =>
    buildInboxCards(input({ brief: brief([line('task:a', 'action', { sourceId: 'a', ...over }, resolved)]) }))[0]!
  const all = { ask: true, open: true }
  const ids = (...args: Parameters<typeof cardActions>) => cardActions(...args).bar.map((a) => a.id)
  const menu = (...args: Parameters<typeof cardActions>) => cardActions(...args).menu
  const filled = (...args: Parameters<typeof cardActions>) =>
    cardActions(...args)
      .bar.filter((a) => a.primary)
      .map((a) => a.id)
  const BLOCKED = { statusLabel: 'Blocked 3h', badge: { label: 'Blocked', tone: 'danger' as const } }

  it('draws the decision first, then Ask, then Open, in one order everywhere', () => {
    expect(ids(card(), all)).toEqual(['check', 'ask', 'open'])
    expect(ids(card({ sourceType: 'approval', sourceId: 'p1', statusLabel: 'APPROVAL · SEND EMAIL' }), all)).toEqual([
      'approve',
      'reject',
      'ask',
      'open',
    ])
    expect(ids(card({}, true), all)).toEqual(['restore', 'ask', 'open'])
  })

  it('leads a blocked or failed card with the reply to its owner, Mark handled after it', () => {
    expect(ids(card(BLOCKED), all)).toEqual(['ask', 'check', 'open'])
    expect(ids(card({ statusLabel: 'FAILED' }), all)).toEqual(['ask', 'check', 'open'])
    expect(ids(card(BLOCKED), { ask: false, open: true })).toEqual(['check', 'open'])
  })

  it('keeps Remove from inbox out of the bar, in the menu of every card that can be marked handled', () => {
    expect(menu(card(), all)).toEqual(['dismiss'])
    expect(menu(card(BLOCKED), all)).toEqual(['dismiss'])
    expect(menu(card({ sourceType: 'approval', sourceId: 'p1', statusLabel: 'APPROVAL · SEND EMAIL' }), all)).toEqual([])
    expect(menu(card({}, true), all)).toEqual([])
    const [running] = buildInboxCards(input({ sessions: [session('s', { working: true })] }))
    expect(menu(running!, all)).toEqual([])
    for (const c of [card(), card(BLOCKED)]) expect(ids(c, all)).not.toContain('dismiss')
  })

  it('fills exactly one action, the open decision, and none on finished or undecidable cards', () => {
    expect(filled(card(), all)).toEqual(['check'])
    expect(filled(card({ sourceType: 'approval', sourceId: 'p1', statusLabel: 'APPROVAL · SEND EMAIL' }), all)).toEqual(['approve'])
    expect(filled(card(BLOCKED), all)).toEqual(['ask'])
    expect(filled(card(BLOCKED), { ask: false, open: true })).toEqual(['check'])
    expect(filled(card({}, true), all)).toEqual([])
    const [running] = buildInboxCards(input({ sessions: [session('s', { working: true })] }))
    expect(filled(running!, all)).toEqual([])
  })

  it('drops only what a surface cannot do, never reorders', () => {
    expect(ids(card(), { ask: false, open: true })).toEqual(['check', 'open'])
    expect(ids(card(), { ask: true, open: false })).toEqual(['check', 'ask'])
    const [running] = buildInboxCards(input({ sessions: [session('s', { working: true })] }))
    expect(ids(running!, all)).toEqual(['open'])
  })
})

describe('blockerOf', () => {
  it('asks why a stuck card is stuck over the source’s own line about it, and is nothing on any other card', () => {
    const cards = buildInboxCards(
      input({
        brief: brief([
          line('task:x', 'action', { sourceId: 'x', statusLabel: 'Blocked 3h', badge: { label: 'Blocked', tone: 'danger' }, body: 'Needs a decision on the prefix.' }),
          line('task:y', 'action', { sourceId: 'y' }),
        ]),
        queues: queues([], [], [ticket('k')]),
        sessions: [session('s', { failed: true })],
      }),
    )
    const blockers = Object.fromEntries(cards.map((c) => [c.key, blockerOf(c)]))
    expect(blockers).toEqual({
      'brief:task:x': { glyph: 'blocked', label: "Why it's blocked", text: 'Needs a decision on the prefix.' },
      'brief:task:y': null,
      'ticket:k': { glyph: 'blocked', label: "Why it's blocked", text: null },
      'session:s': { glyph: 'failed', label: 'Why it failed', text: 'The last turn failed. Open it to run it again.' },
    })
  })
})

describe('ticketStatus', () => {
  it('reads every ticket status in the board’s words, never the workflow’s', () => {
    for (const status of Object.keys(STATUS_LABEL)) expect(ticketStatus(status).word).not.toBe('Inbox')
    expect(ticketStatus('inbox')).toEqual({ glyph: 'triage', word: 'Triage' })
    expect(ticketStatus('in_progress')).toEqual({ glyph: 'running', word: 'Running' })
    expect(ticketStatus('quality_review')).toEqual({ glyph: 'signoff', word: 'Awaiting sign-off' })
    expect(ticketStatus('done')).toEqual({ glyph: 'done', word: 'Done' })
  })
})

describe('todayMessages', () => {
  it('lists the people still waiting, not the threads already answered', () => {
    const messages = todayMessages(
      brief(
        [
          line('channel:c1', 'comms', { sourceType: 'channel', title: 'Jon is waiting on you', body: 'Can you look at the inbox?' }),
          line('channel:c2', 'comms', { sourceType: 'channel', title: 'Replied to Ana', kind: 'resolved' }, true),
        ],
        [{ sourceKey: 'channel:c1', channelId: 'c1', delegated: false, draft: null }],
      ),
    )
    expect(messages.map((m) => [m.from, m.text, m.comms?.channelId])).toEqual([['Jon', 'Can you look at the inbox?', 'c1']])
  })

  it('puts whoever wrote last first', () => {
    const waiting = (key: string, title: string, hours: number) => line(key, 'comms', { sourceType: 'channel', title, createdAt: hoursAgo(hours) })
    const messages = todayMessages(brief([waiting('channel:a', 'Jon is waiting on you', 2), waiting('channel:b', 'Maya asked in #dev', 0.5), waiting('channel:c', 'Priya needs a sign-off', 4)]))
    expect(messages.map((m) => m.from)).toEqual(['Maya', 'Jon', 'Priya'])
  })

  it('names the person alone, and the channel only when the brief says one', () => {
    expect(['Jon Avery is waiting on you', 'Doug asked in #helpdesk', 'Priya Raman needs a sign-off', 'Ana'].map(sender)).toEqual([
      { name: 'Jon Avery', channel: null },
      { name: 'Doug', channel: '#helpdesk' },
      { name: 'Priya Raman', channel: null },
      { name: 'Ana', channel: null },
    ])
  })

  it('gives every message one source token: its channel, DM for a conversation with none, else its kind', () => {
    const waiting = (key: string, title: string, sourceType: string | null) => line(key, 'comms', { sourceType, title })
    const messages = todayMessages(
      brief([
        waiting('channel:a', 'Doug asked in #helpdesk', 'channel'),
        waiting('dm:b', 'Jon Avery is waiting on you', 'channel'),
        waiting('dm:c', 'Priya Raman needs a sign-off', 'channel'),
        waiting('mail:d', 'Ana is waiting on you', 'mail'),
        waiting('x:e', 'Sam', null),
      ]),
    )
    expect(Object.fromEntries(messages.map((m) => [m.from, m.source]))).toEqual({ Doug: '#helpdesk', 'Jon Avery': 'DM', 'Priya Raman': 'DM', Ana: 'Mail', Sam: 'Message' })
  })

  it('gives a person the same initials in a row as in the drawer, from their name', () => {
    const [message] = todayMessages(brief([line('channel:c2', 'comms', { sourceType: 'channel', title: 'Doug asked in #helpdesk' })]))
    expect(initials(message!.from)).toBe('Do')
    expect(initials('Priya Raman')).toBe('PR')
    expect(initials('  ')).toBe('?')
  })
})

describe('needsBreakdown', () => {
  it('counts each kind of need in its card’s own word, most urgent first, and omits a kind with none', () => {
    const cards = buildInboxCards(
      input({
        queues: queues([ticket('1'), ticket('2')], [ticket('3')]),
        sessions: [session('s1', { working: true })],
        brief: brief([line('task:x', 'action', { sourceId: 'x', statusLabel: 'Blocked 3h', badge: { label: 'Blocked', tone: 'danger' } })]),
      }),
    )
    expect(needsBreakdown(cards)).toEqual([
      { glyph: 'blocked', word: 'Blocked', count: 1 },
      { glyph: 'signoff', word: 'Awaiting sign-off', count: 1 },
      { glyph: 'triage', word: 'Triage', count: 2 },
    ])
    for (const kind of needsBreakdown(cards)) expect(cards.some((c) => cardMeta(c, NOW).word === kind.word)).toBe(true)
    expect(needsBreakdown([])).toEqual([])
  })
})

describe('laneTally', () => {
  it('counts what is running and what finished today, and leaves out a lane with nothing', () => {
    const lanes = (running: number, done: number) => ({ needs: [], running: Array(running).fill(0), done: Array(done).fill(0) })
    expect(laneTally(lanes(4, 8))).toBe('4 running · 8 done today')
    expect(laneTally(lanes(0, 8))).toBe('8 done today')
    expect(laneTally(lanes(4, 0))).toBe('4 running')
    expect(laneTally(lanes(0, 0))).toBeNull()
    expect(laneTally(lanes(0, 0), { withZeros: true })).toBe('0 running · 0 done today')
    expect(laneTally(lanes(4, 0), { withZeros: true })).toBe('4 running · 0 done today')
  })
})

describe('isMachineText', () => {
  it('is a failure, error or log line, not prose evidence', () => {
    expect(['Last failure', 'Error', 'Build log', 'Command'].map(isMachineText)).toEqual([true, true, true, true])
    expect(['Agent', 'Judge', 'Catalog', 'Failing over'].map(isMachineText)).toEqual([false, false, false, false])
  })
})

describe('Also today', () => {
  const notes = [
    line('calendar:a', 'schedule', { sourceType: 'calendar', statusLabel: '9:00 AM', title: 'Standup' }),
    line('calendar:b', 'schedule', { sourceType: 'calendar', statusLabel: '1:30 PM', title: 'Triage' }),
    line('calendar:c', 'schedule', { sourceType: 'calendar', statusLabel: 'ALL DAY', title: 'Offsite' }),
    line('notification:d', 'highlights', { sourceType: 'notification', statusLabel: null, title: 'Shipped #734' }),
  ]

  it('names the next meeting still ahead, then counts what else the day holds', () => {
    expect(alsoToday(notes, 10 * 60 + 30)).toEqual({ next: '1:30 PM Triage', more: '+2 meetings · 1 note' })
    expect(alsoToday(notes.slice(1, 2), 0)).toEqual({ next: '1:30 PM Triage', more: null })
  })

  it('counts every meeting, with no "+", once none is ahead', () => {
    expect(alsoToday(notes, 18 * 60)).toEqual({ next: null, more: '3 meetings · 1 note' })
    expect(alsoToday(notes.slice(3), 0)).toEqual({ next: null, more: '1 note' })
    expect(alsoToday([], 0)).toEqual({ next: null, more: null })
  })
})

describe('sentences', () => {
  it('splits a lede at its full stops, not inside a clause, an abbreviation in lower case or a number', () => {
    expect(sentences('Triage day: Jon needs it (the root-owned one). There are 3.5 items, e.g. this. Start with Jon!')).toEqual([
      'Triage day: Jon needs it (the root-owned one).',
      'There are 3.5 items, e.g. this.',
      'Start with Jon!',
    ])
    expect(sentences('  ')).toEqual([])
  })
})

describe('ledeStops', () => {
  const LEDE =
    'Triage day: Jon needs the inbox, and Doug has breakage to route, much of it one problem (the root-owned config, plus the missing user) with a separate pipeline bug. Three items await approval; the rest can wait. Start with Jon.'

  it('offers every full stop, longest first, then the first sentence cut back at each clause boundary', () => {
    expect(ledeStops(LEDE)).toEqual([
      LEDE,
      'Triage day: Jon needs the inbox, and Doug has breakage to route, much of it one problem (the root-owned config, plus the missing user) with a separate pipeline bug. Three items await approval; the rest can wait.',
      'Triage day: Jon needs the inbox, and Doug has breakage to route, much of it one problem (the root-owned config, plus the missing user) with a separate pipeline bug.',
      'Triage day: Jon needs the inbox, and Doug has breakage to route, much of it one problem.',
      'Triage day.',
    ])
  })

  it('ends every stop on a full stop and never on an ellipsis, a comma or an open bracket', () => {
    const stops = ledeStops('Ship it; the fix (see #734: the key change) is in, mostly — review at 11:00 with Jon. Then rest.')
    expect(stops).toEqual([
      'Ship it; the fix (see #734: the key change) is in, mostly — review at 11:00 with Jon. Then rest.',
      'Ship it; the fix (see #734: the key change) is in, mostly — review at 11:00 with Jon.',
      'Ship it; the fix (see #734: the key change) is in, mostly.',
      'Ship it; the fix.',
      'Ship it.',
    ])
    for (const stop of stops) expect(stop).toMatch(/[^,…(]\.$|[!?]$/)
  })

  it('never cuts inside bold or code, and has nothing to say about an empty lede', () => {
    expect(ledeStops('Read **the plan: part one** first.')).toEqual(['Read **the plan: part one** first.'])
    expect(ledeStops('One sentence with no clause to cut at')).toEqual(['One sentence with no clause to cut at'])
    expect(ledeStops('  ')).toEqual([])
  })
})

describe('openLabel', () => {
  it('names where Open goes, by what the card is', () => {
    const cards = buildInboxCards(
      input({
        brief: brief([line('task:a', 'action', { sourceId: 'a' }), line('approval:e', 'action', { sourceType: 'approval', sourceId: 'e' })]),
        queues: queues([ticket('b')]),
        sessions: [session('s', { working: true })],
        runs: [run('r1')],
        artifacts: [doc('d', hoursAgo(1))],
      }),
    )
    expect(Object.fromEntries(cards.map((c) => [c.key, openLabel(c)]))).toEqual({
      'brief:task:a': 'Open ticket',
      'brief:approval:e': 'Open',
      'ticket:b': 'Open ticket',
      'session:s': 'Open session',
      'run:r1': 'Open run',
      'doc:d': 'Open doc',
    })
  })
})
