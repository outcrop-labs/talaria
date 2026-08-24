import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// The read that opens. The brief used to be openable only by the scheduled
// pass, so a missed tick meant a blank page all day; getBrief now kicks the
// open itself and answers 'writing' until the document lands. These tests hold
// the database, the sources and the model in fakes and assert THIS file's
// accounting: the kick, the writing state on both sides of the first append,
// and the birth guard that keeps a sweep from racing the open.

const state = vi.hoisted(() => {
  interface FakeRow {
    id: string
    userId: string
    briefDate: string
    zone: string
    agentModel: string | null
    agentName: string | null
    artifactId: string | null
    lastSeq: number
    readSeq: number
    lastSweptAt: string | null
    createdAt: string
  }
  return {
    agentRows: [] as Array<{ model: string; displayName: string }>,
    briefs: [] as FakeRow[],
    entries: new Map<string, unknown[]>(),
    briefSeq: 0,
    publishes: [] as Array<{ userId: string; event: { type: string } }>,
    // The person's stored zone (users.timezone); null = follow the workspace.
    userTz: null as string | null,
  }
})

// A postgres.js-shaped tagged template answering by query text, so a query
// this module stops issuing stops being answered, instead of silently
// inheriting the shape meant for the one before it.
const makeSql = () => {
  const sql = ((strings: TemplateStringsArray, ...values: unknown[]) => {
    const text = strings.join(' ').replace(/\s+/g, ' ').trim()
    if (text.includes('from app_settings')) return Promise.resolve([])
    if (text.includes('from agent_defs')) return Promise.resolve(state.agentRows)
    if (text.includes('insert into daily_briefs')) {
      const row = {
        id: `brief-${++state.briefSeq}`,
        userId: values[0] as string,
        briefDate: values[1] as string,
        zone: values[2] as string,
        agentModel: values[3] as string | null,
        agentName: values[4] as string | null,
        artifactId: null,
        lastSeq: 0,
        readSeq: 0,
        lastSweptAt: null,
        createdAt: new Date().toISOString(),
      }
      state.briefs.push(row)
      return Promise.resolve([row])
    }
    if (text.includes('set last_seq = last_seq +')) {
      // values: [count, briefId]
      const row = state.briefs.find((b) => b.id === values[1])
      if (!row) return Promise.resolve([])
      row.lastSeq += values[0] as number
      return Promise.resolve([{ lastSeq: row.lastSeq }])
    }
    if (text.includes('insert into daily_brief_entries')) {
      const briefId = values[0] as string
      const seq = values[1] as number
      const entry = {
        id: `e${seq}`,
        seq,
        batch: values[2] as string,
        kind: values[3] as string,
        section: values[4] as string,
        sourceKey: (values[5] as string | null) ?? null,
        sourceType: (values[6] as string | null) ?? null,
        sourceId: (values[7] as string | null) ?? null,
        sourceHref: (values[8] as string | null) ?? null,
        fingerprint: (values[9] as string | null) ?? null,
        supersedes: (values[10] as string | null) ?? null,
        priority: (values[11] as string | null) ?? null,
        statusLabel: (values[12] as string | null) ?? null,
        badge: (values[13] as string | null) ?? null,
        title: (values[14] as string) ?? '',
        body: (values[15] as string) ?? '',
        evidence: (values[16] as unknown[]) ?? [],
        createdAt: new Date().toISOString(),
      }
      const list = state.entries.get(briefId) ?? []
      list.push(entry)
      state.entries.set(briefId, list)
      return Promise.resolve([entry])
    }
    // loadRecentRow: the recent-brief fallback (no date filter, live docs only)
    if (text.includes('last_seq > 0 and created_at >')) {
      return Promise.resolve(
        state.briefs
          .filter((b) => b.userId === values[0] && b.lastSeq > 0 && b.createdAt > String(values[1]))
          .sort((a, b) => (a.briefDate < b.briefDate ? 1 : -1))
          .slice(0, 1),
      )
    }
    if (text.includes('from daily_briefs where user_id')) {
      return Promise.resolve(
        state.briefs.filter((b) => b.userId === values[0] && b.briefDate === values[1]),
      )
    }
    if (text.includes('from daily_brief_entries')) {
      return Promise.resolve(state.entries.get(values[0] as string) ?? [])
    }
    if (text.includes('set last_swept_at')) {
      const row = state.briefs.find((b) => b.id === values[0])
      if (row) row.lastSweptAt = new Date().toISOString()
      return Promise.resolve([])
    }
    return Promise.resolve([])
  }) as unknown as {
    (strings: TemplateStringsArray, ...values: unknown[]): Promise<unknown[]>
    json: (v: unknown) => unknown
  }
  sql.json = (v: unknown) => v
  return sql
}

