// One app tool call, as the gateway runs it — with no host around it. The
// checks a tool passes before any agent sees it, the argument and result
// contracts, the deadline, and the JSON-RPC envelope. A leaf: zod, the wire
// envelope (mcp-jsonrpc.ts) and types, nothing that reaches a database, the
// filesystem or the api. That is what lets `@talaria/sdk/testing` run an
// app's tools through exactly this code in a unit test, and the gateway
// (app-mcp.ts `dispatchAppMcp`) run them through it in production.
import { withApp } from './app-scope'
import { z } from 'zod'
import { dispatchJsonRpc, type Rpc, type ToolAnswer } from './mcp-jsonrpc'
import type { AppMcpCaller, AppMcpContext, AppMcpTool, AppToolContent, AppToolResult } from '@/sdk/server'
import type { AppHarnessRunner } from './app-harness-runner'
import type { AppStore } from './app-store'
import { NO_AGENTS, agentsFollowing, type AppAgents } from './app-agents'

// ── The checks a tool passes before any agent sees it ──────────────────────
// Every tool an app declares is checked when its module loads — by the build
// probe, and by every dispatch. A tool that fails is LEFT OUT and named: not
// listed, not callable, not in the registry's catalog. It never takes the app
// (or the app's other tools) down with it; a typo in one tool is that tool's
// problem. The bounds are the registry's own (`bound_catalog`,
// api/crates/talaria-mcp/src/registry.rs), which used to clip a third-party
// catalog silently and let an app's through unchecked.

/** registry.rs `TOOL_NAME_MAX`. */
export const TOOL_NAME_MAX = 200
/** registry.rs `TOOL_DESCRIPTION_MAX`: the catalog keeps this much. */
export const TOOL_DESCRIPTION_MAX = 300
/** registry.rs `TOOL_SCHEMA_MAX_BYTES` — for `inputSchema`, and for
 *  `outputSchema` by the same rule. */
export const TOOL_SCHEMA_MAX_BYTES = 32 * 1024
/** What a model API accepts as a function name. An agent's harness turns each
 *  MCP tool into one, and providers refuse the WHOLE request over a name with
 *  a space or a dot in it — so one bad name would break every turn of every
 *  agent carrying the app, not just calls to that tool. */
const TOOL_NAME_RE = /^[A-Za-z0-9_-]+$/

export const EMPTY_SCHEMA = { type: 'object', properties: {} } as const

export interface CheckedAppMcp {
  /** The tools that passed — the only ones listed, called or catalogued. */
  tools: AppMcpTool[]
  /** One sentence per tool left out: which one, and why. */
  problems: string[]
}

/** Why a declared schema is not one this gateway serves, or null. `what` is
 *  the field's name; `objectOnly` demands `type: "object"` outright (an
 *  `outputSchema` describes `structuredContent`, which MCP makes an object). */
function schemaShapeProblem(schema: unknown, what: string, objectOnly: boolean): string | null {
  if (!schema || typeof schema !== 'object' || Array.isArray(schema)) return `has an ${what} that is not a JSON Schema object`
  const type = (schema as Record<string, unknown>).type
  if (objectOnly ? type !== 'object' : type !== undefined && type !== 'object') {
    const what2 = what === 'inputSchema' ? 'tool arguments are an object' : 'structured results are an object'
    return type === undefined
      ? `has an ${what} without type "object"; ${what2}`
      : `has an ${what} of type ${JSON.stringify(type)}; ${what2}`
  }
  let bytes: number
  try {
    bytes = new TextEncoder().encode(JSON.stringify(schema)).length
  } catch {
    return `has an ${what} that is not JSON`
  }
  if (bytes > TOOL_SCHEMA_MAX_BYTES) {
    return `has a ${(bytes / 1024).toFixed(1)} KiB ${what} (at most ${TOOL_SCHEMA_MAX_BYTES / 1024} KiB)`
  }
  return null
}

function toolProblem(t: unknown, seen: ReadonlySet<string>): string | null {
  if (!t || typeof t !== 'object' || Array.isArray(t)) return 'is not a tool definition'
  const { name, description, handler, inputSchema, outputSchema } = t as Partial<Record<keyof AppMcpTool, unknown>>
  if (typeof name !== 'string' || !name) return 'has no name'
  if (name.length > TOOL_NAME_MAX) return `has a ${name.length}-character name (at most ${TOOL_NAME_MAX})`
  if (!TOOL_NAME_RE.test(name)) return 'has a name with characters other than letters, digits, _ and -'
  if (seen.has(name)) return 'is defined twice; only the first definition is kept'
  if (typeof description !== 'string' || !description.trim()) return 'has no description'
  if (typeof handler !== 'function') return 'has no handler function'
  if (inputSchema !== undefined) {
    const bad = schemaShapeProblem(inputSchema, 'inputSchema', false)
    if (bad) return bad
  }
  if (outputSchema !== undefined) {
    const bad = schemaShapeProblem(outputSchema, 'outputSchema', true)
    if (bad) return bad
  }
  return null
}

