import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { QueryClient } from '@tanstack/query-core'

// The design is the server's (talaria-agent-design); this module is a read,
// four verbs and pure helpers. svelte-query's barrel pulls .svelte files the
// node environment cannot transform, so its one export used here is mocked;
// the cache the verbs write is query-core's own QueryClient, the class
// svelte-query re-exports.
vi.mock('@tanstack/svelte-query', () => ({ createQuery: vi.fn() }))

import {
  DESIGN_KEY,
  designHeadline,
  discardDesign,
  elapsedSeconds,
  fetchAgentDesign,
  landingKey,
  openingStep,
  pollEvery,
  refineDesign,
  refineReceipt,
  saveDesign,
  startDesign,
  watchDesign,
  type AgentDesign,
  type DesignRead,
  type DraftFields,
  isConflict,
  pendingDesign,
  keepaliveBody,
  KEEPALIVE_MAX_BYTES,
  stashUnsaved,
  takeUnsaved,
  clearUnsavedEdits,
} from './agent-design'

const DRAFT: DraftFields = {
  name: 'Scout',
  handle: 'scout',
  department: 'research',
  role: 'Research Analyst',
  soul: 'A careful analyst.',
  skills: [{ name: 'weekly-digest', content: '# Weekly digest' }],
}

function design(patch: Partial<AgentDesign> = {}): AgentDesign {
  return {
    id: 'd-1',
    runId: 'r-1',
    status: 'generating',
    refining: false,
    purpose: 'A release manager that tracks deploy trains',
    instruction: 'A release manager that tracks deploy trains',
    chatLength: 0,
    draft: null,
    previous: null,
    error: null,
    startedAt: 1_000_000,
    finishedAt: null,
    updatedAt: 1_000_000,
    hireRunId: null,
    ...patch,
  }
}

const ready = (patch: Partial<AgentDesign> = {}) =>
  design({
    status: 'ready',
    draft: DRAFT,
    finishedAt: 1_020_000,
    chatLength: 2,
    ...patch,
  })

type Call = { url: string; method: string; body: unknown }
let calls: Call[] = []
let answer: (call: Call) => { status: number; body: unknown }

beforeEach(() => {
  calls = []
  answer = () => ({ status: 200, body: { design: null, now: 0 } })
  vi.stubGlobal('fetch', async (url: string, init?: RequestInit) => {
    const call = { url, method: init?.method ?? 'GET', body: init?.body ? JSON.parse(String(init.body)) : undefined }
    calls.push(call)
    const a = answer(call)
    return new Response(JSON.stringify(a.body), { status: a.status, headers: { 'content-type': 'application/json' } })
  })
})
afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

describe('resume after a reload: the modal opens where the server says', () => {
  it('a design still generating opens on describe, its splash counting from the server start', async () => {
    // The tab reloaded 25 s into a design. Its clock is 7 s behind the
    // server's; the count is the server's anyway.
    answer = () => ({ status: 200, body: { design: design(), now: 1_025_000 } })
    vi.setSystemTime(1_018_000)
    const read = await fetchAgentDesign(false)
    expect(openingStep(read.design, false)).toBe('describe')
    expect(read.design?.status).toBe('generating')
    expect(elapsedSeconds(read.design, read.skewMs)).toBe(25)
    // A second tab, read 3 s later with its own clock: the same count.
    vi.setSystemTime(1_100_000)
    answer = () => ({ status: 200, body: { design: design(), now: 1_028_000 } })
    const second = await fetchAgentDesign(false)
    expect(elapsedSeconds(second.design, second.skewMs)).toBe(28)
  })

  it('a design that landed while the tab was gone opens on review', async () => {
    answer = () => ({ status: 200, body: { design: ready(), now: 2_000_000 } })
    const read = await fetchAgentDesign(false)
    expect(openingStep(read.design, false)).toBe('review')
    expect(read.design?.draft?.name).toBe('Scout')
  })

  it('a refine in flight, or one that failed over a draft, opens on review', () => {
    expect(openingStep(ready({ status: 'generating', refining: true, finishedAt: null }), false)).toBe('review')
    expect(openingStep(ready({ status: 'error', error: 'no' }), false)).toBe('review')
  })

  it('a describe that failed opens on describe, where its sentence shows', () => {
    expect(openingStep(design({ status: 'error', error: 'Muse could not design an agent' }), false)).toBe('describe')
    expect(openingStep(null, false)).toBe('describe')
    expect(openingStep(null, true)).toBe('review')
  })

  it('the open modal reads with ?watching=1, so a landing skips the bell', async () => {
    const release = watchDesign()
    await fetchAgentDesign(true)
    release()
    expect(calls.at(-1)?.url).toBe('/api/fleet/agent-design?watching=1')
    await fetchAgentDesign(false)
    expect(calls.at(-1)?.url).toBe('/api/fleet/agent-design')
  })
})

