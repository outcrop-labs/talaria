import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { getSessionUser } from '@/server/auth/session'
import { agentCaller } from '@/server/agent-auth'
import { canAccessUpload, getUpload, serveUpload } from '@/server/uploads'

// GET → serve an attachment's bytes: signed-in users, or fleet agents (agent
// key) pulling ticket/chat attachments they were handed. The inline/download
// decision lives in serveUpload (server/uploads.ts) — one allowlist, no route
// widens it on its own.
export const Route = defineApi('/api/uploads/$id', {
  GET: async ({ request, params }) => {
    // Bytes only for viewers who can reach this upload through something
    // they can already read — never by id alone.
    let allowed = false
    const caller = await agentCaller(request)
    if (caller instanceof Response) return caller
    if (caller) {
      const name = caller.model
      allowed = await canAccessUpload(params.id, { agent: name })
    } else {
      const user = await getSessionUser(request)
      if (!user) return json({ error: 'unauthorized' }, { status: 401 })
      allowed = await canAccessUpload(params.id, { userId: user.id, who: user.email ?? user.name, isAdmin: user.role === 'admin' })
    }
    if (!allowed) return json({ error: 'not found' }, { status: 404 })
    const up = await getUpload(params.id)
    if (!up) return json({ error: 'not found' }, { status: 404 })
    return serveUpload(up, { cache: 'private, max-age=86400' })
  },
})
