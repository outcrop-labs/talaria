import { defineApi } from '@/server/api-route'
import { handleOauthCallback } from '@/server/mcp-oauth'
import { renderFleet } from '@/server/fleet-render'
import { rollAgentsForServer, rollAgentForUser } from '@/server/mcp-apply'
import { logAudit } from '@/server/audit'

// Both interpolations carry attacker-reachable text (the provider's
// error_description, a thrown message), and this page is unauthenticated by
// design and built to be opened from the app — so escape them, and pin a CSP
// that allows nothing but the inline script/style this page is built from.
const esc = (s: string) =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;')

const page = (title: string, body: string) =>
  new Response(
    `<!doctype html><meta charset="utf-8"><title>${esc(title)}</title>
<body style="font-family:ui-monospace,monospace;background:#0b0c0e;color:#e6e6e6;display:grid;place-items:center;height:100vh;margin:0">
<div style="text-align:center"><p style="font-size:15px">${esc(body)}</p>
<p style="font-size:12px;opacity:.6">This window closes itself.</p></div>
<script>
try { window.opener && window.opener.postMessage({ type: 'talaria:mcp-oauth-done' }, window.location.origin) } catch {}
setTimeout(()=>{ if (window.opener) window.close(); else location.href='/mcp' }, 1200)
</script></body>`,
    {
      headers: {
        'content-type': 'text/html; charset=utf-8',
        // 'unsafe-inline' covers the postMessage/close script and the inline
        // styles; everything else — remote script, fetch, frames — is denied.
        'content-security-policy': "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'",
        'x-content-type-options': 'nosniff',
        'referrer-policy': 'no-referrer',
      },
    },
  )

// The OAuth redirect target. No session requirement — identity was bound to
// the state row when the flow started; the state is single-use and expiring.
export const Route = defineApi('/api/mcp/oauth/callback', {
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
      // Running agents wire MCP at start — roll the affected ones so the
      // connection is usable without anyone bouncing containers.
      if (subject === 'org') void rollAgentsForServer(serverId).catch(() => {})
      else void rollAgentForUser(subject).catch(() => {})
      return page('Connected', 'Connected — your agents are picking it up now (a graceful restart runs behind the scenes).')
    } catch (e) {
      return page('Connection failed', (e as Error).message)
    }
  },
})