describe('the poll', () => {
  it('asks quickly while watched and generating, slowly in the background, never once settled', () => {
    const r = (d: AgentDesign | null, hiring: DesignRead['hiring'] = []): DesignRead => ({ design: d, hiring, returned: [], skewMs: 0 })
    expect(pollEvery(r(design()), true)).toBe(2_000)
    expect(pollEvery(r(design()), false)).toBe(5_000)
    expect(pollEvery(r(ready()), true)).toBe(false)
    expect(pollEvery(null, true)).toBe(false)
    // A hire in flight is settled by a read: asked for while it runs.
    const h = [{ designId: 'd-0', hireRunId: 'h-1', name: 'A' }]
    expect(pollEvery(r(null, h), true)).toBe(5_000)
    expect(pollEvery(r(ready(), h), false)).toBe(10_000)
  })
})

describe('the form reseeds on a landing, never on a save', () => {
  it('a landing changes the key; the answer to an edit does not', () => {
    const generating = design()
    const landed = ready()
    expect(landingKey(generating)).not.toBe(landingKey(landed))
    const edited = { ...landed, draft: { ...DRAFT, role: 'Lead' }, updatedAt: 1_030_000 }
    expect(landingKey(edited)).toBe(landingKey(landed))
    expect(landingKey(null)).toBe('')
  })
})

describe('the verbs: one call each, the answer in the cache', () => {
  const qc = () => {
    const c = new QueryClient()
    c.setQueryData<DesignRead>(DESIGN_KEY, { design: null, hiring: [], returned: [], skewMs: 1234 })
    return c
  }
  const read = (c: QueryClient) => c.getQueryData<DesignRead>(DESIGN_KEY)

  it('a describe posts the instruction and caches the generating design', async () => {
    const c = qc()
    answer = () => ({ status: 200, body: { design: design() } })
    await startDesign(c, '  A release manager  ')
    expect(calls).toEqual([{ url: '/api/fleet/agent-design', method: 'POST', body: { instruction: 'A release manager' } }])
    expect(read(c)?.design?.status).toBe('generating')
    expect(read(c)?.skewMs).toBe(1234)
  })

  it('a refine posts the form as the person sees it', async () => {
    const c = qc()
    answer = () => ({ status: 200, body: { design: ready({ status: 'generating', refining: true }) } })
    const form = { ...DRAFT, role: 'Lead Analyst' }
    await refineDesign(c, 'd-1', 'make it terse', form)
    expect(calls[0]).toEqual({
      url: '/api/fleet/agent-design',
      method: 'POST',
      body: { instruction: 'make it terse', refine: true, id: 'd-1', current: form },
    })
    expect(read(c)?.design?.refining).toBe(true)
  })

  it('a hand edit is a PATCH of the whole draft, naming the design it edits', async () => {
    const c = qc()
    answer = () => ({ status: 200, body: { design: ready() } })
    await saveDesign(c, DRAFT, 'd-1')
    expect(calls[0]).toEqual({ url: '/api/fleet/agent-design', method: 'PATCH', body: { ...DRAFT, id: 'd-1' } })
    // A manual start names none: there is no design yet.
    await saveDesign(c, DRAFT)
    expect(calls[1]?.body).toEqual(DRAFT)
  })

  it('the save on the way out survives the tab (keepalive)', async () => {
    const c = qc()
    let init: RequestInit | undefined
    vi.stubGlobal('fetch', async (_url: string, i?: RequestInit) => {
      init = i
      return new Response(JSON.stringify({ design: ready() }), { status: 200, headers: { 'content-type': 'application/json' } })
    })
    await saveDesign(c, DRAFT, 'd-1', { keepalive: true })
    expect(init?.keepalive).toBe(true)
    expect(init?.method).toBe('PATCH')
  })

  it('a stale tab is told by a 409 it can tell apart', async () => {
    const c = qc()
    answer = () => ({ status: 409, body: { error: 'this agent design was replaced or discarded since it was opened — showing the current one' } })
    const e = await saveDesign(c, DRAFT, 'd-old').catch((x: unknown) => x)
    expect(isConflict(e)).toBe(true)
    expect(isConflict(new Error('network'))).toBe(false)
  })

  it('a discard names the design, and one already gone is done', async () => {
    const c = qc()
    c.setQueryData<DesignRead>(DESIGN_KEY, { design: ready(), hiring: [], returned: [], skewMs: 0 })
    answer = () => ({ status: 404, body: { error: 'not found' } })
    await discardDesign(c, 'd-1')
    expect(calls[0]).toEqual({ url: '/api/fleet/agent-design', method: 'DELETE', body: { id: 'd-1' } })
    expect(read(c)?.design).toBeNull()
  })

  it('a refused verb throws the server sentence and leaves the cache alone', async () => {
    const c = qc()
    c.setQueryData<DesignRead>(DESIGN_KEY, { design: design(), hiring: [], returned: [], skewMs: 0 })
    answer = () => ({ status: 409, body: { error: 'an agent design is still generating — wait for it, or discard it' } })
    await expect(saveDesign(c, DRAFT)).rejects.toThrow(/still generating/)
    expect(read(c)?.design?.status).toBe('generating')
  })
})

