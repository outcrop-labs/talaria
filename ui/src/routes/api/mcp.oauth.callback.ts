import { createFileRoute } from '@tanstack/react-router'
import { handleOauthCallback } from '@/server/mcp-oauth'
import { renderFleet } from '@/server/fleet-render'
import { logAudit } from '@/server/audit'

const page = (title: string, body: string) =>
  new Response(
    `<!doctype html><meta charset="utf-8"><title>${title}</title>
<body style="font-family:ui-monospace,monospace;background:#0b0c0e;color:#e6e6e6;display:grid;place-items:center;height:100vh;margin:0">
<div style="text-align:center"><p style="font-size:15px">${body}</p>
<p style="font-size:12px;opacity:.6">This window closes itself.</p></div>
<script>setTimeout(()=>{ if (window.opener) window.close(); else location.href='/mcp' }, 1200)</script></body>`,
    { headers: { 'content-type': 'text/html; charset=utf-8' } },
  )

// The OAuth redirect target. No session requirement — identity was bound to
// the state row when the flow started; the state is single-use and expiring.
export const Route = createFileRoute('/api/mcp/oauth/callback')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const url = new URL(request.url)
        const err = url.searchParams.get('error')
        if (err) return page('Connection failed', `The provider said: ${url.searchParams.get('error_description') ?? err}`)
        const state = url.searchParams.get('state')
        const code = url.searchParams.get('code')
        if (!state || !code) return page('Connection failed', 'Missing code or state.')
        try {
          const { subject, serverId } = await handleOauthCallback(state, code)
          void logAudit({
            actor: subject === 'org' ? 'org' : subject,
            action: 'mcp.oauth_connect',
            targetType: 'mcp-server',
            targetId: serverId,
          })
          void renderFleet().catch(() => {}) // connected servers appear in configs
          return page('Connected', 'Connected — your agents can use this server now.')
        } catch (e) {
          return page('Connection failed', (e as Error).message)
        }
      },
    },
  },
})
