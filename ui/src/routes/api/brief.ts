import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { getBrief, sweepIfDue } from '@/server/daily-brief'

export const Route = defineApi('/api/brief', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    // Sweep BEFORE the read, not after, and only if the throttle allows it. A
    // person opening the surface is the one moment latency is visible, and
    // sweeping afterwards would hand them the stale page and the fresh one a
    // realtime nudge later — a document that visibly rewrites itself on arrival,
    // which is the exact impression an append-only brief must never give.
    //
    // Detached would be worse for the same reason, and NOT detached costs at
    // most four scoped queries: `sweepIfDue` returns immediately unless the
    // brief is past its sweep interval, so an open tab polling this route does
    // the work once per interval and nothing on every other call.
    await sweepIfDue(user).catch((e: unknown) => console.error('[brief] on-read sweep failed:', e))
    // The reader's IANA zone, from their browser. The org config's zone is a
    // server-side default; without this, a person's evening can read as the
    // small hours of tomorrow in UTC and hide a brief that exists. Validated
    // and discarded inside `getBrief` if unparseable.
    const tz = new URL(request.url).searchParams.get('tz')
    return json(await getBrief(user, new Date(), tz))
  },
})
