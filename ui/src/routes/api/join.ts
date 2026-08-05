import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { inviteByToken } from '@/server/invites'

// Public invite lookup for the /join page: token → who's invited, by whom,
// to which org. Expired/revoked/accepted tokens read as gone.
export const Route = defineApi('/api/join', {
  GET: async ({ request }) => {
    const token = new URL(request.url).searchParams.get('token') ?? ''
    if (!token) return json({ error: 'missing token' }, { status: 400 })
    const invite = await inviteByToken(token)
    if (!invite) return json({ error: 'invite not found or no longer valid' }, { status: 404 })
    return json({ invite })
  },
})
