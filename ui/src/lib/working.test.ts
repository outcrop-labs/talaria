import { QueryClient } from '@tanstack/svelte-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { type Working, workingOf, workingQuery } from '@/lib/working.svelte'

const jsonResponse = (status: number, body: unknown): Response =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } })

const fetchMock = vi.fn(async (_url: string | URL, _init?: RequestInit) => jsonResponse(200, {}))

beforeEach(() => {
  vi.stubGlobal('fetch', fetchMock)
  fetchMock.mockReset()
})
afterEach(() => vi.unstubAllGlobals())

const quiet: Working = { comms: null, work: null, boards: null }
const chatting: Working = {
  comms: { count: 1, label: 'Deploy notes', href: '/comms/agent/dex/c1' },
  work: null,
  boards: null,
}

async function readThrough(qc: QueryClient): Promise<Working | null> {
  await qc.fetchQuery({ ...workingQuery, retry: false }).catch(() => undefined)
  const state = qc.getQueryState<Working>(workingQuery.queryKey)
  return workingOf({ isError: state?.status === 'error', data: state?.data })
}

describe('the working read', () => {
  it('passes the server answer through', async () => {
    fetchMock.mockImplementationOnce(async () => jsonResponse(200, chatting))
    expect(await readThrough(new QueryClient())).toEqual(chatting)
    expect(String(fetchMock.mock.calls[0]?.[0])).toBe('/api/working')
  })

  it('reads a failed read as null, never as nothing working', async () => {
    fetchMock.mockImplementationOnce(async () => jsonResponse(500, { error: 'boom' }))
    const reading = await readThrough(new QueryClient())
    expect(reading).toBeNull()
    expect(reading).not.toEqual(quiet)
  })

  it('drops the last good reading when a refetch fails, so no glyph outlives its signal', async () => {
    const qc = new QueryClient()
    qc.setQueryData(workingQuery.queryKey, chatting)
    fetchMock.mockImplementationOnce(async () => jsonResponse(500, { error: 'boom' }))
    expect(await readThrough(qc)).toBeNull()
  })

  it('polls fast only while something is lit', () => {
    const interval = (data: Working | undefined) => workingQuery.refetchInterval({ state: { data } })
    expect(interval(chatting)).toBeLessThan(interval(quiet))
    expect(interval(undefined)).toBe(interval(quiet))
  })
})
