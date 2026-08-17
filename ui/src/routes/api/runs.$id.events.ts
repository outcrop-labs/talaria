import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { mayWatchRun, runEventStream } from '@/server/realtime'
// SIDE-EFFECT IMPORT, AND IT IS LOAD-BEARING — see server/runs/boot.ts. It
// registers the reclaim sweep and every run kind, and this route is how they
// reach the server graph: `src/server/app.ts` eagerly globs every module under
// `src/routes/api/**`, which is the same path comms-decay, the digest and the
// notification mailer take to the scheduler. Without it `run-reclaim` never
// arms (and REQUIRED_JOBS turns that into a loud boot error) and a run whose
// driver died is never resumed by anybody.
import '@/server/runs/boot'

// GET /api/runs/:id/events → SSE stream of one run's live transitions (state,
// phase, terminal error). Auth-gated by the run's read ACL. This is what makes
// a long action attachable: a tab that was closed, a view that was navigated
// away from, or a second device can re-attach to the SAME server-owned record
// and see where it actually is.
//
// THE ORDER OF THE TWO LINES IN THE BODY IS LOAD-BEARING. The gate runs to
// completion BEFORE `runEventStream`, which is the only arrangement that gets
// both halves right at once:
//
//   the ACL half   — obvious, and the same shape as boards/channels above.
//   the RESOURCE half — `runEventStream` opens a DEDICATED Redis connection per
//                       client. Creating one and then returning 403 would leak
//                       a subscriber per rejected request, and a rejected
//                       request is exactly the kind a caller retries in a loop.
//                       Nothing downstream disconnects a stream nobody was
//                       handed: the cleanup in `topicEventStream` hangs off the
//                       request's abort signal and the ReadableStream's cancel,
//                       and neither fires for a stream that was never returned.
//
// Every refusal answers 403, including "no such run" — see the note in
// `mayWatchRun` about not turning a guessable id into an existence oracle.
export const Route = defineApi('/api/runs/$id/events', {
  GET: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const verdict = await mayWatchRun(user.id, params.id)
    if (!verdict.ok) return json({ error: 'forbidden' }, { status: 403 })
    return new Response(runEventStream(params.id, request.signal), {
      headers: {
        'Content-Type': 'text/event-stream',
        'Cache-Control': 'no-cache, no-transform',
        Connection: 'keep-alive',
      },
    })
  },
})
