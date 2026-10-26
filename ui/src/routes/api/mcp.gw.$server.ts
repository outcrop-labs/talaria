import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireAgent } from '@/server/agent-auth'
import { effectiveMcpFor, recordMcpToolCall, type EffectiveMcp, type McpCallOutcome } from '@/server/mcp-registry'
import {
  ACCESS_FAILED,
  BATCH_REFUSED,
  CALLER_GONE,
  argumentStrings,
  batchAnswer,
  invalidRequest,
  methodClosedBySubset,
  methodRefusal,
  readCallVerdict,
  readRpcBody,
  recordedMembers,
  rpcError,
  toolOf,
  type Rpc,
} from '@/server/mcp-jsonrpc'
import { timeoutSecsOf, toolTimedOut } from '@/server/app-mcp-call'
import type { OrgToolCall } from '@/server/app-org-tools'

/** The org-tool caller for this request's agent: its own credential, as it
 *  presented it. Resolved on a harness's first org call, not per dispatch —
 *  most tool calls run no harness at all. */
function orgToolsFor(request: Request, name: string): OrgToolCall {
  return async (server, tool, args, signal) => {
    const { presentedCredential } = await import('@/server/agent-auth')
    const credential = presentedCredential(request)
    const { orgToolsAsAgent } = await import('@/server/app-org-tools')
    const call = credential ? orgToolsAsAgent(credential, name) : null
    if (!call) return { text: `"${server}.${tool}" could not be called: there is no agent credential or no api to call it through`, isError: true }
    return call(server, tool, args, signal)
  }
}

// The app-MCP gateway — the registry's ENFORCEMENT point for servers an app
// publishes (/api/mcp/gw/app-<slug>). Agents never see the module or its
// inputs: their configs point here, the agent's own credential identifies the
// caller (agent-auth), and this route resolves the agent's grant (assignment ∩
// the acting owner's allowance) and dispatches the compiled module IN PROCESS
// — app code runs in the TS runtime that can load it (RUST-MIGRATION.md,
// rule 10). Every other MCP server — the builtin toolkit, the Workbench,
// third-party URLs — is the Rust api's relay (api/crates/talaria-routes-fleet/
// src/mcp/mcp_gw_server.rs); the proxy holds back only the app- prefix, so
// those requests never reach this file.
//
// ONE GATEWAY CONTRACT, WHICHEVER RUNTIME ANSWERS. A body that is not JSON is
// a -32700 with the Rust gateway's sentence and bounded hexdump; a batch is a
// -32600 per member; a request that is not an object, or names no method, is
// a -32600; a tool subset closes resources/prompts/completion with the Rust
// sentence — the shapes are in mcp-jsonrpc.ts, twins of the Rust ones.
//
// EVERY tools/call LEAVES A ROW in `mcp_tool_calls` — the refused ones too, a
// batch's members included, and a request refused by method under its own
// method — with the same columns and the same never-the-arguments rule as the
// Rust gateway's CallLog (`recordMcpToolCall`, mcp-registry.ts).
//
// THE HANDLER LEARNS WHO IS CALLING, FOR WHOM, AND UNTIL WHEN: `ctx.caller`
// (name, id, and whether the call came on the agent's own key), the person a
// personal assistant acts for, and a signal that aborts at the server's
// timeout (120 s by default) or when the agent disconnects.

/** The most of a body this gateway reads: axum's default body limit, which
 *  the Rust gateway's `Bytes` extractor holds every other server to. */
const BODY_MAX_BYTES = 2 * 1024 * 1024
const NO_ACCESS = 'no access to this MCP server'

