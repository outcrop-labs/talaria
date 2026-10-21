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

  it('maps a failed tool onto the error status, under either wording', async () => {
    // Hermes sends `tool.failed`; the status field spells it either way. The
    // row's third face is the whole point: a failure that arrived as
    // `undefined` left the tool row spinning forever.
    for (const word of ['error', 'failed', 'FAILED']) {
      const payload = JSON.stringify({ tool: 'web_search', label: 'Search failed', toolCallId: 'c3', status: word })
      expect(await drain([`event: hermes.tool.progress\ndata: ${payload}\n\n`])).toEqual([
        { type: 'tool', id: 'c3', name: 'web_search', label: 'Search failed', status: 'error' },
      ])
    }
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

describe('parseAgentStream — raw Hermes session frames', () => {
  // A turn pinned to a Hermes session is forwarded byte for byte: the tool
  // rows arrive as the session's OWN frames, flat payload and all, not as the
  // `hermes.tool.progress` envelope. Before these three event names were read
  // a session turn ran its whole tool loop with no row on screen.
  const session = (event: string, payload: Record<string, unknown>) =>
    `event: ${event}\ndata: ${JSON.stringify(payload)}\n\n`
  const rows = (events: ChatEvent[]) =>
    events.flatMap((e) => (e.type === 'tool' ? [{ id: e.id, name: e.name, label: e.label, status: e.status }] : []))

  it('maps tool.started / tool.completed / tool.failed onto the three statuses', async () => {
    expect(
      rows(
        await drain([
          session('tool.started', { message_id: 'm1', tool_name: 'read_file', preview: 'docs/plan.md' }),
          session('tool.completed', { message_id: 'm1', tool_name: 'read_file', preview: '' }),
          session('tool.started', { message_id: 'm1', tool_name: 'web_search', preview: 'retry budgets' }),
          session('tool.failed', { message_id: 'm1', tool_name: 'web_search', preview: 'the search timed out' }),
        ]),
      ),
    ).toEqual([
      { id: 'm1:read_file#1', name: 'read_file', label: 'docs/plan.md', status: 'running' },
      { id: 'm1:read_file#1', name: 'read_file', label: '', status: 'completed' },
      { id: 'm1:web_search#1', name: 'web_search', label: 'retry budgets', status: 'running' },
      { id: 'm1:web_search#1', name: 'web_search', label: 'the search timed out', status: 'error' },
    ])
  })

  it('builds the id the gateway persists: <message_id>:<tool_name>#<nth started>', async () => {
    // `session_tool` in fleet_chat.rs counts `tool.started` per name within a
    // message; the parser mirrors it with a per-stream counter so the live row
    // and the row that comes back from history are ONE row.
    expect(
      rows(
        await drain([
          session('tool.started', { message_id: 'm2', tool_name: 'read_file', preview: 'a.md' }),
          session('tool.completed', { message_id: 'm2', tool_name: 'read_file', preview: '' }),
          session('tool.started', { message_id: 'm2', tool_name: 'read_file', preview: 'b.md' }),
          session('tool.completed', { message_id: 'm2', tool_name: 'read_file', preview: '' }),
        ]),
      ).map((r) => r.id),
    ).toEqual(['m2:read_file#1', 'm2:read_file#1', 'm2:read_file#2', 'm2:read_file#2'])
  })

  it('counts per message, so a second message starts over at #1', async () => {
    expect(
      rows(
        await drain([
          session('tool.started', { message_id: 'm3', tool_name: 'read_file', preview: 'a.md' }),
          session('tool.started', { message_id: 'm4', tool_name: 'read_file', preview: 'b.md' }),
        ]),
      ).map((r) => r.id),
    ).toEqual(['m3:read_file#1', 'm4:read_file#1'])
  })

  it('opens #1 for a completion whose start never arrived', async () => {
    // The Rust bumps the counter when it is still 0 for exactly this: a frame
    // addressing a call nobody saw start still gets the first slot rather than
    // an id ending `#0` that nothing else will ever match.
    expect(rows(await drain([session('tool.completed', { message_id: 'm5', tool_name: 'read_file' })]))).toEqual([
      { id: 'm5:read_file#1', name: 'read_file', label: '', status: 'completed' },
    ])
  })

  it('prefers an explicit tool_call_id, and defaults the name when there is none', async () => {
    expect(
      rows(await drain([session('tool.started', { message_id: 'm6', tool_call_id: 'call_abc', preview: 'x' })])),
    ).toEqual([{ id: 'call_abc', name: 'tool', label: 'x', status: 'running' }])
  })

  it('still reports the row when the frame names no call at all', async () => {
    // No id to dedupe on, but the status is still news: `mergeTool` matches it
    // onto the running row of the same name.
    expect(rows(await drain([session('tool.completed', { tool_name: 'read_file' })]))).toEqual([
      { id: undefined, name: 'read_file', label: '', status: 'completed' },
    ])
  })

  it('keeps a long preview as the label, exactly as the gateway persists it', async () => {
    // How much of it the tool list shows is the row's call (toolRowLabel),
    // so the live row and the stored row carry the same words.
    const long = 'x'.repeat(400)
    expect(
      rows(await drain([session('tool.started', { message_id: 'm7', tool_name: 'bash', preview: long })])),
    ).toEqual([{ id: 'm7:bash#1', name: 'bash', label: long, status: 'running' }])
  })

  it('folds into two rows, one done and one failed, and the failure sticks', async () => {
    let tools: ToolCall[] = []
    const events = await drain([
      session('tool.started', { message_id: 'm8', tool_name: 'read_file', preview: 'a.md' }),
      session('tool.completed', { message_id: 'm8', tool_name: 'read_file', preview: '' }),
      session('tool.started', { message_id: 'm8', tool_name: 'read_file', preview: 'b.md' }),
      session('tool.failed', { message_id: 'm8', tool_name: 'read_file', preview: 'no such file' }),
      // A stray late frame for the failed call: terminal is terminal.
      session('tool.completed', { message_id: 'm8', tool_name: 'read_file', preview: '' }),
    ])
    for (const ev of events) if (ev.type === 'tool') tools = mergeTool(tools, ev)
    expect(tools).toEqual([
      { id: 'm8:read_file#1', name: 'read_file', label: 'a.md', status: 'completed' },
      { id: 'm8:read_file#2', name: 'read_file', label: 'no such file', status: 'error' },
    ])
  })

  it('ignores a session frame that is not JSON', async () => {
    expect(await drain([`event: tool.started\ndata: {nope\n\n`, delta('on we go')])).toEqual([
      { type: 'content', text: 'on we go' },
    ])
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

  it('flips the status to error and keeps it there', () => {
    const failed = mergeTool([running()], { type: 'tool', id: 't1', name: 'search', label: 'Search failed', status: 'error' })
    expect(failed).toEqual([{ id: 't1', name: 'search', label: 'Search failed', status: 'error' }])
    // Terminal, exactly like completed: a stray later frame — a retry's
    // progress, a duplicated id — must not hide the failure behind a spinner
    // that never resolves.
    const late = mergeTool(failed, { type: 'tool', id: 't1', name: 'search', label: 'Searching', status: 'running' })
    expect(late[0]!.status).toBe('error')
    expect(late).toHaveLength(1)
  })

  it('does not let a late running frame un-finish a completed tool either', () => {
    const done = mergeTool([running()], { type: 'tool', id: 't1', name: 'search', label: 'Searched', status: 'completed' })
    expect(mergeTool(done, { type: 'tool', id: 't1', name: 'search', label: 'Searching', status: 'running' })[0]!.status).toBe(
      'completed',
    )
  })

  it('does not mutate the array it was given', () => {
    const before: ToolCall[] = [running()]
    const snapshot = structuredClone(before)
    mergeTool(before, { type: 'tool', id: 't1', name: 'search', label: 'x', status: 'completed' })
    expect(before).toEqual(snapshot)
  })
})