const toolLabel = (t: unknown, i: number): string => {
  const name = t && typeof t === 'object' ? (t as { name?: unknown }).name : undefined
  return typeof name === 'string' && name ? JSON.stringify(name.length > 60 ? `${name.slice(0, 60)}…` : name) : `tool #${i + 1}`
}

/** Check every declared tool; keep the ones that pass, name the rest. */
export function checkAppMcpTools(tools: readonly unknown[]): CheckedAppMcp {
  const ok: AppMcpTool[] = []
  const problems: string[] = []
  const seen = new Set<string>()
  tools.forEach((t, i) => {
    const problem = toolProblem(t, seen)
    if (problem) {
      problems.push(`${toolLabel(t, i)} ${problem}`)
      return
    }
    const tool = t as AppMcpTool
    seen.add(tool.name)
    ok.push(tool)
  })
  return { tools: ok, problems }
}

/** One tool as tools/list serves it: name, description, inputSchema, and the
 *  outputSchema when the tool declares one. */
export function listEntry(t: AppMcpTool): { name: string; description: string; inputSchema: Record<string, unknown>; outputSchema?: Record<string, unknown> } {
  return {
    name: t.name,
    description: t.description,
    inputSchema: t.inputSchema ?? { ...EMPTY_SCHEMA },
    ...(t.outputSchema ? { outputSchema: t.outputSchema } : {}),
  }
}

/** The catalog the registry row holds for an app server: what tools/list
 *  serves, with descriptions clipped to the registry's bound. A change here —
 *  a tool added, renamed or removed, a description or a schema edited — is
 *  what tells the agents carrying the server to list its tools again
 *  (talaria-apps `sync_app_mcp_servers` → `bump_tool_lists`, and the gateway's
 *  tools/list_changed notice), so they see the tools the app now has. */
export const catalogOf = (tools: readonly AppMcpTool[]): Array<ReturnType<typeof listEntry>> =>
  tools.map((t) => ({ ...listEntry(t), description: t.description.slice(0, TOOL_DESCRIPTION_MAX) }))

// ── Schemas as checks ───────────────────────────────────────────────────────
// `inputSchema` was advisory: advertised in tools/list, never checked, so the
// handler was the first code to see a wrong shape. zod's `fromJSONSchema`
// turns the JSON Schema into a parser (extra properties pass unless the
// schema forbids them, as JSON Schema says); a schema it cannot convert
// ($ref, a draft it does not know) falls back to no check rather than
// refusing every call to that tool. Compiled once per schema object. The same
// door checks a structured result against the tool's `outputSchema`.
const compiled = new WeakMap<object, z.ZodType | null>()

/** The error `value` raises against `schema`, or null when it passes (or the
 *  schema has no parser). */
function schemaError(schema: Record<string, unknown> | undefined, value: unknown): z.ZodError | null {
  if (!schema) return null
  let parser = compiled.get(schema)
  if (parser === undefined) {
    try {
      parser = z.fromJSONSchema(schema as Parameters<typeof z.fromJSONSchema>[0])
    } catch {
      parser = null
    }
    compiled.set(schema, parser)
  }
  if (!parser) return null
  const r = parser.safeParse(value)
  return r.success ? null : r.error
}

/** Why `value` does not satisfy `schema`, in zod's words, or null. zod quotes
 *  keys and paths, so this is for the agent's own arguments only; a sentence
 *  the call record keeps uses `issueKinds`. */
export function schemaProblem(schema: Record<string, unknown> | undefined, value: unknown): string | null {
  const e = schemaError(schema, value)
  return e ? z.prettifyError(e) : null
}

/** The kinds of issue in a refusal, counted: `2 × invalid_type, 1 ×
 *  unrecognized_keys`. zod's issue codes only — never a key, a path or a
 *  value, which belong to the data refused. */
function issueKinds(e: z.ZodError): string {
  const counts = new Map<string, number>()
  for (const issue of e.issues) counts.set(issue.code, (counts.get(issue.code) ?? 0) + 1)
  return [...counts].map(([code, n]) => `${n} × ${code}`).join(', ')
}

