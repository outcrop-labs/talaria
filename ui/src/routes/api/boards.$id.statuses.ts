import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { boardRole, canEdit } from '@/server/boards'
import {
  agentStartConflict,
  createStatus,
  deleteStatus,
  listStatuses,
  reorderStatuses,
  statusDiagnostics,
  updateStatus,
} from '@/server/statuses'

/** The audit name for a board edit that MOVES TICKETS (deleting a populated
 *  column, or recategorising a populated sign-off column). Both take an actor
 *  because both land on each ticket's activity log, and they must agree on how
 *  the person is named. */
const actorOfUser = (user: { email: string | null; name: string | null }) => user.email ?? user.name ?? 'user'

// Board statuses (custom workflow columns). GET → the ordered list incl. the
// system Blocked column (any member). POST create, PUT update/reorder, DELETE
// (tickets reassigned) — owner/editor. Category + agentStart carry the
// workflow semantics; Blocked is system and not editable here.
const Category = z.enum(['open', 'active', 'review', 'done'])

// ── Cross-validation ─────────────────────────────────────────────────────────
// An agent-start column is the queue agents pick work UP from, so it cannot
// also be a HUMAN GATE:
//   • `review` + agentStart is a loop — the agent's own hand-off drops the
//     ticket straight back into its pickup queue;
//   • `done` + agentStart turns CLOSING a ticket into a dispatch — the move to
//     done fires updateTask's re-dispatch branch and starts a live work session
//     on the ticket a person just signed off.
// Either way it hands an agent a legitimate-looking write into a column the
// assignment gate would otherwise refuse. Zod can't see it (either flag may
// arrive alone, on top of whatever the column already is), so the check runs
// against the EFFECTIVE post-patch column. `statusKey` null = a create, which
// inherits createStatus's defaults.
//
// The rule itself lives with the data (statuses.ts refuses the same write from
// any caller); this stays because a 400 with the reason beats a thrown error,
// and because it runs BEFORE materialize() copies the defaults in.
async function humanGateConflict(
  boardId: string,
  statusKey: string | null,
  patch: { category?: z.infer<typeof Category>; agentStart?: boolean },
): Promise<string | null> {
  if (patch.category === undefined && patch.agentStart === undefined) return null
  const cur = statusKey ? (await listStatuses(boardId)).find((s) => s.key === statusKey) : undefined
  const category = patch.category ?? cur?.category ?? 'active'
  const agentStart = patch.agentStart ?? cur?.agentStart ?? false
  // The system Blocked column is the one entry with a non-StatusCategory
  // category; it is never editable here (updateStatus refuses the key), and it
  // is never agent-start, so the cast can only see a real workflow category.
  return agentStartConflict(category as z.infer<typeof Category>, agentStart)
}

export const Route = createFileRoute('/api/boards/$id/statuses')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        // `diagnostics` rides along with the columns because it is a statement
        // ABOUT this column set, and the two must never be read from different
        // moments — a warning that names a column the list no longer has is
        // worse than no warning. Served to every member (the reader who cannot
        // fix it can at least tell the owner why the board is stuck), and read
        // by the statuses tab, which renders it above the list.
        const [statuses, diagnostics] = await Promise.all([listStatuses(params.id), statusDiagnostics(params.id)])
        return json({ statuses, diagnostics })
      },
      POST: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!canEdit(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(
          request,
          z.object({
            label: z.string().min(1).max(40),
            color: z.string().max(20).optional(),
            category: Category.optional(),
            agentStart: z.boolean().optional(),
          }),
        )
        if (body instanceof Response) return body
        const conflict = await humanGateConflict(params.id, null, body)
        if (conflict) return json({ error: conflict }, { status: 400 })
        try {
          return json({ status: await createStatus(params.id, body) })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
      PUT: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!canEdit(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(
          request,
          z.union([
            z.object({
              statusKey: z.string().min(1).max(40),
              label: z.string().min(1).max(40).optional(),
              color: z.string().max(20).optional(),
              category: Category.optional(),
              agentStart: z.boolean().optional(),
            }),
            z.object({ order: z.array(z.string().min(1).max(40)).min(1).max(50) }),
          ]),
        )
        if (body instanceof Response) return body
        if (!('order' in body)) {
          const conflict = await humanGateConflict(params.id, body.statusKey, body)
          if (conflict) return json({ error: conflict }, { status: 400 })
        }
        try {
          if ('order' in body) await reorderStatuses(params.id, body.order)
          // The actor: recategorising a populated review/done column moves its
          // tickets into a surviving column of the same category, one updateTask
          // each, and the person who reshaped the column owns those moves.
          else await updateStatus(params.id, body.statusKey, body, actorOfUser(user))
          return json({ ok: true })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
      DELETE: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!canEdit(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(request, z.object({ statusKey: z.string().min(1).max(40), reassignTo: z.string().max(40) }))
        if (body instanceof Response) return body
        try {
          // The actor is threaded through because the reassignment lands on each
          // ticket's activity log — deleting a column moves work, and the person
          // who did it owns that move.
          await deleteStatus(params.id, body.statusKey, body.reassignTo, actorOfUser(user))
          return json({ ok: true })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
