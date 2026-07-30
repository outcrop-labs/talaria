import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { effectiveMcpFor, parseMcpResponse } from '@/server/mcp-registry'

// The MCP gateway — the registry's ENFORCEMENT point. Agents never see an
// upstream URL or credential: their configs point here, the fleet key + agent
// name identify the caller, and the gateway
//   · forwards JSON-RPC to the upstream (org headers, or the acting user's
//     connected-account headers on per-user servers)
//   · FILTERS tools/list down to the allowed set
//   · REJECTS tools/call outside it
// so a hand-edited agent config can never exceed what the registry granted.
export const Route = createFileRoute('/api/mcp/gw/$server')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        if (!checkAgentKey(request)) return json({ error: 'unauthorized' }, { status: 401 })
        const name = agentName(request)
        if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
        const eff = await effectiveMcpFor(name, params.server)
        if (!eff) return json({ error: 'no access to this MCP server' }, { status: 403 })

        const bodyText = await request.text()
        interface Rpc {
          id?: unknown
          method?: string
          params?: { name?: string }
        }
        let rpc: Rpc | null = null
        try {
          rpc = JSON.parse(bodyText) as Rpc
        } catch {
          /* non-JSON (batch or ping) — pass through untouched */
        }

        // The call gate: reject disallowed tools before the upstream ever
        // hears about them.
        if (rpc?.method === 'tools/call' && eff.tools !== null && !eff.tools.includes(rpc.params?.name ?? '')) {
          return json(
            {
              jsonrpc: '2.0',
              id: rpc.id ?? null,
              error: { code: -32602, message: `tool "${rpc.params?.name}" is not available here` },
            },
            { status: 200 },
          )
        }

        // The Workbench surface dispatches IN-PROCESS with the caller's agent
        // identity — grants resolved by the same gateway rules as any server.
        if (eff.server.url.startsWith('talaria-workbench://')) {
          const { dispatchWorkbenchMcp } = await import('@/server/workbench-mcp')
          const r = await dispatchWorkbenchMcp(rpc ?? {}, name, eff.tools)
          return r.body === null ? new Response(null, { status: r.status }) : json(r.body, { status: r.status })
        }

        // App-published servers dispatch IN-PROCESS — same access resolution
        // as above, no HTTP hop, tool subset enforced again inside.
        if (eff.server.appSlug) {
          const { dispatchAppMcp } = await import('@/server/app-mcp')
          const r = await dispatchAppMcp(eff.server.appSlug, rpc ?? {}, name, eff.tools)
          return r.body === null ? new Response(null, { status: r.status }) : json(r.body, { status: r.status })
        }

        const upstream = await fetch(eff.server.url, {
          method: 'POST',
          headers: {
            'content-type': request.headers.get('content-type') ?? 'application/json',
            accept: request.headers.get('accept') ?? 'application/json, text/event-stream',
            ...(request.headers.get('mcp-session-id') ? { 'mcp-session-id': request.headers.get('mcp-session-id')! } : {}),
            ...eff.upstreamHeaders,
            // The builtin toolkit keeps per-agent identity: the caller's name
            // travels through, authenticated by the fleet key.
            ...(eff.server.builtin ? { 'X-Agent-Name': name, 'X-Api-Key': process.env.TALARIA_AGENT_KEY ?? '' } : {}),
          },
          body: bodyText,
          signal: AbortSignal.timeout((eff.server.timeoutSecs ?? 120) * 1000),
        }).catch((e: Error) => e)
        if (upstream instanceof Error) return json({ error: `upstream unreachable: ${upstream.message}` }, { status: 502 })

        const respHeaders: Record<string, string> = {
          'content-type': upstream.headers.get('content-type') ?? 'application/json',
        }
        const session = upstream.headers.get('mcp-session-id')
        if (session) respHeaders['mcp-session-id'] = session

        // The list filter: rewrite result.tools inside JSON or SSE-framed
        // bodies. Everything else streams back verbatim.
        if (rpc?.method === 'tools/list' && eff.tools !== null) {
          const text = await upstream.text()
          const allowed = new Set(eff.tools)
          const filterMsg = (msg: unknown): unknown => {
            const m = msg as { result?: { tools?: Array<{ name: string }> } } | null
            if (m?.result?.tools) m.result.tools = m.result.tools.filter((t) => allowed.has(t.name))
            return msg
          }
          const ct = respHeaders['content-type'] ?? ''
          if (ct.includes('text/event-stream')) {
            const out = text
              .split('\n')
              .map((line) => {
                if (!line.startsWith('data:')) return line
                try {
                  return `data: ${JSON.stringify(filterMsg(JSON.parse(line.slice(5).trim())))}`
                } catch {
                  return line
                }
              })
              .join('\n')
            return new Response(out, { status: upstream.status, headers: respHeaders })
          }
          const msg = parseMcpResponse(text)
          if (msg) return new Response(JSON.stringify(filterMsg(msg)), { status: upstream.status, headers: respHeaders })
          return new Response(text, { status: upstream.status, headers: respHeaders })
        }

        return new Response(upstream.body, { status: upstream.status, headers: respHeaders })
      },
      // Streamable-HTTP GET (server → client notification stream): plain relay.
      GET: async ({ request, params }) => {
        if (!checkAgentKey(request)) return json({ error: 'unauthorized' }, { status: 401 })
        const name = agentName(request)
        if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
        const eff = await effectiveMcpFor(name, params.server)
        if (!eff) return json({ error: 'no access to this MCP server' }, { status: 403 })
        // App servers have no notification stream — decline politely.
        if (eff.server.appSlug) return new Response(null, { status: 405 })
        const upstream = await fetch(eff.server.url, {
          method: 'GET',
          headers: {
            accept: request.headers.get('accept') ?? 'text/event-stream',
            ...(request.headers.get('mcp-session-id') ? { 'mcp-session-id': request.headers.get('mcp-session-id')! } : {}),
            ...eff.upstreamHeaders,
            ...(eff.server.builtin ? { 'X-Agent-Name': name, 'X-Api-Key': process.env.TALARIA_AGENT_KEY ?? '' } : {}),
          },
        }).catch((e: Error) => e)
        if (upstream instanceof Error) return json({ error: `upstream unreachable: ${upstream.message}` }, { status: 502 })
        return new Response(upstream.body, {
          status: upstream.status,
          headers: { 'content-type': upstream.headers.get('content-type') ?? 'text/event-stream' },
        })
      },
    },
  },
})