export const argumentProblem = (schema: Record<string, unknown> | undefined, args: Record<string, unknown>): string | null =>
  schemaProblem(schema, args)

// ── What a handler may return ───────────────────────────────────────────────
// A string reaches the agent as one text block; any other value as its JSON,
// pretty-printed — unchanged since the first SDK. `toolResult(...)` is the
// whole MCP result instead: content blocks (text, images, audio, resource
// links), `structuredContent` the agent's client can read without parsing
// text, and `isError` for a failure that is not an exception. A tool that
// declares an `outputSchema` answers structured: its plain object return is
// the `structuredContent`, and whatever it returns is checked against the
// schema before the agent sees it.

/** The brand `toolResult` stamps. `Symbol.for`, not a module-local symbol: an
 *  app's built mcp.js reaches the SDK through the host's runtime shim, and a
 *  registry symbol is the same symbol on every side of that seam. */
export const TOOL_RESULT: unique symbol = Symbol.for('talaria.app-mcp.tool-result')

/** A whole MCP tool result, for a handler to return: content blocks,
 *  `structuredContent`, `isError`. Returned as-is, it reaches the agent
 *  unchanged — with a text block of the structured content's JSON added when
 *  you give no content, for clients that read only text. */
export function toolResult(init: { content?: AppToolContent[]; structuredContent?: Record<string, unknown>; isError?: boolean }): AppToolResult {
  return { ...init, [TOOL_RESULT]: true } as AppToolResult
}

export const isToolResult = (v: unknown): v is AppToolResult =>
  !!v && typeof v === 'object' && (v as { [TOOL_RESULT]?: unknown })[TOOL_RESULT] === true

/** True when `value` — a string, or any string inside an array or object —
 *  carries a `«secret:…»` handle. The gateway resolves every handle it may on
 *  the way into your handler; one still here did not resolve (the secret's
 *  Spendable-at list lacks `talaria-app://<slug>`, the agent holds no grant,
 *  the secret expired, or the agent called on the org-wide key), and it is
 *  text, not a credential. Refuse with a sentence naming the secret instead
 *  of sending it upstream. */
export function isUnresolvedHandle(value: unknown): boolean {
  const seen = new Set<object>()
  const walk = (v: unknown): boolean => {
    if (typeof v === 'string') return /«secret:/i.test(v)
    if (!v || typeof v !== 'object' || seen.has(v)) return false
    seen.add(v)
    return (Array.isArray(v) ? v : Object.values(v)).some(walk)
  }
  return walk(value)
}

/** Why one content block is not a block MCP defines, or null. */
function contentProblem(b: unknown): string | null {
  if (!b || typeof b !== 'object' || Array.isArray(b)) return 'is not a content block object'
  const block = b as Record<string, unknown>
  const str = (k: string) => typeof block[k] === 'string'
  switch (block.type) {
    case 'text':
      return str('text') ? null : 'is a text block without a string `text`'
    case 'image':
    case 'audio':
      return str('data') && str('mimeType') ? null : `is an ${block.type} block without string \`data\` (base64) and \`mimeType\``
    case 'resource_link':
      return str('uri') && str('name') ? null : 'is a resource_link without string `uri` and `name`'
    case 'resource': {
      const r = block.resource as Record<string, unknown> | undefined
      if (!r || typeof r !== 'object' || typeof r.uri !== 'string') return 'is a resource block without a `resource.uri`'
      return typeof r.text === 'string' || typeof r.blob === 'string' ? null : 'is a resource block without `resource.text` or `resource.blob`'
    }
    default:
      return `has type ${JSON.stringify(block.type)}; MCP content is text, image, audio, resource_link or resource`
  }
}

const isPlainObject = (v: unknown): v is Record<string, unknown> =>
  !!v && typeof v === 'object' && !Array.isArray(v) && (Object.getPrototypeOf(v) === Object.prototype || Object.getPrototypeOf(v) === null)

/** A result the server itself could not answer with: -32603, naming the tool
 *  and what broke. Never the tool's own data — the record keeps this
 *  sentence. */
const broken = (tool: AppMcpTool, why: string): ToolAnswer => ({
  error: { code: -32603, message: `tool "${tool.name}" ${why}` },
})

/** The JSON text of a value, or null when it has none (a cycle, a BigInt). */
function jsonText(v: unknown, pretty: boolean): string | null {
  try {
    return JSON.stringify(v ?? null, null, pretty ? 2 : undefined) ?? 'null'
  } catch {
    return null
  }
}

