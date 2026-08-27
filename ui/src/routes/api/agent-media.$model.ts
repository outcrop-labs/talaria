import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { canUseAgentModel } from '@/server/users'
import { isMediaError, readAgentImage } from '@/server/agent-media'

// GET ?path=/opt/data/ → stream an image out of the agent's container, so
// media agents produce ("MEDIA:<path>" in replies) renders inline in chat.
// Access + path/type guardrails live in server/agent-media.
export const Route = defineApi('/api/agent-media/$model', {
  GET: async ({ request, params }) => {
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
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
})
