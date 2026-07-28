import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { getBriefing } from '@/server/briefing'

// GET → the assistant's attention briefing for the Inbox. Fast: returns the
// current (possibly stale) summary immediately and regenerates detached when
// the attention fingerprint changed; the client polls while `generating`.
export const Route = createFileRoute('/api/me/briefing')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const raw = new URL(request.url).searchParams.get('scope')
        const scope = (['inbox', 'boards', 'comms', 'plans', 'research'] as const).find((v) => v === raw) ?? 'inbox'
        try {
          return json(await getBriefing(user.id, user.role === 'admin', scope))
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 500 })
        }
      },
    },
  },
})
