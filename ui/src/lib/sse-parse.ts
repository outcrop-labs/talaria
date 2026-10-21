// Shared parser for the agent SSE stream (OpenAI deltas, hermes.tool.progress
// events, and the raw Hermes session frames a pinned SESSION turn forwards).
// Used by the client (to render) AND the server (to persist durably).
// Pure Web-stream APIs, so it runs in the browser and in the Node server runtime.

/** A tool row's life: it starts `running` and ends `completed` or `error`.
 *  Both endings are TERMINAL — Hermes sends `tool.failed` for a tool that
 *  threw, and a stray later frame must never walk the row back to running. */
export type ToolStatus = 'running' | 'completed' | 'error'

export interface ToolCall {
  id?: string
  name: string
  label: string
  status: ToolStatus
}

const terminal = (s: ToolStatus | undefined): boolean => s === 'completed' || s === 'error'

/** A JSON field read as a string, or '' for anything else. Every payload
 *  below is whatever a provider or a session actually sent. */
const str = (v: unknown) => (typeof v === 'string' ? v : '')

export type ChatEvent =
  | { type: 'content'; text: string }
  | { type: 'reasoning'; text: string }
  | { type: 'tool'; id?: string; name: string; label: string; status?: ToolStatus }
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
    const name = str(r.tool) || str(r.name) || 'tool'
    const label = [str(r.emoji), str(r.label)].filter(Boolean).join(' ').trim()
    const id = str(r.toolCallId) || str(r.tool_call_id) || undefined
    const s = str(r.status).toLowerCase()
    const status: ToolStatus | undefined =
      s === 'running'
        ? 'running'
        : s === 'completed' || s === 'complete'
          ? 'completed'
          : s === 'error' || s === 'failed'
            ? 'error'
            : undefined
    if (!label && !id) return null
    // label may be empty — a later "completed" frame carries none; display falls
    // back to `name`, and mergers must not overwrite a good label with an empty one.
    return { type: 'tool', id, name, label, status }
  } catch {
    return null
  }
}

// THE RAW HERMES SESSION FRAMES. A turn pinned to a Hermes session is not
// re-shaped by the gateway: it forwards the session's own frames byte for
// byte, so the tool rows arrive under these three event names with a flat
// payload — `{message_id, tool_name, tool_call_id?, preview, args?}` — and
// nothing like the `hermes.tool.progress` envelope above. Until this branch
// existed a session turn ran its whole tool loop with NO row on screen: the
// calls only appeared once the turn settled and the persisted rows loaded.
const SESSION_TOOL = new Map<string, ToolStatus>([
  ['tool.started', 'running'],
  ['tool.completed', 'completed'],
  // A failure is its own terminal word, here as everywhere else: a row left
  // `running` after `tool.failed` promises a result nothing is coming with.
  ['tool.failed', 'error'],
])


function parseSessionTool(
  eventName: string,
  payload: string,
  calls: Map<string, number>,
): Extract<ChatEvent, { type: 'tool' }> | null {
  const status = SESSION_TOOL.get(eventName)
  if (!status) return null
  try {
    const r = JSON.parse(payload) as Record<string, unknown>
    const name = str(r.tool_name) || 'tool'
    const preview = str(r.preview).trim()
    return {
      type: 'tool',
      id: sessionToolId(r, name, eventName === 'tool.started', calls),
      name,
      // The row's words are the tool name; the preview rides as the label
      // in full, exactly as the gateway persists it, so the live row and the
      // stored row are the same row. How much of it the tool list shows is
      // the row's decision (`toolRowLabel`), not the parser's.
      label: preview,
      status,
    }
  } catch {
    return null
  }
}

/** The id the GATEWAY will persist for this same call, recomputed here so the
 *  live row and the row that comes back from history are one row and not two.
 *
 *  Mirrors `session_tool` in `api/crates/talaria-gateway/src/fleet_chat.rs`:
 *  an explicit `tool_call_id` (or `id`) wins; failing that the id is
 *  `<message_id>:<tool_name>#<n>`, where `n` is the call's ordinal within its
 *  message — a `tool.started` opens the next one, and anything else addresses
 *  the one already open (opening `#1` itself, for a completion whose start
 *  never arrived). Hence the counter, and hence it is per STREAM: it counts
 *  what the gateway counted over the same frames. */
function sessionToolId(
  r: Record<string, unknown>,
  name: string,
  started: boolean,
  calls: Map<string, number>,
): string | undefined {
  const explicit = str(r.tool_call_id) || str(r.id)
  if (explicit) return explicit
  const message = str(r.message_id)
  if (!message) return undefined
  const key = `${message}:${name}`
  const prior = calls.get(key) ?? 0
  const n = started || prior === 0 ? prior + 1 : prior
  calls.set(key, n)
  return `${key}#${n}`
}

/** Split a raw SSE body into frames (`event:` name + joined `data:` payload).
 *
 *  THE READER DISCIPLINE LIVES HERE, ONCE. Every exit path — normal end,
 *  `break` in the caller's for-await, a throw from the caller's loop body, an
 *  aborted fetch — must cancel AND release the reader. Without this a consumer
 *  that stops early (chat-view breaks on `queued`) leaves the lock held and
 *  the HTTP connection open forever: one leaked socket per early exit, and the
 *  browser's per-host cap eventually stalls every other request on the page. */
export async function* sseFrames(body: ReadableStream<Uint8Array>): AsyncGenerator<{ event: string; data: string }> {
  const reader = body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
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
        yield { event: eventName, data: dataLines.join('\n') }
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

/** Parse a raw agent SSE body into typed chat events. */
export async function* parseAgentStream(body: ReadableStream<Uint8Array>): AsyncGenerator<ChatEvent> {
  // One counter per stream: the ordinal in a session tool id counts the calls
  // within a message as the frames go past, which is exactly what the gateway
  // does with the same frames on its way to the persisted row.
  const calls = new Map<string, number>()
  for await (const { event: eventName, data } of sseFrames(body)) {
    if (!data || data === '[DONE]') continue
    if (eventName === 'hermes.tool.progress' || eventName === 'claude.tool.progress') {
      const tool = parseToolProgress(data)
      if (tool) yield tool
      continue
    }
    if (SESSION_TOOL.has(eventName)) {
      const tool = parseSessionTool(eventName, data, calls)
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

/** Fold a tool event into a running tool list (dedupe by id, else name+running).
 *
 *  A terminal status sticks. The name path already only matches a row that is
 *  still running, but the id path would otherwise let a late `running` frame
 *  (a retry's progress, a duplicated id) un-finish a row the reader already
 *  saw complete — or, worse, hide a failure behind a spinner that never ends. */
export function mergeTool(tools: ToolCall[], ev: Extract<ChatEvent, { type: 'tool' }>): ToolCall[] {
  const copy = tools.slice()
  const idx = ev.id
    ? copy.findIndex((t) => t.id === ev.id)
    : copy.findIndex((t) => t.name === ev.name && t.status === 'running')
  if (idx >= 0) {
    const existing = copy[idx]!
    copy[idx] = {
      ...existing,
      label: ev.label || existing.label,
      status: terminal(existing.status) ? existing.status : (ev.status ?? existing.status),
    }
  } else {
    copy.push({ id: ev.id, name: ev.name, label: ev.label, status: ev.status ?? 'running' })
  }
  return copy
}