/** A handler's return value as the agent receives it. */
export function shapeToolAnswer(tool: AppMcpTool, out: unknown): ToolAnswer {
  const schema = tool.outputSchema
  let content: unknown[] | undefined
  let structured: unknown
  let isError = false
  if (isToolResult(out)) {
    if (out.content !== undefined) {
      if (!Array.isArray(out.content)) return broken(tool, 'returned a toolResult whose content is not an array')
      for (const [i, b] of out.content.entries()) {
        const bad = contentProblem(b)
        if (bad) return broken(tool, `returned a toolResult whose content[${i}] ${bad}`)
      }
      content = out.content
    }
    if (out.structuredContent !== undefined && !isPlainObject(out.structuredContent)) {
      return broken(tool, 'returned a toolResult whose structuredContent is not a JSON object')
    }
    structured = out.structuredContent
    isError = out.isError === true
  } else if (schema) {
    // A tool that declares an outputSchema answers structured: its object is
    // the structured content, and its JSON the text block.
    if (!isPlainObject(out)) {
      return broken(tool, 'declares an outputSchema, so it must return a JSON object or toolResult({ structuredContent })')
    }
    structured = out
  } else {
    if (typeof out === 'string') return { text: out }
    const text = jsonText(out, true)
    if (text === null) return broken(tool, 'returned a value that is not JSON')
    return { text }
  }
  let structuredText: string | null = null
  if (structured !== undefined) {
    structuredText = jsonText(structured, false)
    if (structuredText === null) return broken(tool, 'returned structuredContent that is not JSON')
  }
  // An error result need not hold the schema's contract; anything else must.
  if (schema && !isError) {
    if (structured === undefined) return broken(tool, 'declares an outputSchema but returned no structuredContent')
    // Named by issue kind only: zod's own words quote the result's keys.
    const bad = schemaError(schema, JSON.parse(structuredText!))
    if (bad) return broken(tool, `returned structuredContent its outputSchema refuses (${issueKinds(bad)})`)
  }
  if (!content?.length && structuredText !== null) content = [{ type: 'text', text: structuredText }]
  return {
    result: {
      content: content ?? [],
      ...(structured !== undefined ? { structuredContent: structured as Record<string, unknown> } : {}),
      ...(isError ? { isError: true } : {}),
    },
  }
}

// ── The deadline ────────────────────────────────────────────────────────────
// A handler runs under the server's `timeoutSecs` — the same per-server
// setting the Rust relay holds a remote server's hop to, 120 s unless an
// admin set one in Manage → MCP. `ctx.signal` aborts when it passes, or when
// the agent disconnects; the gateway stops waiting at that moment and answers
// (or, for a caller who left, records the call cancelled). A handler that
// ignores the signal keeps running to its end — JavaScript cannot stop it —
// but nothing it returns reaches anyone.

/** mcp_gw_server.rs `timeout_secs.unwrap_or(120)`. */
export const DEFAULT_TOOL_TIMEOUT_SECS = 120

export const timeoutSecsOf = (s: number | null | undefined): number =>
  typeof s === 'number' && Number.isFinite(s) && s > 0 ? s : DEFAULT_TOOL_TIMEOUT_SECS

/** The record's sentence for a call the deadline cut. */
export const toolTimedOut = (secs: number): string =>
  `the tool did not finish within ${secs} s (the server's timeout); the call was cut off`

/** `ctx.harness` where nothing bound one: a tool run with no host behind it.
 *  It refuses rather than running unbound — a harness run with no caller to
 *  stamp and no person to check is the overreach the bound runner exists to
 *  stop. `callAppTool` takes a `harness` to stand in. */
const noHarness = (): never => {
  throw new Error('ctx.harness is not available here: this call has no host to bind a runner to (in a test, pass `harness` to callAppTool)')
}
const NO_HARNESS: AppHarnessRunner = { run: noHarness, stream: noHarness }

/** `harness`, with every run also following `signal`. */
function followingSignal(harness: AppHarnessRunner, signal: AbortSignal): AppHarnessRunner {
  const both = (own: AbortSignal | undefined) => (own ? AbortSignal.any([own, signal]) : signal)
  return {
    run: (def, input, opts = {}) => harness.run(def, input, { ...opts, signal: both(opts.signal) }),
    stream: (def, input, opts = {}) => harness.stream(def, input, { ...opts, signal: both(opts.signal) }),
  }
}

