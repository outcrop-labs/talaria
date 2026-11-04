import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { AgentDraft } from './muse.svelte'

// Both peer stores are mocked at the MODULE boundary, not the fetch one.
// The real muse module pulls @tanstack/svelte-query, whose barrel imports
// .svelte files the node test environment cannot transform; toast.svelte is
// a RUNES module ($state), and the node environment runs no Svelte
// transform either. The store's contract is the ORCHESTRATION (what it
// sends, what it keeps, what it exposes), and the mocked seams are the
// exact APIs the store consumes: draftAgent() and pushToast(). Errors are
// thrown the way the real helper throws (an Error with the person-readable
// sentence), which is all the store sees either way.
const draftAgentMock = vi.hoisted(() => vi.fn<(input: Record<string, unknown>) => Promise<AgentDraft>>())
const pushToastMock = vi.hoisted(() => vi.fn())
vi.mock('@/lib/muse.svelte', () => ({ draftAgent: draftAgentMock }))
vi.mock('@/lib/toast.svelte', () => ({ pushToast: pushToastMock }))

import { adoptDesign, clearDesign, currentDesign, designGenerating, ensureDesign, refineDesign, setModalOpen, startDesign } from './agent-design.svelte'

// The store is MODULE state, so every test starts from clearDesign() — the
// real "the caller claimed it" transition, not a resetModules dance.

const DRAFT: AgentDraft = {
  name: 'Scout',
  handle: 'scout',
  department: 'research',
  role: 'Research Analyst',
  soul: 'A careful analyst.',
  skills: [{ name: 'weekly-digest', content: '# Weekly digest' }],
}

const OTHER: AgentDraft = { ...DRAFT, name: 'Sentinel', handle: 'sentinel', role: 'Watchman' }
beforeEach(() => {
  draftAgentMock.mockReset()
  pushToastMock.mockClear()
  setModalOpen(true) // default: no toasts while "the modal is open"
  clearDesign()
})
afterEach(() => {
  clearDesign()
  setModalOpen(false)
})

// A pending promise the test resolves when it wants the turn to land.
function pendingDraft() {
  let resolve!: (d: AgentDraft) => void
  const promise = new Promise<AgentDraft>((r) => (resolve = r))
  return { promise, resolve }
}

// The store fires run() and forgets it; completion is observable only through
// the design's status. Every link on the test path is a microtask (the mock
// resolves immediately and the 1s tick is wall-clock irrelevant), so a
// bounded microtask drain settles the whole chain — deterministic, no
// wall-clock wait, no timer games.
const settle = async () => {
  for (let i = 0; i < 25; i++) await Promise.resolve()
}

