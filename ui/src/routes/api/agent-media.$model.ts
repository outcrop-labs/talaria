import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { canUseAgentModel } from '@/server/users'
import { isMediaError, readAgentImage } from '@/server/agent-media'

// GET ?path=/opt/data/ → stream an image out of the agent's container, so
// media agents produce ("MEDIA:<path>" in replies) renders inline in chat.
// Access + path/type guardrails live in server/agent-media.
export const Route = createFileRoute('/api/agent-media/$model')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await canUseAgentModel(user.id, user.role, params.model))) {
          return json({ error: 'forbidden' }, { status: 403 })
        }
        const path = new URL(request.url).searchParams.get('path') ?? ''
        const media = await readAgentImage(params.model, path)
        if (isMediaError(media)) return json({ error: media.error }, { status: media.status })
        // media.bytes is a fresh exact-size copy, so its backing buffer is safe.
        return new Response(media.bytes.buffer as ArrayBuffer, {
          headers: {
            'content-type': media.mime,
            'cache-control': 'private, max-age=300',
            'x-content-type-options': 'nosniff',
          },
        })
      },
    },
  },
})
