import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ChatEvent, ChatMeta } from '@/lib/chat'
import { queueChatMessage, streamChat } from '@/lib/chat'

// streamChat is where the `queued` ChatEvent actually comes from — the parser
// never produces it (see sse-parse.test.ts). Only `fetch` is stubbed; the SSE
// parsing underneath is the real thing.

const params = { model: 'claude-opus-5', content: 'hello' }

const sse = (body: string) =>
  new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream', 'X-Conversation-Id': 'c1', 'X-Message-Id': 'm1' } })

const jsonRes = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } })

const collect = async (gen: AsyncGenerator<ChatEvent>) => {
  const out: ChatEvent[] = []
  for await (const ev of gen) out.push(ev)
  return out
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('streamChat — the queued path', () => {
  it('yields a single queued event and reports the conversation id', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => jsonRes({ queued: true, conversationId: 'c9' })))
    const meta: ChatMeta[] = []
    expect(await collect(streamChat(params, (m) => meta.push(m)))).toEqual([{ type: 'queued' }])
    // messageId is empty: nothing is streaming, the message joined history.
    expect(meta).toEqual([{ conversationId: 'c9', messageId: '' }])
  })

  it('treats a queued reply with no conversation id as an unexpected response', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => jsonRes({ queued: true })))
    await expect(collect(streamChat(params))).rejects.toThrow(/unexpected chat response/)
  })

  it('surfaces a server error message from a JSON reply', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => jsonRes({ error: 'rate limited' }, 429)))
    await expect(collect(streamChat(params))).rejects.toThrow('rate limited')
  })

  it('falls back to the status code when the error body has no message', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => jsonRes({}, 500)))
    await expect(collect(streamChat(params))).rejects.toThrow('request failed (500)')
  })
})

describe('streamChat — the streaming path', () => {
  it('reports meta from the response headers, then parses the stream', async () => {
    const body =
      `data: ${JSON.stringify({ choices: [{ delta: { content: 'hi' } }] })}\n\n` +
      `data: ${JSON.stringify({ usage: { prompt_tokens: 3, completion_tokens: 1 } })}\n\n` +
      'data: [DONE]\n\n'
    vi.stubGlobal('fetch', vi.fn(async () => sse(body)))

    const meta: ChatMeta[] = []
    const events = await collect(streamChat(params, (m) => meta.push(m)))
    expect(meta).toEqual([{ conversationId: 'c1', messageId: 'm1' }])
    expect(events).toEqual([
      { type: 'content', text: 'hi' },
      { type: 'usage', promptTokens: 3, completionTokens: 1 },
    ])
  })

  it('posts the params as JSON with same-origin credentials', async () => {
    const fetchMock = vi.fn(async () => sse('data: [DONE]\n\n'))
    vi.stubGlobal('fetch', fetchMock)
    await collect(streamChat({ ...params, conversationId: 'c1', tier: 'deep' }))

    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toBe('/api/chat')
    expect(init.method).toBe('POST')
    expect(init.credentials).toBe('same-origin')
    expect(JSON.parse(init.body as string)).toEqual({ ...params, conversationId: 'c1', tier: 'deep' })
  })

  it('throws on a non-OK streaming response', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response('nope', { status: 502, headers: { 'content-type': 'text/plain' } })))
    await expect(collect(streamChat(params))).rejects.toThrow('request failed (502)')
  })
})

describe('queueChatMessage', () => {
  it('sends queue: true and resolves on success', async () => {
    // The route's real answer: 202 with a JSON body (never a bare 204 — the
    // door reads every reply, and an unparseable one is an error by contract).
    const fetchMock = vi.fn(async () => jsonRes({ queued: true, conversationId: 'c1' }, 202))
    vi.stubGlobal('fetch', fetchMock)
    await queueChatMessage({ model: 'm', conversationId: 'c1', content: 'later' })
    const [, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(JSON.parse(init.body as string)).toMatchObject({ queue: true, conversationId: 'c1' })
  })

  it('rejects with the server message on failure', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => jsonRes({ error: 'conversation is gone' }, 404)))
    await expect(queueChatMessage({ model: 'm', conversationId: 'c1', content: 'x' })).rejects.toThrow('conversation is gone')
  })
})