describe('startDesign: the single-flight rule', () => {
  it('fills the design and lands the draft as ready', async () => {
    const gate = pendingDraft()
    draftAgentMock.mockImplementation(() => gate.promise)
    const id = startDesign('A release manager that tracks deploy trains')
    let design = currentDesign()
    expect(design?.status).toBe('generating')
    expect(design?.purpose).toBe('A release manager that tracks deploy trains')
    expect(design?.name).toBe('')
    expect(id).toBe(design?.id)

    gate.resolve(DRAFT)
    await settle()
    design = currentDesign()
    expect(design?.status).toBe('ready')
    expect(design?.name).toBe('Scout')
    expect(design?.handle).toBe('scout')
    expect(design?.soul).toBe('A careful analyst.')
    expect(design?.skills).toEqual(DRAFT.skills)
    expect(designGenerating()).toBe(false)
    // The chat the next refine sees is the VALIDATED draft, not raw model text.
    expect(design?.chat).toEqual([
      { role: 'user', content: 'A release manager that tracks deploy trains' },
      { role: 'assistant', content: JSON.stringify(DRAFT) },
    ])
  })

  it('sends the describe shape: instruction + chat, no current', async () => {
    draftAgentMock.mockImplementation(async () => DRAFT)
    startDesign('design a night auditor')
    await settle()
    expect(draftAgentMock).toHaveBeenCalledWith({
      kind: undefined,
      instruction: 'design a night auditor',
      chat: [],
    } as never)
    const input = draftAgentMock.mock.calls[0]?.[0] as Record<string, unknown>
    expect(input.instruction).toBe('design a night auditor')
    expect(input.chat).toEqual([])
    expect('current' in input).toBe(false)
  })

  it('does NOT replace an in-flight run — a second start returns the same id', async () => {
    const gate = pendingDraft()
    draftAgentMock.mockImplementation(() => gate.promise)
    const first = startDesign('first purpose')
    const second = startDesign('a different purpose entirely')
    expect(second).toBe(first)
    // And the in-flight run is untouched: still the first purpose, one call.
    expect(currentDesign()?.purpose).toBe('first purpose')
    expect(draftAgentMock).toHaveBeenCalledTimes(1)

    gate.resolve(DRAFT)
    await settle()
    expect(currentDesign()?.status).toBe('ready')
  })

  it('DOES replace an idle finished design — a fresh describe is a fresh intent', async () => {
    draftAgentMock.mockImplementation(async () => DRAFT)
    const first = startDesign('first purpose')
    await settle()
    expect(currentDesign()?.status).toBe('ready')

    draftAgentMock.mockClear()
    const second = startDesign('second purpose')
    expect(second).not.toBe(first)
    expect(currentDesign()?.purpose).toBe('second purpose')
    expect(currentDesign()?.name).toBe('') // the old fields do not leak into the new run
  })

  it('captures the error sentence as status error, purpose intact for a reopen', async () => {
    draftAgentMock.mockRejectedValue(new Error('the gateway has no model'))
    startDesign('design a fact checker')
    await settle()
    const design = currentDesign()
    expect(design?.status).toBe('error')
    expect(design?.error).toBe('the gateway has no model')
    expect(design?.purpose).toBe('design a fact checker')
    expect(designGenerating()).toBe(false)
  })
})

describe('refineDesign: the refine path through the store', () => {
  const landed = async () => {
    draftAgentMock.mockImplementation(async () => DRAFT)
    startDesign('first purpose')
    await settle()
    draftAgentMock.mockClear()
  }

  it('sends current fields + chat window, and replaces the fields', async () => {
    await landed()
    draftAgentMock.mockImplementation(async () => OTHER)
    refineDesign('more formal, add a retro skill')
    expect(currentDesign()?.status).toBe('generating')
    await settle()
    const design = currentDesign()
    expect(design?.status).toBe('ready')
    expect(design?.name).toBe('Sentinel')
    const input = draftAgentMock.mock.calls[0]?.[0] as Record<string, unknown>
    // Refine = the describe call plus `current`: the live form values.
    expect(input.instruction).toBe('more formal, add a retro skill')
    expect(JSON.parse(input.current as string)).toMatchObject({ name: 'Scout', handle: 'scout', role: 'Research Analyst' })
    // The chat window carries the first turn — the [-8] slice the modal used.
    expect(input.chat).toEqual([
      { role: 'user', content: 'first purpose' },
      { role: 'assistant', content: JSON.stringify(DRAFT) },
    ])
  })

  it('records the refine receipt: what changed, against the form values the viewer had', async () => {
    await landed()
    draftAgentMock.mockImplementation(async () => OTHER)
    refineDesign('more formal')
    await settle()
    const receipt = currentDesign()?.lastChange
    expect(receipt).not.toBeNull()
    // role changed (Research Analyst → Watchman); soul and skills did not.
    expect(receipt?.fields.map((f) => f.label)).toContain('Role')
    expect(receipt?.soul.text).toBe('no text changes')
  })

  it('is a no-op with nothing designed or while a run is in flight', async () => {
    refineDesign('nothing to refine')
    expect(currentDesign()).toBeNull()
    expect(draftAgentMock).not.toHaveBeenCalled()

    const gate = pendingDraft()
    draftAgentMock.mockImplementation(() => gate.promise)
    startDesign('a purpose')
    refineDesign('refine attempt during initial run')
    expect(draftAgentMock).toHaveBeenCalledTimes(1)
    gate.resolve(DRAFT)
    await settle()
    expect(currentDesign()?.instruction).toBe('a purpose') // not the refine text
  })
})

