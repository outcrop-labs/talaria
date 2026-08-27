import { describe, expect, it } from 'vitest'
import { mergeTool, parseAgentStream, sseFrames, type ChatEvent, type ToolCall } from '@/lib/sse-parse'

// A ReadableStream we can interrogate afterwards: `cancelled` tells us the
// underlying source was torn down, and `stream.locked` tells us the reader was
// released. Together they're the observable half of parseAgentStream's finally.
function makeStream(chunks: Array<string | Uint8Array>) {
  const enc = new TextEncoder()
  const state = { cancelled: false, enqueued: 0 }
  let i = 0
  const stream = new ReadableStream<Uint8Array>({
    pull(controller) {
      if (i >= chunks.length) return controller.close()
      const c = chunks[i++]!
      state.enqueued++
      controller.enqueue(typeof c === 'string' ? enc.encode(c) : c)
    },
    cancel() {
      state.cancelled = true
    },
  })
  return { stream, state }
}

const drain = async (chunks: Array<string | Uint8Array>): Promise<ChatEvent[]> => {
  const out: ChatEvent[] = []
  for await (const ev of parseAgentStream(makeStream(chunks).stream)) out.push(ev)
  return out
}

const delta = (content: string) => `data: ${JSON.stringify({ choices: [{ delta: { content } }] })}\n\n`

describe('parseAgentStream — framing', () => {
  it('emits content deltas in order', async () => {
    expect(await drain([delta('Hello'), delta(' world')])).toEqual([
      { type: 'content', text: 'Hello' },
      { type: 'content', text: ' world' },
    ])
  })

  it('reassembles a frame split across chunks, including mid-"\\n\\n"', async () => {
    const frame = delta('Hello world') // ends with \n\n
    // Split so the first chunk ends on the FIRST newline of the separator: the
    // parser must not treat a lone \n as a frame boundary, and must keep the
    // partial frame buffered until the second \n arrives.
    const cut = frame.length - 1
    const events = await drain([frame.slice(0, cut), frame.slice(cut)])
    expect(events).toEqual([{ type: 'content', text: 'Hello world' }])
  })

  it('reassembles a frame split mid-JSON across three chunks', async () => {
    const frame = delta('the quick brown fox')
    const a = frame.slice(0, 20)
    const b = frame.slice(20, 34)
    const c = frame.slice(34)
    expect(await drain([a, b, c])).toEqual([{ type: 'content', text: 'the quick brown fox' }])
  })

  it('reassembles a multi-byte UTF-8 character split across chunks', async () => {
    // TextDecoder is created with { stream: true } precisely for this; a naive
    // per-chunk decode would produce U+FFFD and the JSON.parse would then fail.
    const bytes = new TextEncoder().encode(delta('héllo 🎉'))
    const emoji = bytes.length - 6 // lands inside the 4-byte 🎉 sequence
    expect(await drain([bytes.slice(0, emoji), bytes.slice(emoji)])).toEqual([
      { type: 'content', text: 'héllo 🎉' },
    ])
  })

  it('handles several frames arriving in a single chunk', async () => {
    expect(await drain([delta('a') + delta('b') + delta('c')])).toEqual([
      { type: 'content', text: 'a' },
      { type: 'content', text: 'b' },
      { type: 'content', text: 'c' },
    ])
  })

  it('drops an unterminated trailing frame rather than emitting a partial parse', async () => {
    expect(await drain([delta('kept'), 'data: {"choices":[{"delta":{"content":"lost"'])).toEqual([
      { type: 'content', text: 'kept' },
    ])
  })
})

