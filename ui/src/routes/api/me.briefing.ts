import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireUser } from '@/server/api-guard'
import { getBriefing } from '@/server/briefing'

// GET → the assistant's attention briefing for the Inbox. Fast: returns the
// current (possibly stale) summary immediately and regenerates detached when
// the attention fingerprint changed; the client polls while `generating`.
export const Route = createFileRoute('/api/me/briefing')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const raw = new URL(request.url).searchParams.get('scope')
        const scope = (['inbox', 'boards', 'comms', 'plans', 'research'] as const).find((v) => v === raw) ?? 'inbox'
        try {
          return json(await getBriefing(user.id, user.role === 'admin', scope))
        } catch (e) {
          console.error('[me.briefing]', e)
          return json({ error: 'briefing failed — see server logs' }, { status: 500 })
        }
      },
    },
  },
})