describe('a design on a hire is out of the slot', () => {
  it('leaves New agent open: the read lists it as hiring, not as the design', async () => {
    answer = () => ({
      status: 200,
      body: { design: null, hiring: [{ designId: 'd-0', hireRunId: 'h-1', name: 'A' }], returned: [], now: 0 },
    })
    const read = await fetchAgentDesign(false)
    expect(read.design).toBeNull()
    expect(openingStep(pendingDesign(read.design), false)).toBe('describe')
    expect(read.hiring[0]?.hireRunId).toBe('h-1')
  })

  it('a design handed back opens on review, saying why', () => {
    const back = ready({ error: 'The hire did not finish: the department is taken. Check the fields and create it again.' })
    expect(openingStep(pendingDesign(back), false)).toBe('review')
  })
})

describe('what the review and the roster say', () => {
  it('the receipt compares a landed refine with the form it started from', () => {
    const before = { ...DRAFT, role: 'Analyst', soul: 'one\ntwo' }
    const r = refineReceipt(ready({ previous: before, draft: { ...DRAFT, soul: 'one\nthree' } }))
    expect(r?.fields.map((f) => f.label)).toEqual(['Role', 'Soul'])
    expect(refineReceipt(ready())).toBeNull()
    expect(refineReceipt(design({ previous: before }))).toBeNull()
  })

  it('the headline is the purpose while it runs and the name once it lands', () => {
    expect(designHeadline(design())).toBe('A release manager that tracks deploy trains')
    expect(designHeadline(design({ purpose: 'one two three four five six seven eight.' }))).toBe('one two three four five six seven')
    expect(designHeadline(ready())).toBe('Scout')
  })
})

describe('the save that leaves with the page', () => {
  const big = { ...DRAFT, soul: 'x'.repeat(200_000) }

  it('sends the whole edit when it fits the keepalive cap', () => {
    const { fields, whole } = keepaliveBody(DRAFT, null)
    expect(whole).toBe(true)
    expect(fields).toEqual(DRAFT)
  })

  it('over the cap, sends the small fields over the server soul and says it is partial', () => {
    const server = { ...DRAFT, soul: 'the saved soul' }
    const { fields, whole } = keepaliveBody({ ...big, name: 'Renamed' }, server)
    expect(whole).toBe(false)
    expect(fields?.name).toBe('Renamed')
    expect(fields?.soul).toBe('the saved soul')
    expect(new TextEncoder().encode(JSON.stringify(fields)).length).toBeLessThanOrEqual(KEEPALIVE_MAX_BYTES)
  })

  it('over the cap with a large saved soul too, sends nothing and keeps the edit here', () => {
    const { fields, whole } = keepaliveBody(big, { ...DRAFT, soul: 'y'.repeat(100_000) })
    expect(whole).toBe(false)
    expect(fields).toBeNull()
  })

  it('keeps the edit in this browser, per person, and offers it once unless the server moved on', () => {
    const store = new Map<string, string>()
    vi.stubGlobal('localStorage', {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, v),
      removeItem: (k: string) => void store.delete(k),
      key: (n: number) => [...store.keys()][n] ?? null,
      get length() {
        return store.size
      },
    })
    const sent = { ...big, soul: DRAFT.soul }
    const stash = () => stashUnsaved('u-1', { id: 'd-1', form: big, base: DRAFT, sent, at: 1_000 })
    stash()
    // Another person at this browser: never offered.
    expect(takeUnsaved('u-2', 'd-1', DRAFT, 2_000)).toBeNull()
    expect(takeUnsaved('u-1', 'd-2', DRAFT, 2_000)).toBeNull() // another design (and it is gone)
    stash()
    // The server holds what the tab had: offered, once.
    expect(takeUnsaved('u-1', 'd-1', DRAFT, 2_000)?.soul.length).toBe(200_000)
    expect(takeUnsaved('u-1', 'd-1', DRAFT, 2_000)).toBeNull()
    stash()
    // The server holds what the keepalive save sent: still offered.
    expect(takeUnsaved('u-1', 'd-1', sent, 2_000)?.soul.length).toBe(200_000)
    stash()
    // Something newer landed since (another tab, a later edit): dropped.
    expect(takeUnsaved('u-1', 'd-1', { ...DRAFT, handle: 'renamed' }, 2_000)).toBeNull()
    stash()
    // A day later it is not offered.
    expect(takeUnsaved('u-1', 'd-1', DRAFT, 1_000 + 25 * 3600 * 1000)).toBeNull()
    // Sign-out clears every person's stash at this browser.
    stash()
    stashUnsaved('u-2', { id: null, form: DRAFT, base: null, sent: null, at: 1_000 })
    clearUnsavedEdits()
    expect(store.size).toBe(0)
  })

})