/** Everything a call knows besides its arguments. */
export interface AppToolCallFacts {
  caller: AppMcpCaller
  /** The person the agent acts for, when the gateway knows it. */
  actingUserId: string | null
  store: AppStore
  /** `ctx.agents`, bound to this caller by the host (app-mcp.ts builds it
   *  with `appServices`). Absent where no host is — a unit test that gave
   *  none gets one that says so. */
  agents?: AppAgents
  /** `ctx.harness`, bound to this caller by the host (app-mcp.ts builds it with
   *  `appHarnessRunner`). Absent where no host is — a unit test that gave
   *  none gets a runner that says so. */
  harness?: AppHarnessRunner
  /** The request's signal: aborts when the agent disconnects. */
  signal?: AbortSignal
  /** The server's `timeoutSecs`; null or absent is the 120 s default. */
  timeoutSecs?: number | null
}

/** How a call ended, beyond its answer: cut by the deadline, cut by the
 *  caller leaving, or not cut. */
export type CallCut = 'deadline' | 'caller' | null

class Cut {
  constructor(readonly by: 'deadline' | 'caller') {}
}

/** Run the app's (already checked and granted) tools over one JSON-RPC
 *  request, under the deadline. */
export async function runAppTools(
  slug: string,
  tools: AppMcpTool[],
  rpc: Rpc,
  facts: AppToolCallFacts,
): Promise<{ status: number; body: unknown | null; cut: CallCut }> {
  let cut: CallCut = null
  const secs = timeoutSecsOf(facts.timeoutSecs)
  const reply = await dispatchJsonRpc(rpc, tools, {
    serverName: `talaria-app-${slug}`,
    listEntry,
    call: async (tool, args) => {
      // A caller already gone (it left while access resolved or credentials
      // were spent) gets no handler run: its side effects would land for
      // nobody, on a call recorded as cancelled.
      if (facts.signal?.aborted) {
        cut = 'caller'
        return { text: `tool "${tool.name}" was not started: the caller had already disconnected`, isError: true }
      }
      const bad = argumentProblem(tool.inputSchema, args)
      if (bad) return { text: `invalid arguments for ${tool.name}:\n${bad}`, isError: true }
      const ctl = new AbortController()
      const timer = setTimeout(
        () => ctl.abort(new DOMException(`tool "${tool.name}" did not finish within ${secs} s`, 'TimeoutError')),
        secs * 1000,
      )
      const callerLeft = () => ctl.abort(facts.signal?.reason ?? new DOMException('the caller disconnected', 'AbortError'))
      // Armed BEFORE the handler starts, so this listener runs first when the
      // signal fires: the race settles as a cut even when the handler's own
      // fetch rejects on the same abort.
      const cutting = new Promise<never>((_, reject) => {
        ctl.signal.addEventListener('abort', () => reject(new Cut(facts.signal?.aborted ? 'caller' : 'deadline')), { once: true })
      })
      cutting.catch(() => {})
      facts.signal?.addEventListener('abort', callerLeft, { once: true })
      const ctx: AppMcpContext = {
        app: slug,
        agent: facts.caller.name,
        caller: facts.caller,
        actingUserId: facts.actingUserId,
        store: facts.store,
        signal: ctl.signal,
        // Every turn follows THIS call's signal — the deadline and the
        // agent's disconnect — on top of any the handler passes.
        agents: facts.agents ? agentsFollowing(facts.agents, ctl.signal) : NO_AGENTS,
        // The runner follows THIS call's signal — the deadline and the
        // caller's disconnect — on top of any the handler passes.
        harness: facts.harness ? followingSignal(facts.harness, ctl.signal) : NO_HARNESS,
      }
      try {
        // As the app: a harness the handler runs signs with this app's key.
        const running = Promise.resolve().then(() => withApp(slug, () => tool.handler(args, ctx)))
        running.catch(() => {}) // a cut call's late rejection is nobody's
        return shapeToolAnswer(tool, await Promise.race([running, cutting]))
      } catch (e) {
        if (e instanceof Cut) {
          cut = e.by
          // The gateway answers a caller cut with no body (499); this one is
          // read by a test that aborted `callAppTool`'s signal.
          const text =
            e.by === 'caller'
              ? `tool "${tool.name}" was cut off: the caller disconnected before it finished, and it may have done part of its work`
              : `tool "${tool.name}" did not finish within ${secs} s, this server's time limit; it was cut off and may have done part of its work`
          return { text, isError: true }
        }
        throw e
      } finally {
        clearTimeout(timer)
        facts.signal?.removeEventListener('abort', callerLeft)
      }
    },
  })
  return { ...reply, cut }
}
