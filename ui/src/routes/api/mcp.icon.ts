import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireUser } from '@/server/api-guard'
import { resolveIcon } from '@/server/mcp-icons'

// FALLBACK marketplace icons: the publisher's favicon, proxied + cached
// server-side (warmed in bulk when library pages are served). Registry-
// declared icons hotlink directly from the client and never come through here.
export const Route = createFileRoute('/api/mcp/icon')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const domain = new URL(request.url).searchParams.get('domain')
        if (!domain || !/^[a-z0-9.-]+$/i.test(domain)) return json({ error: 'bad request' }, { status: 400 })
        const icon = await resolveIcon({ domain })
        if (!icon) return new Response(null, { status: 404 })
        return new Response(icon.buf, { headers: { 'content-type': icon.ct, 'cache-control': 'public, max-age=86400' } })
      },
    },
  },
})