export const Route = defineApi('/api/mcp/gw/$server', {
  POST: async ({ request, params }) => {
    const startedAt = Date.now()
    const caller = await requireAgent(request)
    if (caller instanceof Response) return caller
    // Pass the CALLER, never `caller.model`. `subjectModel`/`subjectProven`
    // read a bare string as PROVEN, so downgrading to the name here throws
    // away `legacy` — and this route is where that matters most: it resolves
    // the acting owner and gates what that human's assistant may reach.
    // `name` below is only ever used where an unthreaded callee genuinely
    // needs the string.
    const name = caller.model

    // Read and parsed ONCE, before access is known, so a refused tools/call is
    // recorded as the call it was. A failure is kept for the -32700 below,
    // which answers only a caller the gateway admits (the Rust order).
    const declared = Number(request.headers.get('content-length') ?? 0)
    if (declared > BODY_MAX_BYTES) return json({ error: 'request body is over 2 MiB' }, { status: 413 })
    const bytes = new Uint8Array(await request.arrayBuffer())
    if (bytes.length > BODY_MAX_BYTES) return json({ error: 'request body is over 2 MiB' }, { status: 413 })
    const parsed = readRpcBody(bytes)
    const body = parsed.ok ? parsed.value : undefined
    // One row per tools/call the body carries — the body itself, or each
    // tools/call member of a batch. The record never sees the arguments: only
    // the tool name and, scrubbed, the one sentence the caller got.
    const members = recordedMembers(body)
    let serverId: string | null = null
    const record = (member: Rpc, outcome: McpCallOutcome, status: number, error: string | null, method?: string): void =>
      recordMcpToolCall({
        serverId,
        serverName: params.server,
        agent: caller,
        method,
        tool: method ? null : toolOf(member),
        outcome,
        status,
        latencyMs: Date.now() - startedAt,
        error,
      })
    const recordAll = (outcome: McpCallOutcome, status: number, error: string): void => {
      for (const m of members) record(m, outcome, status, error)
    }

    let eff: EffectiveMcp | null
    try {
      eff = await effectiveMcpFor(caller, params.server)
    } catch (e) {
      // An outage shows up in the record as a burst of these, not as silence.
      recordAll('error', 500, ACCESS_FAILED)
      throw e
    }
    if (!eff) {
      recordAll('denied', 403, NO_ACCESS)
      return json({ error: NO_ACCESS }, { status: 403 })
    }
    serverId = eff.server.id
    // A disabled app serves nothing, whatever its row says (`appMcpOff`).
    const { appMcpOff } = await import('@/server/app-mcp')
    const off = await appMcpOff(eff.server.appSlug, eff.server.appOff)
    if (off) {
      recordAll('denied', 403, off)
      return json({ error: off }, { status: 403 })
    }

    // A batch would ride past the tool gate, the spend boundary and the
    // dispatcher, which all read a single object — and an in-process server
    // answers one request at a time. Refused per member, each tools/call in it
    // recorded as refused. (The 2025-06 MCP revision dropped batching.)
    if (Array.isArray(body)) {
      console.warn(`[mcp/gw:${params.server}] ${name} sent a batch; refused`)
      recordAll('denied', 200, BATCH_REFUSED)
      return json(batchAnswer(body), { status: 200 })
    }
    if (!parsed.ok) {
      console.warn(`[mcp/gw:${params.server}] unparseable body (${bytes.length} bytes) | ${parsed.diagnostic}`)
      return json(rpcError(null, -32700, parsed.message), { status: 200 })
    }
    const invalid = invalidRequest(body)
    if (invalid) return json(invalid, { status: 200 })
    const rpc = body as Rpc
    const method = rpc.method ?? ''
    const isCall = method === 'tools/call'
    // The agent's own argument strings, taken before the spend boundary
    // rewrites them — the list a stored sentence is scrubbed of.
    const argStrings = isCall ? argumentStrings(rpc) : []

    // The method gate: a tool subset closes the resource, prompt and
    // completion surfaces too — recorded under the request's own method, so
    // the record shows what a narrowed agent reached for.
    if (eff.tools !== null && methodClosedBySubset(method)) {
      record(rpc, 'denied', 200, methodRefusal(method), method)
      return json(rpcError(rpc.id, -32601, methodRefusal(method)), { status: 200 })
    }
    // The call gate: reject disallowed tools before the module ever hears
    // about them.
    if (isCall && eff.tools !== null && !eff.tools.includes(toolOf(rpc) ?? '')) {
      const sentence = `tool "${rpc.params?.name}" is not available here`
      record(rpc, 'denied', 200, sentence)
      return json(rpcError(rpc.id, -32602, sentence), { status: 200 })
    }

    // THE BOUNDARY THAT SPENDS A CREDENTIAL. An agent holds
    // `«secret:deploy.github_pat»` and passes it wherever the value would go;
    // this is where the value actually appears, on its way OUT. It has to be
    // here, before the dispatch below, because this route is the only thing an
    // agent's tool call goes through — see `spendHandlesInToolCall` for what
    // forwarding the handle verbatim looked like. The dispatch takes the
    // mutated `rpc`. An unresolved handle is reported to the OPERATOR and
    // never back to the model: a caller that learns which names exist has
    // been handed a map of the workspace's credentials. The handler can tell
    // one apart from a value (`isUnresolvedHandle`). The boundary takes the
    // DESTINATION — `talaria-app://<slug>`, which a secret's allowlist must
    // name to admit — and whether the caller is PROVEN (a legacy shared-key
    // caller spends nothing).
    const { spendHandlesInToolCall } = await import('@/server/workspace-secrets')
    const destination = `talaria-app://${(eff.server.appSlug ?? params.server.replace(/^app-/, '')).toLowerCase()}`
    const spend = await spendHandlesInToolCall(rpc, name, destination, !caller.legacy)
    for (const u of spend.used) console.warn(`[secrets] ${name} spent ${u.name}.${u.key} (${u.label}) on ${params.server}.${rpc.params?.name}`)
    for (const u of spend.unresolved) console.warn(`[secrets] ${name} could not resolve ${u.handle} on ${params.server}.${rpc.params?.name}: ${u.reason}`)

    // App-published servers dispatch IN-PROCESS — no HTTP hop, tool subset
    // enforced again inside — under the server's deadline.
    if (eff.server.appSlug) {
      const { dispatchAppMcp } = await import('@/server/app-mcp')
      const r = await dispatchAppMcp(
        eff.server.appSlug,
        rpc,
        {
          caller: { name, id: caller.id, proven: !caller.legacy },
          actingUserId: eff.actingUserId,
          signal: request.signal,
          timeoutSecs: eff.server.timeoutSecs,
          // A harness the tool runs reaches an org MCP tool as THIS agent,
          // presenting its own key to the gateway — only a proven caller
          // lends its identity (app-org-tools.ts).
          orgTools: caller.legacy ? null : orgToolsFor(request, name),
        },
        eff.tools,
      )
      if (r.cut === 'caller') {
        // Nobody is left to answer; the record says the caller left.
        if (isCall) record(rpc, 'cancelled', 499, CALLER_GONE)
        return new Response(null, { status: 499 })
      }
      if (isCall) {
        if (r.cut === 'deadline') {
          record(rpc, 'error', r.status, toolTimedOut(timeoutSecsOf(eff.server.timeoutSecs)))
        } else {
          // A call that spent a credential keeps no dispatcher-authored
          // sentence at all — the reply may quote the real value back.
          const verdict = readCallVerdict(r.body, spend.used.length > 0, argStrings)
          record(rpc, verdict.outcome, r.status, verdict.error)
        }
      }
      return r.body === null ? new Response(null, { status: r.status }) : json(r.body, { status: r.status })
    }

    // Anything else is not this process's to serve (see the header): refuse
    // rather than grow a second relay that could disagree with the api's.
    if (isCall) record(rpc, 'denied', 404, 'not an app-published server')
    return json({ error: 'not an app-published server' }, { status: 404 })
  },
  // Streamable-HTTP GET (server → client notification stream): app servers
  // have none.
  GET: async ({ request, params }) => {
    const caller = await requireAgent(request)
    if (caller instanceof Response) return caller
    // Same rule as POST: the caller carries the proof, the string does not.
    const eff = await effectiveMcpFor(caller, params.server)
    if (!eff) return json({ error: 'no access to this MCP server' }, { status: 403 })
    const { appMcpOff } = await import('@/server/app-mcp')
    const off = await appMcpOff(eff.server.appSlug, eff.server.appOff)
    if (off) return json({ error: off }, { status: 403 })
    if (eff.server.appSlug) return new Response(null, { status: 405 })
    return json({ error: 'not an app-published server' }, { status: 404 })
  },
})