describe('parseAgentStream — event kinds', () => {
  it('yields reasoning from either delta field, and prefers content when both are present', async () => {
    expect(await drain([`data: ${JSON.stringify({ choices: [{ delta: { reasoning: 'thinking' } }] })}\n\n`])).toEqual([
      { type: 'reasoning', text: 'thinking' },
    ])
    expect(
      await drain([`data: ${JSON.stringify({ choices: [{ delta: { reasoning_content: 'thinking' } }] })}\n\n`]),
    ).toEqual([{ type: 'reasoning', text: 'thinking' }])
    expect(
      await drain([`data: ${JSON.stringify({ choices: [{ delta: { content: 'said', reasoning: 'thought' } }] })}\n\n`]),
    ).toEqual([{ type: 'content', text: 'said' }])
  })

  it('yields a usage event from the final chunk, filling absent counts with 0', async () => {
    const body = `data: ${JSON.stringify({ choices: [], usage: { prompt_tokens: 120, completion_tokens: 34 } })}\n\n`
    expect(await drain([body])).toEqual([{ type: 'usage', promptTokens: 120, completionTokens: 34 }])

    const partial = `data: ${JSON.stringify({ usage: { prompt_tokens: 7 } })}\n\n`
    expect(await drain([partial])).toEqual([{ type: 'usage', promptTokens: 7, completionTokens: 0 }])
  })

  it('does not yield usage when both counts are zero or absent', async () => {
    expect(await drain([`data: ${JSON.stringify({ usage: { prompt_tokens: 0, completion_tokens: 0 } })}\n\n`])).toEqual([])
    expect(await drain([`data: ${JSON.stringify({ usage: {} })}\n\n`])).toEqual([])
  })

  it('yields content AND usage from one frame that carries both', async () => {
    const both = `data: ${JSON.stringify({
      choices: [{ delta: { content: 'done' } }],
      usage: { prompt_tokens: 1, completion_tokens: 2 },
    })}\n\n`
    expect(await drain([both])).toEqual([
      { type: 'content', text: 'done' },
      { type: 'usage', promptTokens: 1, completionTokens: 2 },
    ])
  })

  it('parses tool progress under both the hermes and claude event names', async () => {
    const payload = JSON.stringify({ tool: 'web_search', emoji: '🔎', label: 'Searching', toolCallId: 'c1', status: 'running' })
    for (const name of ['hermes.tool.progress', 'claude.tool.progress']) {
      expect(await drain([`event: ${name}\ndata: ${payload}\n\n`])).toEqual([
        { type: 'tool', id: 'c1', name: 'web_search', label: '🔎 Searching', status: 'running' },
      ])
    }
  })

  it('accepts the snake_case id field and treats "complete" as completed', async () => {
    const payload = JSON.stringify({ name: 'fetch', label: 'Fetched', tool_call_id: 'c2', status: 'COMPLETE' })
    expect(await drain([`event: hermes.tool.progress\ndata: ${payload}\n\n`])).toEqual([
      { type: 'tool', id: 'c2', name: 'fetch', label: 'Fetched', status: 'completed' },
    ])
  })

  it('leaves status undefined for an unrecognised value and defaults the name to "tool"', async () => {
    const payload = JSON.stringify({ label: 'Doing a thing', status: 'weird' })
    expect(await drain([`event: hermes.tool.progress\ndata: ${payload}\n\n`])).toEqual([
      { type: 'tool', id: undefined, name: 'tool', label: 'Doing a thing', status: undefined },
    ])
  })

  it('drops a progress frame carrying neither a label nor an id', async () => {
    const payload = JSON.stringify({ tool: 'noop', status: 'running' })
    expect(await drain([`event: hermes.tool.progress\ndata: ${payload}\n\n`])).toEqual([])
  })

  it('never yields a "queued" event — that one comes from streamChat, not the parser', async () => {
    const events = await drain([`event: queued\ndata: ${JSON.stringify({ queued: true })}\n\n`, delta('hi')])
    expect(events.some((e) => e.type === 'queued')).toBe(false)
    expect(events).toEqual([{ type: 'content', text: 'hi' }])
  })
})

describe('parseAgentStream — malformed and noise input', () => {
  it('skips malformed JSON without throwing, and keeps parsing after it', async () => {
    expect(await drain(['data: {not json at all}\n\n', delta('still here')])).toEqual([
      { type: 'content', text: 'still here' },
    ])
  })

  it('skips a malformed tool-progress payload', async () => {
    expect(await drain(['event: hermes.tool.progress\ndata: {oops\n\n', delta('ok')])).toEqual([
      { type: 'content', text: 'ok' },
    ])
  })

  it('ignores [DONE], empty data lines, and comment keep-alives', async () => {
    expect(await drain([': keep-alive\n\n', 'data:\n\n', 'data: [DONE]\n\n', delta('x')])).toEqual([
      { type: 'content', text: 'x' },
    ])
  })

  it('joins a frame whose data spans multiple data: lines into ONE payload — the EventSource rule', async () => {
    // sseFrames unified the two consumers on the spec behavior: the lines of a
    // frame concatenate with \n. The split below lands between JSON tokens,
    // where that \n is legal whitespace — so the joined frame PARSES, while
    // under the old per-line reading each half was invalid JSON and dropped.
    // (Splitting mid-string, e.g. inside "content", stays unparseable — a
    // literal newline in a JSON string is invalid, join or no join.)
    const body = 'data: {"choices":[\ndata: {"delta":{"content":"hello"}}]}\n\n'
    expect(await drain([body])).toEqual([{ type: 'content', text: 'hello' }])
  })

  it('yields nothing for an empty stream', async () => {
    expect(await drain([])).toEqual([])
  })
})

