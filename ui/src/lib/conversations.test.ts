import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { loadConversation, openConversation } from '@/lib/conversations.svelte'

// The pane's first read of a thread. The server answers 404 both for a thread
// that is gone and for one the viewer may not read, and the pane has to say
// so rather than open it as an empty thread: that 404 is the one answer the
// first read must not swallow. Every other failure stays non-throwing.

const jsonResponse = (status: number, body: unknown): Response =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } })

const fetchMock = vi.fn(async (_url: string | URL, _init?: RequestInit) => jsonResponse(200, {}))

beforeEach(() => {
  vi.stubGlobal('fetch', fetchMock)
  fetchMock.mockReset()
})
afterEach(() => vi.unstubAllGlobals())

const detail = { conversation: { id: 'c1', agentModel: 'dex', title: null }, messages: [] }

describe('openConversation', () => {
  it('reads the thread the server lets this viewer read', async () => {
    fetchMock.mockImplementationOnce(async () => jsonResponse(200, detail))
    await expect(openConversation('c1')).resolves.toEqual(detail)
    expect(String(fetchMock.mock.calls[0]?.[0])).toBe('/api/conversations/c1')
  })

  it('tells a 404 (gone, or not yours to read) apart', async () => {
    fetchMock.mockImplementationOnce(async () => jsonResponse(404, { error: 'not found' }))
    await expect(openConversation('c1')).resolves.toBe('missing')
  })

  it('reads every other failure as nothing to show, and never rejects', async () => {
    fetchMock.mockImplementationOnce(async () => jsonResponse(500, { error: 'boom' }))
    await expect(openConversation('c1')).resolves.toBeNull()
    fetchMock.mockImplementationOnce(async () => jsonResponse(403, { error: 'no' }))
    await expect(openConversation('c1')).resolves.toBeNull()
    fetchMock.mockImplementationOnce(async () => {
      throw new TypeError('Failed to fetch')
    })
    await expect(openConversation('c1')).resolves.toBeNull()
  })

  it('leaves loadConversation, the pollers’ read, as it was: a 404 is null there', async () => {
    fetchMock.mockImplementationOnce(async () => jsonResponse(404, { error: 'not found' }))
    await expect(loadConversation('c1')).resolves.toBeNull()
  })
})