const publishUser = vi.fn((userId: string, event: { type: string }) => {
  state.publishes.push({ userId, event })
})

vi.mock('@/server/db/pg', () => ({ db: async () => makeSql() }))
// Whole-module stub (not importOriginal-spread): the real users.ts drags the
// auth session graph (redis, crypto) into a test that never touches it, and
// nothing else in this file's module graph imports users.ts for anything.
vi.mock('@/server/users', () => ({ getTimezone: async () => state.userTz }))
vi.mock('@/server/scheduler', () => ({ registerJob: () => {} }))
vi.mock('@/server/realtime', () => ({ publishUser }))
vi.mock('@/server/harness/run', () => ({
  // The lede model call, faked to a fixed sentence. onFailure paths are not
  // under test here; the fallback lede has its own coverage via the harness.
  runHarness: async () => ({ value: 'Two things need you; the ledger first.' }),
}))
vi.mock('@/server/harness/defs/briefer', () => ({ dailyBriefLedeHarness: {}, dailyBriefNoteHarness: {} }))
vi.mock('@/server/google/calendar', () => ({ listUpcomingEvents: async () => null }))
vi.mock('@/server/inbox-focus-sources', () => ({
  approvalItems: async () => [],
  taskItems: async () => [],
  notificationItems: async () => [],
}))
vi.mock('@/server/daily-brief-comms', () => ({ commsLines: async () => [] }))
vi.mock('@/server/daily-brief-delegation', () => ({ draftReply: async () => null, releaseDrafts: async () => 0 }))
vi.mock('@/server/daily-brief-artifact', () => ({ mirrorBriefArtifact: async () => {} }))

const user = { id: 'u1', sub: '', email: 'jon@example.com', name: 'Jon', picture: null, provider: 'local', role: 'member' } as never

const load = async () => {
  vi.resetModules()
  return await import('@/server/daily-brief')
}

/** The detached open chain lands within a bounded number of microtask turns —
 *  every fake in the graph resolves without a timer. The yield is what lets
 *  those turns happen: a busy loop would starve the very chain it waits on. */
const until = async (cond: () => boolean) => {
  for (let i = 0; i < 50_000; i++) {
    if (cond()) return
    await Promise.resolve()
  }
  expect(cond()).toBe(true)
}

// The default config is UTC, workday 09:00, lead 2h → the brief is due from
// 07:00 local. 10:00Z is comfortably inside the window on any runner whose TZ
// is UTC; TZ is stubbed so the config's module-load default agrees.
const DUE_AT = new Date('2026-08-20T10:00:00Z')
const NOT_YET = new Date('2026-08-20T05:00:00Z')

