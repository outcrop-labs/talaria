// Shared parser for the agent SSE stream (OpenAI deltas + hermes.tool.progress
// events). Used by the client (to render) AND the server (to persist durably).
// Pure Web-stream APIs, so it runs in the browser and in the Node server runtime.

export interface ToolCall {
  id?: string
  name: string
  label: string
  status: 'running' | 'completed'
}

export type ChatEvent =
  | { type: 'content'; text: string }
  | { type: 'reasoning'; text: string }
  | { type: 'tool'; id?: string; name: string; label: string; status?: 'running' | 'completed' }
  | { type: 'usage'; promptTokens: number; completionTokens: number }
  // Not produced by the parser below — `streamChat` yields it when the server
  // answers with JSON instead of a stream because the reply joined history
  // rather than streaming. It belongs in the union anyway: this is the type
  // every chat consumer switches on, and chat-view already handles it. Leaving
  // it out made streamChat's inferred return type wider than its own declared
  // event type, which nothing caught until a test tried to annotate it.
  | { type: 'queued' }

function parseToolProgress(payload: string): Extract<ChatEvent, { type: 'tool' }> | null {
  try {
    const r = JSON.parse(payload) as Record<string, unknown>
    const str = (v: unknown) => (typeof v === 'string' ? v : '')
    const name = str(r.tool) || str(r.name) || 'tool'
    const label = [str(r.emoji), str(r.label)].filter(Boolean).join(' ').trim()
    const id = str(r.toolCallId) || str(r.tool_call_id) || undefined
    const s = str(r.status).toLowerCase()
    const status = s === 'running' ? 'running' : s === 'completed' || s === 'complete' ? 'completed' : undefined
    if (!label && !id) return null
    // label may be empty — a later "completed" frame carries none; display falls
    // back to `name`, and mergers must not overwrite a good label with an empty one.
    return { type: 'tool', id, name, label, status }
  } catch {
    return null
  }
}

/** Parse a raw agent SSE body into typed chat events. */
export async function* parseAgentStream(body: ReadableStream<Uint8Array>): AsyncGenerator<ChatEvent> {
  const reader = body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''

  // Every exit path — normal end, `break` in the caller's for-await, a throw
  // from the caller's loop body, an aborted fetch — must release the reader.
  // Without this a consumer that stops early (chat-view breaks on `queued`)
  // leaves the lock held and the HTTP connection open forever: one leaked
  // socket per early exit, and the browser's per-host cap eventually stalls
  // every other request on the page.
  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      buffer += decoder.decode(value, { stream: true })

      let sep: number
      while ((sep = buffer.indexOf('\n\n')) !== -1) {
        const frame = buffer.slice(0, sep)
        buffer = buffer.slice(sep + 2)

        let eventName = ''
        const dataLines: string[] = []
        for (const line of frame.split('\n')) {
          const t = line.trim()
          if (t.startsWith('event:')) eventName = t.slice(6).trim()
          else if (t.startsWith('data:')) dataLines.push(t.slice(5).trim())
        }

        for (const data of dataLines) {
          if (!data || data === '[DONE]') continue
          if (eventName === 'hermes.tool.progress' || eventName === 'claude.tool.progress') {
            const tool = parseToolProgress(data)
            if (tool) yield tool
            continue
          }
          try {
            const json = JSON.parse(data) as {
              choices?: Array<{ delta?: { content?: string; reasoning?: string; reasoning_content?: string } }>
              usage?: { prompt_tokens?: number; completion_tokens?: number }
            }
            const d = json.choices?.[0]?.delta
            if (d?.content) yield { type: 'content', text: d.content }
            else if (d?.reasoning || d?.reasoning_content) {
              yield { type: 'reasoning', text: d.reasoning || d.reasoning_content || '' }
            }
            // Final chunk carries usage when stream_options.include_usage is honoured.
            if (json.usage && (json.usage.prompt_tokens || json.usage.completion_tokens)) {
              yield {
                type: 'usage',
                promptTokens: json.usage.prompt_tokens ?? 0,
                completionTokens: json.usage.completion_tokens ?? 0,
              }
            }
          } catch {
            /* keep-alive / partial frame */
          }
        }
      }
    }
  } finally {
    // Swallow: on an errored/aborted stream cancel() rejects with the stored
    // error, which would otherwise mask the real failure the caller is handling.
    await reader.cancel().catch(() => {})
    // cancel() closes the stream but does NOT release the lock — that is two
    // separate operations in the spec. Leaving it locked means nothing can ever
    // read the body again, which matters when a caller retries against a
    // response it still holds.
    reader.releaseLock()
  }
}

/** Fold a tool event into a running tool list (dedupe by id, else name+running). */
export function mergeTool(tools: ToolCall[], ev: Extract<ChatEvent, { type: 'tool' }>): ToolCall[] {
  const copy = tools.slice()
  const idx = ev.id
    ? copy.findIndex((t) => t.id === ev.id)
    : copy.findIndex((t) => t.name === ev.name && t.status === 'running')
  if (idx >= 0) {
    const existing = copy[idx]!
    copy[idx] = { ...existing, label: ev.label || existing.label, status: ev.status ?? existing.status }
  } else {
    copy.push({ id: ev.id, name: ev.name, label: ev.label, status: ev.status ?? 'running' })
  }
  return copy
}
