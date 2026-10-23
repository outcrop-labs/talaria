// @talaria/sdk/testing — run an app's MCP tools in a unit test, with no host.
//
// `callAppTool(mcp, name, args, ctx?)` calls one tool of your mcp.ts through
// the code the gateway runs (app-mcp-call.ts): the load checks that leave a
// bad tool out, the inputSchema check, the handler under its deadline and
// signal, and the result's shaping — `toolResult`, `structuredContent`
// checked against `outputSchema`, the JSON a plain value becomes. It answers
// what the agent would receive. `memoryStore()` is a document store with the
// installed store's interface and answers, held in memory.
//
//   import mcp from './mcp'
//   import { callAppTool, memoryStore } from '@talaria/sdk/testing'
//
//   const store = memoryStore()
//   await store.insert('contacts', { name: 'Ada' })
//   const r = await callAppTool(mcp, 'contacts_search', { query: 'ada' }, { store })
//   expect(r.isError).toBe(false)
//
// Nothing here reaches a database, the api or the network — and nothing here
// belongs in mcp.ts or server.ts: an app build does not resolve this entry.
import { memoryStore } from '@/server/app-store-memory'
import { checkAppMcpTools, runAppTools } from '@/server/app-mcp-call'
import type { AppMcp, AppMcpCaller, AppToolContent, AppStore } from './server'

export { memoryStore }

/** What a test may set about the call; everything has a default. */
export interface AppToolCallOptions {
  /** The app's slug (`ctx.app`). Default `"app"`. */
  app?: string
  /** Who calls. Default a proven agent, `{ name: "test-agent", id:
   *  "test-agent", proven: true }`; give `{ proven: false }` to test how a
   *  tool treats a caller on the org-wide key, whose `id` is then null as
   *  the gateway sends it. */
  caller?: Partial<AppMcpCaller>
  /** The person the agent acts for (`ctx.actingUserId`). Default null. */
  actingUserId?: string | null
  /** The store (`ctx.store`). Default a fresh `memoryStore()`. */
  store?: AppStore
  /** The server's timeout in seconds. Default 120, as on a server an admin
   *  left alone. */
  timeoutSecs?: number
  /** Aborts like an agent disconnecting. */
  signal?: AbortSignal
  /** The agent's granted tool subset; a tool outside it is not available.
   *  Default every tool. */
  tools?: string[]
}

/** What the agent receives from one call. */
export interface AppToolCallResult {
  content: AppToolContent[]
  structuredContent?: Record<string, unknown>
  /** True for a tool-reported failure: a throw, `toolResult({ isError })`,
   *  arguments its inputSchema refuses, or a cut. */
  isError: boolean
  /** Every text block, joined with a newline — what most assertions read. */
  text: string
  /** True when the call was cut: the deadline passed, or `signal` aborted. */
  cut: boolean
  /** Which cut, when `cut` is true: `"deadline"` or `"caller"` (`signal`). */
  cutBy?: 'deadline' | 'caller'
}

/** Call one tool as the gateway would, and answer what the agent receives.
 *  Rejects with the gateway's own sentence when the gateway would answer an
 *  error instead of a result: a tool the load checks leave out (naming why),
 *  a name outside `tools`, or a result that breaks its own contract (content
 *  MCP does not define, structuredContent its outputSchema refuses). */
export async function callAppTool(
  mcp: AppMcp,
  name: string,
  args: Record<string, unknown> = {},
  ctx: AppToolCallOptions = {},
): Promise<AppToolCallResult> {
  const declared = Array.isArray(mcp?.tools) ? mcp.tools : []
  const checked = checkAppMcpTools(declared)
  const left = checked.problems.find((p) => p.startsWith(`${JSON.stringify(name)} `))
  if (left && !checked.tools.some((t) => t.name === name)) throw new Error(`tool ${left}, so the load checks leave it out`)
  const tools = checked.tools.filter((t) => !ctx.tools || ctx.tools.includes(t.name))
  // The gateway sends an unproven caller's id as null; a default id beside
  // `proven: false` is a pair production never produces.
  const given = ctx.caller ?? {}
  const proven = given.proven ?? true
  const caller: AppMcpCaller = {
    name: given.name ?? 'test-agent',
    id: given.id !== undefined ? given.id : proven ? 'test-agent' : null,
    proven,
  }
  // Through JSON, as the arguments cross the wire: a Date arrives as its
  // string, an undefined field not at all.
  const wire = JSON.parse(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name, arguments: args } }))
  const r = await runAppTools(ctx.app ?? 'app', tools, wire, {
    caller,
    actingUserId: ctx.actingUserId ?? null,
    store: ctx.store ?? memoryStore(),
    signal: ctx.signal,
    timeoutSecs: ctx.timeoutSecs,
  })
  const body = JSON.parse(JSON.stringify(r.body)) as {
    error?: { message: string }
    result?: { content: AppToolContent[]; structuredContent?: Record<string, unknown>; isError?: boolean }
  }
  if (body.error) throw new Error(body.error.message)
  const result = body.result!
  return {
    content: result.content,
    ...(result.structuredContent !== undefined ? { structuredContent: result.structuredContent } : {}),
    isError: result.isError === true,
    text: result.content.flatMap((b) => (b.type === 'text' ? [b.text] : [])).join('\n'),
    cut: r.cut !== null,
    ...(r.cut ? { cutBy: r.cut } : {}),
  }
}