describe('parseAgentStream — reader cleanup (the try/finally)', () => {
  it('cancels the body and releases the reader when the consumer breaks early', async () => {
    // The regression this guards: chat-view breaks out of the for-await on the
    // first `queued` event. Without the finally, the reader is never released
    // and the underlying fetch stays open for the life of the page.
    const { stream, state } = makeStream([delta('first'), delta('second'), delta('third')])

    for await (const ev of parseAgentStream(stream)) {
      expect(ev).toEqual({ type: 'content', text: 'first' })
      break
    }

    expect(state.cancelled).toBe(true) // body torn down → fetch aborted
    expect(stream.locked).toBe(false) // reader released
    expect(state.enqueued).toBeLessThan(3) // and we stopped pulling
  })

  it('releases the reader when the consumer throws', async () => {
    const { stream, state } = makeStream([delta('first'), delta('second')])
    const boom = new Error('consumer exploded')

    await expect(
      (async () => {
        for await (const _ev of parseAgentStream(stream)) throw boom
      })(),
    ).rejects.toBe(boom)

    expect(state.cancelled).toBe(true)
    expect(stream.locked).toBe(false)
  })

  it('releases the reader on a normal end-of-stream too', async () => {
    const { stream } = makeStream([delta('a'), delta('b')])
    for await (const _ev of parseAgentStream(stream)) {
      /* drain */
    }
    expect(stream.locked).toBe(false)
  })

  it('does not surface a cancel() failure to the consumer', async () => {
    // reader.cancel() is wrapped in .catch(() => {}) — a source that rejects on
    // cancel must not turn a clean early exit into a thrown error.
    const stream = new ReadableStream<Uint8Array>({
      pull(controller) {
        controller.enqueue(new TextEncoder().encode(delta('one')))
      },
      cancel() {
        throw new Error('cancel failed')
      },
    })
    await expect(
      (async () => {
        for await (const _ev of parseAgentStream(stream)) break
      })(),
    ).resolves.toBeUndefined()
    expect(stream.locked).toBe(false)
  })
})

describe('sseFrames — the shared frame reader', () => {
  it('yields event names and joined data payloads, in order', async () => {
    const out: Array<{ event: string; data: string }> = []
    for await (const f of sseFrames(makeStream(['event: one\ndata: a\n\n', 'data: b\n\nevent: two\ndata: c\n\n']).stream)) out.push(f)
    expect(out).toEqual([
      { event: 'one', data: 'a' },
      { event: '', data: 'b' },
      { event: 'two', data: 'c' },
    ])
  })

  it('joins multi-line data with \\n and drops [DONE]/empty only at the consumer', async () => {
    const out: string[] = []
    for await (const f of sseFrames(makeStream(['data: first\ndata: second\n\n', 'data: [DONE]\n\n', 'data:\n\n']).stream)) out.push(f.data)
    // The reader frames; payload semantics (skip [DONE], skip empties) belong
    // to each consumer — that is why it yields them untouched.
    expect(out).toEqual(['first\nsecond', '[DONE]', ''])
  })

  it('releases the reader on an early consumer exit — the discipline every stream inherits', async () => {
    const { stream, state } = makeStream(['data: a\n\n', 'data: b\n\n'])
    for await (const _f of sseFrames(stream)) break
    expect(state.cancelled).toBe(true)
    expect(stream.locked).toBe(false)
  })
})

describe('mergeTool', () => {
  const running = (over: Partial<ToolCall> = {}): ToolCall => ({ id: 't1', name: 'search', label: 'Searching', status: 'running', ...over })

  it('appends an unseen tool, defaulting status to running', () => {
    expect(mergeTool([], { type: 'tool', id: 't1', name: 'search', label: 'Searching' })).toEqual([running()])
  })

  it('merges by id and flips the status to completed', () => {
    const out = mergeTool([running()], { type: 'tool', id: 't1', name: 'search', label: 'Searched', status: 'completed' })
    expect(out).toEqual([{ id: 't1', name: 'search', label: 'Searched', status: 'completed' }])
  })

  it('does not overwrite a good label with the empty one a completion frame carries', () => {
    const out = mergeTool([running()], { type: 'tool', id: 't1', name: 'search', label: '', status: 'completed' })
    expect(out[0]).toEqual({ id: 't1', name: 'search', label: 'Searching', status: 'completed' })
  })

  it('keeps the existing status when the event carries none', () => {
    const out = mergeTool([running({ status: 'completed' })], { type: 'tool', id: 't1', name: 'search', label: 'x' })
    expect(out[0]!.status).toBe('completed')
  })

  it('falls back to name+running matching when the event has no id', () => {
    const out = mergeTool([running({ id: undefined })], { type: 'tool', name: 'search', label: 'Done', status: 'completed' })
    expect(out).toHaveLength(1)
    expect(out[0]!.status).toBe('completed')
  })

  it('does not merge an id-less event into an already-completed tool of the same name', () => {
    const done: ToolCall = { id: undefined, name: 'search', label: 'Searched', status: 'completed' }
    const out = mergeTool([done], { type: 'tool', name: 'search', label: 'Searching', status: 'running' })
    expect(out).toHaveLength(2)
  })

  it('keeps two concurrent calls of the same tool apart by id', () => {
    let tools: ToolCall[] = []
    tools = mergeTool(tools, { type: 'tool', id: 'a', name: 'search', label: 'A', status: 'running' })
    tools = mergeTool(tools, { type: 'tool', id: 'b', name: 'search', label: 'B', status: 'running' })
    tools = mergeTool(tools, { type: 'tool', id: 'a', name: 'search', label: 'A', status: 'completed' })
    expect(tools.map((t) => [t.id, t.status])).toEqual([
      ['a', 'completed'],
      ['b', 'running'],
    ])
  })

  it('does not mutate the array it was given', () => {
    const before: ToolCall[] = [running()]
    const snapshot = structuredClone(before)
    mergeTool(before, { type: 'tool', id: 't1', name: 'search', label: 'x', status: 'completed' })
    expect(before).toEqual(snapshot)
  })
})
