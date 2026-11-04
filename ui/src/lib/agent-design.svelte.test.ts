// @vitest-environment runes
//
// The design store under Svelte's CLIENT transform — the one the browser
// runs. The rest of the store's suite (agent-design.test.ts) runs under the
// server transform, where `$state` is a plain object, so a run that wrote
// to the object it built rather than to the store's proxy passed every test
// there while the modal said "Designing" for good: the first read froze the
// proxy's signals at `generating`, and the server's answer, 20 s later,
// landed where nothing could see it. Each case below READS the design while
// it runs, the way the modal and the roster row do, before the run lands.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { flushSync } from 'svelte'
import type { AgentDraft } from './muse.svelte'

const draftAgentMock = vi.hoisted(() => vi.fn<(input: Record<string, unknown>) => Promise<AgentDraft>>())
const pushToastMock = vi.hoisted(() => vi.fn())
vi.mock('@/lib/muse.svelte', () => ({ draftAgent: draftAgentMock }))
vi.mock('@/lib/toast.svelte', () => ({ pushToast: pushToastMock }))

import { clearDesign, currentDesign, designGenerating, setModalOpen, startDesign } from './agent-design.svelte'

const DRAFT: AgentDraft = {
  name: 'Scout',
  handle: 'scout',
  department: 'research',
  role: 'Research Analyst',
  soul: 'A careful analyst.',
  skills: [{ name: 'weekly-digest', content: '# Weekly digest' }],
}

function pendingDraft() {
  let resolve!: (d: AgentDraft) => void
  let reject!: (e: Error) => void
  const promise = new Promise<AgentDraft>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

const settle = async () => {
  for (let i = 0; i < 25; i++) await Promise.resolve()
}

beforeEach(() => {
  draftAgentMock.mockReset()
  pushToastMock.mockClear()
  setModalOpen(false)
  clearDesign()
})
afterEach(() => {
  vi.useRealTimers()
  clearDesign()
})

describe('a design read while it runs (the client transform)', () => {
  it('lands as ready for a reader that looked while it generated', async () => {
    const gate = pendingDraft()
    draftAgentMock.mockImplementation(() => gate.promise)
    startDesign('A release manager that tracks deploy trains')
    // The modal's first read, while the run is out.
    expect(currentDesign()?.status).toBe('generating')
    expect(designGenerating()).toBe(true)

    gate.resolve(DRAFT)
    await settle()
    expect(currentDesign()?.status).toBe('ready')
    expect(currentDesign()?.name).toBe('Scout')
    expect(designGenerating()).toBe(false)
    // The modal is closed, so the landing says so.
    expect(pushToastMock).toHaveBeenCalledOnce()
  })

  it('moves a derived reader (the modal and the roster row) to ready', async () => {
    const gate = pendingDraft()
    draftAgentMock.mockImplementation(() => gate.promise)
    const seen: string[] = []
    const stop = $effect.root(() => {
      const status = $derived(currentDesign()?.status ?? 'none')
      $effect(() => {
        seen.push(status)
      })
    })
    flushSync()
    startDesign('a night auditor')
    flushSync()
    gate.resolve(DRAFT)
    await settle()
    flushSync()
    stop()
    expect(seen).toContain('generating')
    expect(seen.at(-1)).toBe('ready')
  })

  it('counts the elapsed seconds a reader sees', async () => {
    vi.useFakeTimers()
    const gate = pendingDraft()
    draftAgentMock.mockImplementation(() => gate.promise)
    startDesign('a support triager')
    expect(currentDesign()?.genSeconds).toBe(0)
    vi.advanceTimersByTime(3_000)
    expect(currentDesign()?.genSeconds).toBe(3)
    gate.resolve(DRAFT)
    await settle()
    expect(currentDesign()?.status).toBe('ready')
  })

  it('lands a failure for a reader that looked while it generated', async () => {
    const gate = pendingDraft()
    draftAgentMock.mockImplementation(() => gate.promise)
    startDesign('a vague purpose')
    expect(currentDesign()?.status).toBe('generating')
    gate.reject(new Error('Muse could not design an agent from that'))
    await settle()
    expect(currentDesign()?.status).toBe('error')
    expect(currentDesign()?.error).toBe('Muse could not design an agent from that')
  })
})
