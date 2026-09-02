import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireAgent } from '@/server/agent-auth'
import { effectiveMcpFor } from '@/server/mcp-registry'
import { rpcError, type Rpc } from '@/server/mcp-jsonrpc'

// The app-MCP gateway — the registry's ENFORCEMENT point for servers an app
// publishes (/api/mcp/gw/app-<slug>). Agents never see the module or its
// inputs: their configs point here, the agent's own credential identifies the
// caller (agent-auth), and this route resolves the agent's grant (assignment ∩
// the acting owner's allowance) and dispatches the compiled module IN PROCESS
// — app code runs in the TS runtime that can load it (RUST-MIGRATION.md,
// rule 10). Every other MCP server — the builtin toolkit, the Workbench,
// third-party URLs — is the Rust api's relay (api/src/routes/mcp/
// mcp_gw_server.rs); the proxy holds back only the app- prefix, so those
// requests never reach this file.
export const Route = defineApi('/api/mcp/gw/$server', {
  POST: async ({ request, params }) => {
    const caller = await requireAgent(request)
    if (caller instanceof Response) return caller
    // Pass the CALLER, never `caller.model`. `subjectModel`/`subjectProven`
    // read a bare string as PROVEN, so downgrading to the name here throws
    // away `legacy` — and this route is where that matters most: it resolves
    // the acting owner and gates what that human's assistant may reach.
    // `name` below is only ever used where an unthreaded callee genuinely
    // needs the string.
    const name = caller.model
    const eff = await effectiveMcpFor(caller, params.server)
    if (!eff) return json({ error: 'no access to this MCP server' }, { status: 403 })

    let rpc: Rpc | null = null
    try {
      rpc = JSON.parse(await request.text()) as Rpc
    } catch {
      /* not valid JSON-RPC — the dispatcher below answers it */
    }

    // The call gate: reject disallowed tools before the module ever hears
    // about them.
    if (rpc?.method === 'tools/call' && eff.tools !== null && !eff.tools.includes(rpc.params?.name ?? '')) {
      return json(
        rpcError(rpc.id, -32602, `tool "${rpc.params?.name}" is not available here`),
        { status: 200 },
      )
    }

    // THE BOUNDARY THAT SPENDS A CREDENTIAL. An agent holds
    // `«secret:deploy.github_pat»` and passes it wherever the value would go;
    // this is where the value actually appears, on its way OUT. It has to be
    // here, before the dispatch below, because this route is the only thing an
    // agent's tool call goes through — see `spendHandlesInToolCall` for what
    // forwarding the handle verbatim looked like. The dispatch takes the
    // mutated `rpc`. An unresolved handle is reported to the OPERATOR and
    // never back to the model: a caller that learns which names exist has
    // been handed a map of the workspace's credentials.
    const { spendHandlesInToolCall } = await import('@/server/workspace-secrets')
    const spend = await spendHandlesInToolCall(rpc, name)
    for (const u of spend.used) console.warn(`[secrets] ${name} spent ${u.name}.${u.key} (${u.label}) on ${params.server}.${rpc?.params?.name}`)
    for (const u of spend.unresolved) console.warn(`[secrets] ${name} could not resolve ${u.handle} on ${params.server}.${rpc?.params?.name}: ${u.reason}`)

    // App-published servers dispatch IN-PROCESS — no HTTP hop, tool subset
    // enforced again inside.
    if (eff.server.appSlug) {
      const { dispatchAppMcp } = await import('@/server/app-mcp')
      const r = await dispatchAppMcp(eff.server.appSlug, rpc ?? {}, name, eff.tools)
      return r.body === null ? new Response(null, { status: r.status }) : json(r.body, { status: r.status })
    }

    // Anything else is not this process's to serve (see the header): refuse
    // rather than grow a second relay that could disagree with the api's.
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
    if (eff.server.appSlug) return new Response(null, { status: 405 })
    return json({ error: 'not an app-published server' }, { status: 404 })
  },
})
