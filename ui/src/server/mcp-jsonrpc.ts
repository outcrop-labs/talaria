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

/** What one tool call answers, as the dispatcher frames it:
 *   - `{ text, isError? }` — one text block, the common case;
 *   - `{ result }` — a whole MCP `CallToolResult` (content blocks,
 *     `structuredContent`, `isError`), passed to the caller unchanged;
 *   - `{ error }` — a JSON-RPC error: the server itself could not answer
 *     (a result that broke its own contract), not a tool-reported failure. */
export type ToolAnswer =
  | { text: string; isError?: boolean }
  | { result: { content: unknown[]; structuredContent?: Record<string, unknown>; isError?: boolean } }
  | { error: { code: number; message: string } }

/** The method envelope for an MCP tools surface. `tools` arrives ALREADY
 *  filtered to what the caller may use (the gateway's resolution, enforced
 *  again by the in-process dispatcher); `listEntry` shapes each tool for
 *  tools/list; `call` runs one tool and answers what the model reads
 *  (`ToolAnswer`), with `isError: true` for a tool that answered with a
 *  failure. A thrown error becomes isError content — the exception's message
 *  is tool output the model should see and adapt to, not a transport
 *  failure. */
export async function dispatchJsonRpc<T extends { name: string }>(
  rpc: Rpc,
  tools: T[],
  opts: {
    serverName: string
    listEntry: (tool: T) => unknown
    call: (tool: T, args: Record<string, unknown>, rpc: Rpc) => Promise<ToolAnswer> | ToolAnswer
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
        if ('error' in out) return { status: 200, body: rpcError(rpc.id, out.error.code, out.error.message) }
        if ('result' in out) return { status: 200, body: result(rpc.id, out.result) }
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

// ── The gateway's answers to a body it will not dispatch ────────────────────
// Twins of the Rust gateway (api/crates/talaria-routes-fleet/src/mcp/
// mcp_gw_server.rs) and its call record (talaria-mcp `calls.rs`), so an agent
// gets the same codes, the same sentences and the same record whichever
// runtime answers its server. The app gateway used to parse with JSON.parse,
// dispatch `{}` on a failure and answer -32601 'method "undefined" not
// supported' — to a double-escaped tools/call and to a batch alike — so an
// agent was told its method was unsupported when its JSON was malformed.

/** calls.rs `BATCH_REFUSED`: the record's sentence for a tools/call that rode
 *  in a batch, and the answer to each member. */
export const BATCH_REFUSED =
  'a JSON-RPC batch carrying tools/call or tools/list is not accepted here; send each request on its own'
/** The answer to a batch that carries neither — which the Rust relay passes
 *  upstream, and an in-process app server, answering one request at a time,
 *  has nothing to pass it to. */
export const BATCH_NOT_SERVED = 'a JSON-RPC batch is not accepted by an app server; send each request on its own'
/** calls.rs `ACCESS_FAILED`. */
export const ACCESS_FAILED = "the gateway could not resolve this agent's access to the server (internal error)"
/** calls.rs `CALLER_GONE`: the caller hung up before the answer finished. */
export const CALLER_GONE = 'the caller disconnected before the answer finished'
/** The answer to a body that parsed but is not a request object. */
export const NOT_A_REQUEST = 'the request is not a JSON-RPC request object'
/** The answer to a request object that names no method. */
export const NO_METHOD = 'the request names no method'

/** A leading UTF-8 byte-order mark is not JSON, but a well-formed body behind
 *  one is (mcp_gw_server.rs `strip_bom`). */
export function stripBom(body: Uint8Array): Uint8Array {
  return body[0] === 0xef && body[1] === 0xbb && body[2] === 0xbf ? body.subarray(3) : body
}

/** Why a body is not JSON, in serde_json's words, and where: `offset` is the
 *  whole-body byte offset `error_byte_offset` computes from serde's line and
 *  column — one past the failing byte, or the body's length when the input
 *  ended early. */
export interface JsonSyntaxError {
  message: string
  offset: number
}

const isWs = (b: number | undefined): boolean => b === 0x20 || b === 0x09 || b === 0x0a || b === 0x0d
const isDigit = (b: number | undefined): boolean => b !== undefined && b >= 0x30 && b <= 0x39
const isHex = (b: number | undefined): boolean =>
  b !== undefined && ((b >= 0x30 && b <= 0x39) || (b >= 0x41 && b <= 0x46) || (b >= 0x61 && b <= 0x66))

/** The first syntax error in `body` (RFC 8259 JSON, UTF-8 inside strings), or
 *  null for a body that is one valid JSON text. A byte walk rather than
 *  JSON.parse's message, whose wording and position differ by engine (Bun,
 *  Node) and which reads a decoded string — the diagnostic is about BYTES. */
export function jsonSyntaxError(body: Uint8Array): JsonSyntaxError | null {
  const len = body.length
  let i = 0
  class Stop {
    constructor(
      readonly message: string,
      readonly offset: number,
    ) {}
  }
  const at = (message: string): never => {
    throw new Stop(message, i + 1)
  }
  const eof = (what: string): never => {
    throw new Stop(`EOF while parsing ${what}`, len)
  }
  const ws = () => {
    while (i < len && isWs(body[i])) i++
  }
  const string = () => {
    i++ // the opening quote
    // serde reads the whole string before it checks its UTF-8, so a bad
    // sequence is reported at the closing quote.
    let badUtf8 = false
    for (;;) {
      if (i >= len) eof('a string')
      const b = body[i]!
      if (b === 0x22) {
        if (badUtf8) at('invalid unicode code point')
        i++
        return
      }
      if (b === 0x5c) {
        i++
        if (i >= len) eof('a string')
        const e = body[i]!
        if (e === 0x75) {
          // serde takes all four bytes, then checks them.
          if (i + 4 >= len) {
            i = len
            eof('a string')
          }
          for (let k = 1; k <= 4; k++) {
            if (!isHex(body[i + k])) {
              i += 4
              at('invalid escape')
            }
          }
          i += 5
          continue
        }
        if (![0x22, 0x5c, 0x2f, 0x62, 0x66, 0x6e, 0x72, 0x74].includes(e)) at('invalid escape')
        i++
        continue
      }
      if (b < 0x20) at('control character (\\u0000-\\u001F) found while parsing a string')
      if (b < 0x80) {
        i++
        continue
      }
      // A multi-byte UTF-8 sequence: a lead byte and its continuations.
      const need = b >= 0xc2 && b <= 0xdf ? 1 : b >= 0xe0 && b <= 0xef ? 2 : b >= 0xf0 && b <= 0xf4 ? 3 : 0
      if (need === 0) badUtf8 = true
      let k = 1
      for (; k <= need; k++) {
        const c = body[i + k]
        if (c === undefined) break // the input ended: the loop's EOF says so
        if (c < 0x80 || c > 0xbf) {
          badUtf8 = true
          break
        }
      }
      i += k
    }
  }
  const number = () => {
    if (body[i] === 0x2d) i++
    if (i >= len) eof('a value')
    if (body[i] === 0x30) {
      i++
      if (isDigit(body[i])) at('invalid number') // a leading zero
    } else if (isDigit(body[i])) while (isDigit(body[i])) i++
    else at('invalid number')
    if (body[i] === 0x2e) {
      i++
      if (i >= len) eof('a value')
      if (!isDigit(body[i])) at('invalid number')
      while (isDigit(body[i])) i++
    }
    if (body[i] === 0x65 || body[i] === 0x45) {
      i++
      if (body[i] === 0x2b || body[i] === 0x2d) i++
      if (i >= len) eof('a value')
      if (!isDigit(body[i])) at('invalid number')
      while (isDigit(body[i])) i++
    }
  }
  const literal = (word: string) => {
    for (let k = 0; k < word.length; k++) {
      if (i >= len) eof('a value')
      if (body[i] !== word.charCodeAt(k)) at('expected ident')
      i++
    }
  }
  const value = (depth: number): void => {
    if (depth > 128) at('recursion limit exceeded')
    ws()
    if (i >= len) eof('a value')
    const b = body[i]
    if (b === 0x7b) {
      i++
      ws()
      if (body[i] === 0x7d) {
        i++
        return
      }
      for (;;) {
        ws()
        if (i >= len) eof('an object')
        if (body[i] !== 0x22) at('key must be a string')
        string()
        ws()
        if (i >= len) eof('an object')
        if (body[i] !== 0x3a) at('expected `:`')
        i++
        value(depth + 1)
        ws()
        if (i >= len) eof('an object')
        if (body[i] === 0x2c) {
          i++
          ws()
          if (body[i] === 0x7d) at('trailing comma')
          continue
        }
        if (body[i] === 0x7d) {
          i++
          return
        }
        at('expected `,` or `}`')
      }
    }
    if (b === 0x5b) {
      i++
      ws()
      if (i >= len) eof('a list')
      if (body[i] === 0x5d) {
        i++
        return
      }
      for (;;) {
        value(depth + 1)
        ws()
        if (i >= len) eof('a list')
        if (body[i] === 0x2c) {
          i++
          ws()
          if (body[i] === 0x5d) at('trailing comma')
          continue
        }
        if (body[i] === 0x5d) {
          i++
          return
        }
        at('expected `,` or `]`')
      }
    }
    if (b === 0x22) return string()
    if (b === 0x2d || isDigit(b)) return number()
    if (b === 0x74) return literal('true')
    if (b === 0x66) return literal('false')
    if (b === 0x6e) return literal('null')
    at('expected value')
  }
  try {
    value(0)
    ws()
    if (i < len) at('trailing characters')
    return null
  } catch (e) {
    if (e instanceof Stop) return { message: e.message, offset: Math.min(e.offset, len) }
    throw e
  }
}

/** serde's "line L column C" for a whole-body offset — the inverse of
 *  `error_byte_offset`: the line is one plus the newlines before the offset,
 *  the column the bytes since the last of them. */
function lineColumn(body: Uint8Array, offset: number): { line: number; column: number } {
  let line = 1
  let start = 0
  for (let k = 0; k < offset && k < body.length; k++) {
    if (body[k] === 0x0a) {
      line++
      start = k + 1
    }
  }
  return { line, column: offset - start }
}

/** mcp_gw_server.rs `parse_diagnostic`: the offset and a hexdump of the 8
 *  bytes before it through the 8 after, hex pairs plus printable ASCII (a dot
 *  for anything else). Those windowed bytes are the ONLY body content that
 *  ever reaches a log line or an answer: bodies carry tool arguments and
 *  credentials. */
export function parseDiagnostic(body: Uint8Array, offset: number): string {
  const window = body.subarray(Math.max(0, offset - 8), Math.min(body.length, offset + 8))
  const hex = [...window].map((b) => b.toString(16).padStart(2, '0')).join(' ')
  const ascii = [...window].map((b) => (b >= 0x20 && b <= 0x7e ? String.fromCharCode(b) : '.')).join('')
  return `offset ${offset} | ${hex} | ${ascii}`
}

/** mcp_gw_server.rs `parse_error_response`'s sentence: serde's reason, the
 *  caller-side hint, and the bounded diagnostic. Answered as -32700 with a
 *  null id over HTTP 200. */
export function parseErrorMessage(body: Uint8Array, err: JsonSyntaxError): string {
  const { line, column } = lineColumn(body, err.offset)
  return (
    `request body is not valid JSON: ${err.message} at line ${line} column ${column}. The call was refused here, ` +
    `not forwarded: the hexdump below is what the caller sent, so the fix is on the sender's serialization ` +
    `(nested tool arguments need one \\ escape per JSON layer). ${parseDiagnostic(body, err.offset)}`
  )
}

/** A gateway body, read: the parsed value, or why it is not JSON. A leading
 *  BOM is stripped first; an empty body is a parse error. */
export function readRpcBody(raw: Uint8Array): { ok: true; value: unknown } | { ok: false; message: string; diagnostic: string } {
  const body = stripBom(raw)
  try {
    return { ok: true, value: JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(body)) as unknown }
  } catch {
    const err = jsonSyntaxError(body) ?? { message: 'invalid unicode code point', offset: 0 }
    return { ok: false, message: parseErrorMessage(body, err), diagnostic: parseDiagnostic(body, err.offset) }
  }
}

const methodOf = (v: unknown): string | null => {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null
  const m = (v as { method?: unknown }).method
  return typeof m === 'string' ? m : null
}

/** calls.rs `recorded_members`: the tools/calls a body carries — the body
 *  itself when it is one, each tools/call member when it is a batch. */
export function recordedMembers(v: unknown): Rpc[] {
  if (Array.isArray(v)) return v.filter((m) => methodOf(m) === 'tools/call') as Rpc[]
  return methodOf(v) === 'tools/call' ? [v as Rpc] : []
}

/** calls.rs `tool_of`: the called tool's name when the call names one as a
 *  string, else null. */
export function toolOf(rpc: unknown): string | null {
  const name = (rpc as { params?: { name?: unknown } } | null)?.params?.name
  return typeof name === 'string' ? name : null
}

/** The answer to a batch: one -32600 per member that carries an id, none for
 *  a notification — or a single -32600 with a null id when no member could
 *  take a reply (mcp_gw_server.rs `batch_refusal`). A batch carrying a
 *  tools/call or a tools/list gets the Rust gateway's sentence; any other
 *  batch the app sentence. */
export function batchAnswer(batch: unknown[]): unknown {
  const tools = batch.some((m) => {
    const method = methodOf(m)
    return method === 'tools/call' || method === 'tools/list'
  })
  const sentence = tools ? BATCH_REFUSED : BATCH_NOT_SERVED
  const replies = batch
    .filter((m): m is { id: unknown } => !!m && typeof m === 'object' && !Array.isArray(m) && 'id' in m)
    .map((m) => rpcError(m.id, -32600, sentence))
  return replies.length ? replies : rpcError(null, -32600, sentence)
}

/** The -32600 for a body that parsed into something no dispatcher can read:
 *  not an object, or an object naming no method. Null for a request. */
export function invalidRequest(v: unknown): unknown | null {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return rpcError(null, -32600, NOT_A_REQUEST)
  if (methodOf(v) === null) return rpcError((v as { id?: unknown }).id, -32600, NO_METHOD)
  return null
}

/** mcp_gw_server.rs `method_closed_by_subset`: a tool SUBSET closes the
 *  resource, prompt and completion surfaces, which carry no tool name for the
 *  gate to read. */
export const methodClosedBySubset = (method: string): boolean =>
  method.startsWith('resources/') || method.startsWith('prompts/') || method.startsWith('completion/')

/** mcp_gw_server.rs `method_refusal`. */
export const methodRefusal = (method: string): string =>
  `method "${method}" is not available to this agent here (it holds a tool subset)`