describe('getBrief opens on demand', () => {
  beforeEach(() => {
    vi.stubEnv('TZ', 'UTC')
    state.agentRows.length = 0
    state.briefs.length = 0
    state.entries.clear()
    state.briefSeq = 0
    state.publishes.length = 0
    state.userTz = null
    state.agentRows.push({ model: 'aide-1', displayName: 'Aida' })
  })
  afterEach(() => {
    vi.unstubAllEnvs()
  })

  it("answers 'writing' past the due hour with no brief, then opens — lede first", async () => {
    const lib = await load()
    const first = await lib.getBrief(user, DUE_AT)
    // The read does not WAIT on the open (a model call) — it says what is
    // happening and lets the append publish.
    expect(first).toMatchObject({ absent: 'writing', nextAt: null, agent: { configured: true, name: 'Aida' } })

    await until(() => state.briefs.length === 1 && (state.entries.get(state.briefs[0]!.id)?.length ?? 0) > 0)
    const entries = state.entries.get(state.briefs[0]!.id)!
    // Seq 1 is the lede, always — never an item above the day's opening read.
    expect(entries[0]).toMatchObject({ seq: 1, kind: 'lede' })
    // The append published, which is what turns an open page into a document.
    expect(state.publishes.some((p) => p.userId === 'u1' && p.event.type === 'brief')).toBe(true)

    // And the NEXT read is the document, not the writing state.
    const second = await lib.getBrief(user, DUE_AT)
    expect(second).toMatchObject({ lede: 'Two things need you; the ledger first.', agent: { name: 'Aida' } })
  })

  it("re-kicks at most one open per person no matter how fast the 'writing' polls arrive", async () => {
    const lib = await load()
    await lib.getBrief(user, DUE_AT)
    await lib.getBrief(user, DUE_AT)
    await lib.getBrief(user, DUE_AT)
    await until(() => state.briefs.length === 1)
    // The 4s client poll during 'writing' must not stampede opens: the row is
    // claimed once and every subsequent read is a cheap 'writing' answer.
    expect(state.briefs).toHaveLength(1)
  })

  it("answers 'writing' while the row exists but its first batch has not landed, and refuses to sweep it", async () => {
    const lib = await load()
    // A row with lastSeq 0: the open inserted it and is mid-flight on the
    // model call for the lede. Birth, not a document.
    state.briefs.push({
      id: 'brief-born',
      userId: 'u1',
      briefDate: '2026-08-20',
      zone: 'UTC',
      agentModel: 'aide-1',
      agentName: 'Aida',
      artifactId: null,
      lastSeq: 0,
      readSeq: 0,
      lastSweptAt: null,
      createdAt: new Date().toISOString(),
    })
    expect(await lib.getBrief(user, DUE_AT)).toMatchObject({ absent: 'writing' })
    // The sweep throttle is clear (never swept) and the row exists — the old
    // sweepIfDue would have run. The birth guard is what stops it: a sweep now
    // would race the open's own append.
    expect(await lib.sweepIfDue(user, DUE_AT)).toBeNull()
    expect((await lib.sweepBrief(user, DUE_AT) as { appended: number }).appended).toBe(0)
    expect(state.entries.get('brief-born')).toBeUndefined()
  })

  it("answers 'pending' before the brief hour, opening nothing", async () => {
    const lib = await load()
    const res = await lib.getBrief(user, NOT_YET)
    expect(res).toMatchObject({ absent: 'pending' })
    expect((res as { nextAt: string | null }).nextAt).toBeTruthy()
    expect(state.briefs).toHaveLength(0)
  })

  it("never answers 'pending' over a readable document — the recent brief is served", async () => {
    const lib = await load()
    // Yesterday's brief exists (created recently); the read happens before
    // today's fire hour. The surface must show the DOCUMENT, titled with its
    // own date — not "your next brief opens tomorrow".
    state.briefs.push({
      id: 'brief-yesterday',
      userId: 'u1',
      briefDate: '2026-08-19',
      zone: 'UTC',
      agentModel: 'aide-1',
      agentName: 'Aida',
      artifactId: null,
      lastSeq: 4,
      readSeq: 0,
      lastSweptAt: null,
      createdAt: new Date(Date.now() - 6 * 3600_000).toISOString(),
    })
    state.entries.set('brief-yesterday', [
      { id: 'y1', seq: 1, batch: 'b', kind: 'lede', section: 'action', sourceKey: null, sourceType: null, sourceId: null, sourceHref: null, fingerprint: null, supersedes: null, priority: null, statusLabel: null, badge: null, title: 'Daily brief — 2026-08-19', body: 'Quiet day.', evidence: [], createdAt: new Date().toISOString() },
    ])
    const res = await lib.getBrief(user, NOT_YET)
    expect(res).toMatchObject({ date: '2026-08-19', lede: 'Quiet day.' })
    // and it opened nothing — the pending hour is not an open trigger
    expect(state.briefs).toHaveLength(1)
  })

  it("answers 'pending' only when there is genuinely no brief to show", async () => {
    const lib = await load()
    expect(await lib.getBrief(user, NOT_YET)).toMatchObject({ absent: 'pending' })
  })

  it('honours the reader timezone: an evening in Denver is not tomorrow in UTC', async () => {
    const lib = await load()
    // 00:30 UTC on Aug 21 is 18:30 on Aug 20 in America/Denver. Under the UTC
    // config this moment is "not due until 07:00"; with the reader's zone it
    // is squarely inside the workday — so the read opens a brief FOR AUG 20
    // (the reader's today) instead of promising one tomorrow.
    const denverEveningUtc = new Date('2026-08-21T00:30:00Z')
    const res = await lib.getBrief(user, denverEveningUtc, 'America/Denver')
    expect(res).toMatchObject({ absent: 'writing' })
    await until(() => state.briefs.length === 1)
    expect(state.briefs[0]!.briefDate).toBe('2026-08-20')
    expect(state.briefs[0]!.zone).toBe('America/Denver')
  })

  it('discards an unparseable timezone and falls back to the config zone', async () => {
    const lib = await load()
    const res = await lib.getBrief(user, NOT_YET, 'not/a zone!!')
    expect(res).toMatchObject({ absent: 'pending' })
    expect(state.briefs).toHaveLength(0)
  })

  it('lets the stored zone beat the browser’s: the set zone is the contract', async () => {
    const lib = await load()
    // 23:30Z on Aug 20 is Aug 21 08:30 in Tokyo (due, FOR Aug 21) but still
    // Aug 20 17:30 in Denver (due, for Aug 20). The person SET Tokyo; a
    // laptop reporting Denver — travel, a VPN, a wrong system clock — must
    // not re-file their day. Under browser-wins this opens Aug 20.
    state.userTz = 'Asia/Tokyo'
    const res = await lib.getBrief(user, new Date('2026-08-20T23:30:00Z'), 'America/Denver')
    expect(res).toMatchObject({ absent: 'writing' })
    await until(() => state.briefs.length === 1)
    expect(state.briefs[0]!.briefDate).toBe('2026-08-21')
    expect(state.briefs[0]!.zone).toBe('Asia/Tokyo')
  })

  it('degrades to UTC on an unreadable STORED zone instead of throwing', async () => {
    const lib = await load()
    // Only reachable by direct DB edit — the PUT validates — but the rule a
    // typo in one person's row must not stop their brief is the same one the
    // org-config typo already follows: localMoment warns and reads UTC.
    state.userTz = 'bogus/zone'
    const res = await lib.getBrief(user, DUE_AT)
    expect(res).toMatchObject({ absent: 'writing' })
    await until(() => state.briefs.length === 1)
    expect(state.briefs[0]!.briefDate).toBe('2026-08-20')
  })

  it("answers 'no-agent' when nothing can write the brief, opening nothing", async () => {
    state.agentRows.length = 0
    const lib = await load()
    expect(await lib.getBrief(user, DUE_AT)).toMatchObject({ absent: 'no-agent' })
    expect(state.briefs).toHaveLength(0)
  })
})
