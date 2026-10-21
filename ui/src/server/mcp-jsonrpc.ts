// JSON-RPC 2.0 over MCP — the wire shape and the method envelope every
// in-process MCP surface in this repo speaks. One shared dispatch: the
// server's name and what a tool call actually does arrive as callbacks.
//
// A leaf beside mcp-protocol.ts: the registry, the app dispatcher, and the
// workbench dispatcher all consume it, and none may import the others.

import { MCP_PROTOCOL_VERSION } from './mcp-protocol'

/** One JSON-RPC request message. `id` is `unknown` on purpose — JSON-RPC ids
 *  may be numbers, strings, or null, and the envelope echoes whatever the
 *  caller sent (or null when absent). */
export interface Rpc {
  jsonrpc?: string
  id?: unknown
  method?: string
  params?: { name?: string; arguments?: Record<string, unknown>; [k: string]: unknown }
}

export const result = (id: unknown, res: unknown) => ({ jsonrpc: '2.0', id: id ?? null, result: res })
export const rpcError = (id: unknown, code: number, message: string) => ({ jsonrpc: '2.0', id: id ?? null, error: { code, message } })

// ── What a reply says about the call, for the call record ───────────────────
// Twins of calls.rs (api/crates/talaria-mcp/src/calls.rs): the sentences a
// row keeps instead of a result, the argument scrub, and the reading of one
// reply into an outcome. Pure, so the leaf stays a leaf.

/** A tool-reported failure is recorded with this sentence — its own text is a
 *  result, and results are never kept (`TOOL_FAILED`). */
export const TOOL_FAILED = 'the tool reported an error (isError: true); its text is a result and is not kept'
/** What replaces a dispatcher's own sentence on a call that carried a spent
 *  credential: the reply may quote the real value back (`SPENT_REDACTED`). */
export const SPENT_REDACTED = 'its message is not kept: the call carried a spent credential the reply may quote'
/** What an echoed argument becomes in a stored sentence. */
export const ARGUMENT_MARK = '«argument»'
/** Argument strings shorter than this are not scrubbed (an "id" of "yes" is
 *  not a secret and would shred every sentence). calls.rs `SCRUB_MIN_CHARS`. */
const SCRUB_MIN_CHARS = 6
const SCRUB_BUDGET_BYTES = 64 * 1024

/** Every string the agent sent as an argument (6+ chars, anywhere in the
 *  tree), longest first — the list `scrubArguments` removes from a sentence.
 *  Taken BEFORE the spend boundary rewrites the arguments, so a resolved
 *  credential is never on this list (a spent call keeps no sentence at all). */
export function argumentStrings(rpc: Rpc | null): string[] {
  const out: string[] = []
  let budget = SCRUB_BUDGET_BYTES
  const walk = (v: unknown): void => {
    if (typeof v === 'string') {
      const s = v.trim()
      if ([...s].length >= SCRUB_MIN_CHARS && s.length <= budget) {
        budget -= s.length
        out.push(s)
      }
    } else if (Array.isArray(v)) v.forEach(walk)
    else if (v && typeof v === 'object') Object.values(v).forEach(walk)
  }
  walk(rpc?.params?.arguments)
  out.sort((a, b) => b.length - a.length)
  return [...new Set(out)]
}

/** `sentence` with every argument string the call carried replaced by
 *  `ARGUMENT_MARK` — a validator that quotes a bad value back is the one
 *  place arguments could leak into the error column. */
export function scrubArguments(sentence: string, args: string[]): string {
  let out = sentence
  for (const a of args) if (out.includes(a)) out = out.split(a).join(ARGUMENT_MARK)
  return out
}

/** The one sentence a row keeps for a reply: a JSON-RPC error's message, the
 *  fixed `TOOL_FAILED` for an `isError` result, nothing for success. On a
 *  call that spent a credential the dispatcher's own words are replaced by
 *  `SPENT_REDACTED` — the real value is exactly what a reply may quote. */
export function readCallVerdict(body: unknown, spent: boolean, args: string[]): { outcome: 'ok' | 'error'; error: string | null } {
  const b = body as { error?: { code?: unknown; message?: unknown }; result?: { isError?: unknown } } | null
  if (b?.error) {
    const code = b.error.code
    if (spent) return { outcome: 'error', error: `JSON-RPC error ${String(code ?? '?')}; ${SPENT_REDACTED}` }
    const msg = typeof b.error.message === 'string' ? b.error.message : 'JSON-RPC error'
    return { outcome: 'error', error: scrubArguments(msg, args) }
  }
  if (b?.result?.isError === true) return { outcome: 'error', error: TOOL_FAILED }
  return { outcome: 'ok', error: null }
}

/** The method envelope for an MCP tools surface. `tools` arrives ALREADY
 *  filtered to what the caller may use (the gateway's resolution, enforced
 *  again by the in-process dispatcher); `listEntry` shapes each tool for
 *  tools/list; `call` runs one tool and returns the text the model reads,
 *  with `isError: true` for a tool that answered with a failure. A thrown
 *  error becomes isError content — the exception's message is tool output
 *  the model should see and adapt to, not a transport failure. */
export async function dispatchJsonRpc<T extends { name: string }>(
  rpc: Rpc,
  tools: T[],
  opts: {
    serverName: string
    listEntry: (tool: T) => unknown
    call: (tool: T, args: Record<string, unknown>, rpc: Rpc) => Promise<{ text: string; isError?: boolean }> | { text: string; isError?: boolean }
  },
): Promise<{ status: number; body: unknown | null }> {
  switch (rpc.method) {
    case 'initialize':
      return {
        status: 200,
        body: result(rpc.id, {
          protocolVersion: (rpc.params?.protocolVersion as string) ?? MCP_PROTOCOL_VERSION,
          capabilities: { tools: {} },
          serverInfo: { name: opts.serverName, version: '1.0' },
        }),
      }
    case 'notifications/initialized':
      // A notification carries no id and expects no reply — acknowledged,
      // not answered.
      return { status: 202, body: null }
    case 'ping':
      return { status: 200, body: result(rpc.id, {}) }
    case 'tools/list':
      return { status: 200, body: result(rpc.id, { tools: tools.map(opts.listEntry) }) }
    case 'tools/call': {
      const tool = tools.find((t) => t.name === rpc.params?.name)
      if (!tool) return { status: 200, body: rpcError(rpc.id, -32602, `tool "${rpc.params?.name}" is not available here`) }
      // The wire says `arguments` is an object. For a remote server the
      // upstream validates; for an in-process tool the handler is the first
      // code to see the value, and `args.ids.map(...)` on `"arguments": "x"`
      // is a TypeError naming nothing useful. Refuse the shape here; the
      // app dispatcher checks the advertised inputSchema on top.
      const raw: unknown = rpc.params?.arguments ?? {}
      if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
        return { status: 200, body: rpcError(rpc.id, -32602, 'tools/call arguments must be a JSON object') }
      }
      try {
        const out = await opts.call(tool, raw as Record<string, unknown>, rpc)
        return {
          status: 200,
          body: out.isError
            ? result(rpc.id, { content: [{ type: 'text', text: out.text }], isError: true })
            : result(rpc.id, { content: [{ type: 'text', text: out.text }] }),
        }
      } catch (e) {
        return { status: 200, body: result(rpc.id, { content: [{ type: 'text', text: `error: ${(e as Error).message}` }], isError: true }) }
      }
    }
    default:
      return { status: 200, body: rpcError(rpc.id, -32601, `method "${rpc.method}" not supported`) }
  }
}