describe('the landed-design toast: only when the modal is closed', () => {
  it('stays silent while the modal is open', async () => {
    setModalOpen(true)
    draftAgentMock.mockImplementation(async () => DRAFT)
    startDesign('a purpose')
    await settle()
    expect(currentDesign()?.status).toBe('ready')
    expect(pushToastMock).not.toHaveBeenCalled()
  })

  it('fires a success toast linking to the roster when the modal is closed', async () => {
    setModalOpen(false)
    draftAgentMock.mockImplementation(async () => DRAFT)
    startDesign('a purpose')
    await settle()
    expect(pushToastMock).toHaveBeenCalledTimes(1)
    expect(pushToastMock).toHaveBeenCalledWith({
      title: 'The agent design is ready',
      body: 'Scout',
      href: '/agents',
      tone: 'success',
    })
  })
})

describe('clearDesign: claiming the design', () => {
  it('drops the design, and a late landing does not resurrect it', async () => {
    const gate = pendingDraft()
    draftAgentMock.mockImplementation(() => gate.promise)
    startDesign('a purpose')
    clearDesign()
    expect(currentDesign()).toBeNull()
    gate.resolve(DRAFT)
    await settle()
    expect(currentDesign()).toBeNull()
  })
})

describe('ensureDesign: a manual start has somewhere to write', () => {
  // "Configure manually →" and Duplicate open the review step with nothing
  // designed. Its fields write through to the store, and with no record there
  // every keystroke was dropped: the form looked typed into while Create
  // stayed disabled. The review step writes through ensureDesign.
  it('seeds an empty, ready design when there is none, and the edits land in it', () => {
    expect(currentDesign()).toBeNull()
    const d = ensureDesign()
    expect(d.status).toBe('ready')
    expect([d.name, d.handle, d.department, d.role, d.soul]).toEqual(['', '', '', '', ''])
    expect(d.skills).toEqual([])
    expect(d.chat).toEqual([]) // no muse run behind it: the refine bar stays hidden
    d.name = 'Night Auditor'
    Object.assign(ensureDesign(), { handle: 'auditor', department: 'audit' })
    const read = currentDesign()
    expect(read?.id).toBe(d.id)
    expect([read?.name, read?.handle, read?.department]).toEqual(['Night Auditor', 'auditor', 'audit'])
  })

  it('writes to the design that exists, in any state, and never replaces it', async () => {
    adoptDesign({ name: 'Data Analyst', handle: 'dataanalyst', department: 'analytics', role: 'Analyst', soul: 'Reads numbers.' })
    const adopted = currentDesign()
    ensureDesign().department = 'analytics-2'
    expect(currentDesign()?.id).toBe(adopted?.id)
    expect(currentDesign()?.name).toBe('Data Analyst')
    expect(currentDesign()?.department).toBe('analytics-2')

    // An in-flight run is the record too: a hand edit does not start over.
    const gate = pendingDraft()
    draftAgentMock.mockImplementation(() => gate.promise)
    const id = startDesign('a purpose')
    expect(ensureDesign().id).toBe(id)
    expect(currentDesign()?.status).toBe('generating')
    gate.resolve(DRAFT)
    await settle()
    expect(currentDesign()?.id).toBe(id)
  })

  it('is cleared like any other design once the hire claims it', () => {
    ensureDesign().name = 'Scout'
    clearDesign()
    expect(currentDesign()).toBeNull()
  })
})
